use std::io::{Read, Write};
use std::sync::Arc;
#[cfg(feature = "ghostty")]
use std::sync::OnceLock;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TryRecvError, sync_channel};
use std::thread;
use std::time::{Duration, Instant};

use mux_protocol::{ProcessExit, ServerEvent, SpawnCommand};
#[cfg(not(feature = "ghostty"))]
use mux_terminal::ReplayEngine;
use mux_terminal::{
    KITTY_KEYBOARD_REPORT_EVENTS, KITTY_KEYBOARD_RESET_SEQUENCE, TerminalEngine, TerminalError,
    TerminalSize, TitleScanner,
};
#[cfg(feature = "ghostty")]
use mux_terminal_ghostty::{GhosttyEngine, GhosttyTheme};
use mux_workspace::{PaneId, SessionId};
use parking_lot::Mutex;
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use thiserror::Error;
use tokio::sync::broadcast;
use tracing::{debug, error, warn};

const READ_BUFFER_SIZE: usize = 64 * 1024;
const OUTPUT_BATCH_SIZE: usize = 64 * 1024;
const OUTPUT_BATCH_IDLE: Duration = Duration::from_micros(500);
/// Output that comes this soon after the last batch is part of a stream.
const OUTPUT_STREAM_GAP: Duration = Duration::from_millis(2);
/// How long output after a lull waits for more: long enough that a screen
/// drawn in one go is sent whole, short enough not to hold up a key's echo.
const OUTPUT_LULL_IDLE: Duration = Duration::from_micros(150);
/// How long a pane is quiet before its history is compressed, so the work
/// never lands while a program is printing.
const HISTORY_COMPRESSION_IDLE: Duration = Duration::from_secs(5);
/// Home, erase the screen, erase the history.
const CLEAR_SCREEN_AND_HISTORY: &[u8] = b"\x1b[H\x1b[2J\x1b[3J";
const CLEAR_HISTORY: &[u8] = b"\x1b[3J";
/// RIS: the terminal back as it started, modes, colours and screen.
const FULL_RESET: &[u8] = b"\x1bc";
/// Form feed, Ctrl+L: a shell at its prompt draws the prompt again.
const REDRAW_PROMPT: &[u8] = b"\x0c";

/// A way into the pane's output stream, behind what the PTY has said, for
/// bytes Mux itself has for the terminal. It closes with the PTY.
type OutputInjector = Arc<Mutex<Option<SyncSender<Vec<u8>>>>>;

#[derive(Debug)]
struct ForegroundProcessTracker {
    shell_process_id: Option<u32>,
    child_was_foreground: bool,
}

impl ForegroundProcessTracker {
    const fn new(shell_process_id: Option<u32>) -> Self {
        Self {
            shell_process_id,
            child_was_foreground: false,
        }
    }

    fn observe(&mut self, foreground_process_id: Option<u32>) -> bool {
        let Some(shell_process_id) = self.shell_process_id else {
            return false;
        };
        match foreground_process_id {
            Some(process_id) if process_id == shell_process_id => {
                std::mem::take(&mut self.child_was_foreground)
            }
            Some(_) => {
                self.child_was_foreground = true;
                false
            }
            None => false,
        }
    }
}

struct PaneOutputWorker {
    session_id: SessionId,
    pane_id: PaneId,
    terminal: Arc<Mutex<Box<dyn TerminalEngine>>>,
    response_writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    shell_process_id: Option<u32>,
    title: Arc<Mutex<TitleScanner>>,
    events: broadcast::Sender<ServerEvent>,
}

impl PaneOutputWorker {
    fn run(self, output_receiver: &Receiver<Vec<u8>>) {
        let mut foreground = ForegroundProcessTracker::new(self.shell_process_id);
        let mut last_batch: Option<Instant> = None;
        let mut compression_owed = false;
        loop {
            let first = if compression_owed {
                match output_receiver.recv_timeout(HISTORY_COMPRESSION_IDLE) {
                    Ok(first) => first,
                    Err(RecvTimeoutError::Timeout) => {
                        match self.compress_history(output_receiver) {
                            Ok(first) => first,
                            Err(TryRecvError::Empty) => {
                                compression_owed = false;
                                continue;
                            }
                            Err(TryRecvError::Disconnected) => break,
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            } else {
                match output_receiver.recv() {
                    Ok(first) => first,
                    Err(_) => break,
                }
            };
            compression_owed = true;
            // Output after a lull, such as the echo of a key, goes out almost
            // at once, with the rest of the screen if more is being drawn.
            // Output that keeps coming waits a moment for more, so a flood is
            // applied and sent in large batches.
            let streaming = last_batch.is_some_and(|at| at.elapsed() < OUTPUT_STREAM_GAP);
            let wait = if streaming {
                OUTPUT_BATCH_IDLE
            } else {
                OUTPUT_LULL_IDLE
            };
            let (mut bytes, disconnected) =
                collect_output_batch(first, output_receiver, Some(wait));
            last_batch = Some(Instant::now());
            let shell_regained = foreground.observe(foreground_process_id(&self.master));
            let (output_sequence, responses, reset_keyboard) = match self
                .apply_terminal_output(&bytes, shell_regained)
            {
                Ok(output) => output,
                Err(error) => {
                    error!(session_id = %self.session_id, pane_id = %self.pane_id, %error, "terminal engine failed to apply pane output");
                    break;
                }
            };
            if !responses.is_empty() && !self.write_responses(&responses) {
                break;
            }
            {
                let mut title = self.title.lock();
                title.scan(&bytes);
                // The windows put copies on the clipboard; the daemon has
                // nothing to keep them for.
                let _ = title.take_copied();
            }
            if reset_keyboard {
                warn!(
                    session_id = %self.session_id,
                    pane_id = %self.pane_id,
                    "reset stale Kitty keyboard mode after the shell regained the terminal"
                );
                bytes.extend_from_slice(KITTY_KEYBOARD_RESET_SEQUENCE);
            }
            let _ = self.events.send(ServerEvent::PaneOutput {
                session_id: self.session_id,
                pane_id: self.pane_id,
                sequence: output_sequence,
                bytes,
            });
            if disconnected {
                break;
            }
        }
    }

    /// Compress the quiet pane's history a step at a time, letting go of
    /// the terminal between steps, until it is done or output arrives.
    /// Gives back output that arrived, which leaves the rest for the next
    /// quiet spell, and `Empty` once the history is done.
    fn compress_history(
        &self,
        output_receiver: &Receiver<Vec<u8>>,
    ) -> Result<Vec<u8>, TryRecvError> {
        while self.terminal.lock().compress_history_step() {
            match output_receiver.try_recv() {
                Err(TryRecvError::Empty) => {}
                arrived => return arrived,
            }
        }
        output_receiver.try_recv()
    }

    fn apply_terminal_output(
        &self,
        bytes: &[u8],
        shell_regained: bool,
    ) -> Result<(u64, Vec<u8>, bool), TerminalError> {
        let mut terminal = self.terminal.lock();
        let output_sequence = terminal.next_output_sequence();
        terminal.apply_output(output_sequence, bytes)?;
        let reset_keyboard =
            shell_regained && terminal.kitty_keyboard_flags()? & KITTY_KEYBOARD_REPORT_EVENTS != 0;
        if reset_keyboard {
            terminal.reset_kitty_keyboard()?;
        }
        let responses = terminal.take_pty_responses()?;
        Ok((output_sequence, responses, reset_keyboard))
    }

    fn write_responses(&self, responses: &[u8]) -> bool {
        let mut writer = self.response_writer.lock();
        if writer
            .write_all(responses)
            .and_then(|()| writer.flush())
            .is_ok()
        {
            true
        } else {
            warn!(session_id = %self.session_id, pane_id = %self.pane_id, "failed writing terminal response to PTY");
            false
        }
    }
}

/// The output from `first` on as one batch: what is already queued, then,
/// with a `wait`, what follows before the queue has been empty that long.
fn collect_output_batch(
    first: Vec<u8>,
    receiver: &Receiver<Vec<u8>>,
    wait: Option<Duration>,
) -> (Vec<u8>, bool) {
    let mut bytes = first;
    while bytes.len() < OUTPUT_BATCH_SIZE {
        let next = match (receiver.try_recv(), wait) {
            (Ok(next), _) => Ok(next),
            (Err(TryRecvError::Empty), Some(wait)) => receiver.recv_timeout(wait),
            (Err(TryRecvError::Empty), None) => Err(RecvTimeoutError::Timeout),
            (Err(TryRecvError::Disconnected), _) => Err(RecvTimeoutError::Disconnected),
        };
        match next {
            Ok(next) => bytes.extend_from_slice(&next),
            Err(RecvTimeoutError::Timeout) => return (bytes, false),
            Err(RecvTimeoutError::Disconnected) => return (bytes, true),
        }
    }
    (bytes, false)
}

pub struct PaneRuntime {
    pub id: PaneId,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    terminal: Arc<Mutex<Box<dyn TerminalEngine>>>,
    exit_status: Arc<Mutex<Option<ProcessExit>>>,
    process_id: Option<u32>,
    /// The window title from the pane's output, for clients that attach
    /// after the program set it.
    title: Arc<Mutex<TitleScanner>>,
    /// The program in front of the shell when last looked, and since when.
    foreground_command: Mutex<Option<ForegroundCommand>>,
    /// Whether the terminal was reading a password when last looked.
    secret_input: Mutex<bool>,
    /// A process in front that still bears the shell's own name: either a
    /// fork that has not yet become the program it will run, or a nested
    /// shell. It is named only once seen twice.
    shell_named_pending: Mutex<Option<u32>>,
    output_injector: OutputInjector,
}

struct ForegroundCommand {
    since: Instant,
    name: String,
}

/// What changed in front of a pane's shell between two looks.
#[derive(Debug, Eq, PartialEq)]
pub enum ForegroundChange {
    /// This program has taken the terminal.
    Started(String),
    /// This program has given the terminal back after running this long.
    Finished(String, Duration),
}

/// The rule under the text a pane showed before a restart.
pub const HISTORY_RULE: &str = "── before Mux restarted ──";
/// The rule under the text a pane of a reopened tab showed before the tab
/// was closed.
pub const CLOSED_RULE: &str = "── before the tab was closed ──";
/// The rule under the text a reopened pane showed before it was closed.
pub const CLOSED_PANE_RULE: &str = "── before the pane was closed ──";

/// Text a pane showed before a restart, as output for a new terminal: each
/// line as it was, without anything that would act as a control, then a
/// faint rule saying where the past ends.
fn history_output(history: &str, rule: &str) -> Vec<u8> {
    let mut output = Vec::with_capacity(history.len() + 64);
    for line in history.trim_end().lines() {
        let line = line
            .chars()
            .filter(|character| !character.is_control())
            .collect::<String>();
        output.extend_from_slice(line.trim_end().as_bytes());
        output.extend_from_slice(b"\r\n");
    }
    output.extend_from_slice(format!("\x1b[2m{rule}\x1b[0m\r\n").as_bytes());
    output
}

/// Whether an inherited variable belongs to the terminal the daemon was
/// started from rather than to Mux.
fn names_another_terminal(key: &str) -> bool {
    const PREFIXES: [&str; 6] = [
        "GHOSTTY_",
        "ITERM_",
        "KITTY_",
        "WEZTERM_",
        "ALACRITTY_",
        "KONSOLE_",
    ];
    const NAMES: [&str; 5] = [
        "TERMINFO",
        "TERM_SESSION_ID",
        "LC_TERMINAL",
        "LC_TERMINAL_VERSION",
        "WT_SESSION",
    ];
    PREFIXES.iter().any(|prefix| key.starts_with(prefix)) || NAMES.contains(&key)
}

impl PaneRuntime {
    pub fn spawn(
        session_id: SessionId,
        id: PaneId,
        cwd: &std::path::Path,
        command: &SpawnCommand,
        size: TerminalSize,
        replay_bytes: usize,
        events: broadcast::Sender<ServerEvent>,
    ) -> Result<Arc<Self>, PaneError> {
        Self::spawn_with_history(
            session_id,
            id,
            cwd,
            command,
            size,
            replay_bytes,
            events,
            None,
        )
    }

    /// Start a pane that first shows `history`: the text a pane in its
    /// place showed before, then the rule that says when that was, so it
    /// reads as the past before the new shell's prompt comes up under it.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub fn spawn_with_history(
        session_id: SessionId,
        id: PaneId,
        cwd: &std::path::Path,
        command: &SpawnCommand,
        size: TerminalSize,
        replay_bytes: usize,
        events: broadcast::Sender<ServerEvent>,
        history: Option<(&str, &str)>,
    ) -> Result<Arc<Self>, PaneError> {
        let size = size.validate()?;
        let pair = native_pty_system().openpty(to_pty_size(size))?;

        let mut builder = CommandBuilder::new(&command.program);
        builder.args(&command.args);
        builder.cwd(cwd);
        builder.env("TERM", "xterm-256color");
        builder.env("COLORTERM", "truecolor");
        // Programs ask TERM_PROGRAM which terminal they are in. A daemon
        // started from inside another terminal, as one run from Ghostty is,
        // would otherwise hand its shells that terminal's name and the
        // variables that go with it, and they would take Mux for it.
        builder.env("TERM_PROGRAM", "mux");
        builder.env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));
        for (key, _) in std::env::vars_os() {
            if key.to_str().is_some_and(names_another_terminal) {
                builder.env_remove(key);
            }
        }
        for (key, value) in crate::shell_integration::environment(&command.program) {
            builder.env(key, value);
        }
        for (key, value) in &command.environment {
            builder.env(key, value);
        }

        let mut child = pair.slave.spawn_command(builder)?;
        let process_id = child.process_id();
        let killer = child.clone_killer();
        let reader = pair.master.try_clone_reader()?;
        let writer = Arc::new(Mutex::new(pair.master.take_writer()?));
        drop(pair.slave);

        let mut engine = create_terminal_engine(size, replay_bytes)?;
        if let Some((text, rule)) = history.filter(|(text, _)| !text.trim().is_empty()) {
            // Before the reader starts, so it lands ahead of the shell's
            // first output.
            let sequence = engine.next_output_sequence();
            engine.apply_output(sequence, &history_output(text, rule))?;
        }
        let terminal: Arc<Mutex<Box<dyn TerminalEngine>>> = Arc::new(Mutex::new(engine));
        let exit_status = Arc::new(Mutex::new(None));
        let master = Arc::new(Mutex::new(pair.master));
        let title = Arc::new(Mutex::new(TitleScanner::default()));
        let output_injector = spawn_reader(
            reader,
            PaneOutputWorker {
                session_id,
                pane_id: id,
                terminal: Arc::clone(&terminal),
                response_writer: Arc::clone(&writer),
                master: Arc::clone(&master),
                shell_process_id: process_id,
                title: Arc::clone(&title),
                events: events.clone(),
            },
        )?;

        let exit_status_for_thread = Arc::clone(&exit_status);
        thread::Builder::new()
            .name(format!("mux-pane-wait-{id}"))
            .spawn(move || match child.wait() {
                Ok(status) => {
                    let process_exit = ProcessExit {
                        code: Some(status.exit_code()),
                        success: status.success(),
                    };
                    *exit_status_for_thread.lock() = Some(process_exit);
                    let _ = events.send(ServerEvent::PaneExited {
                        session_id,
                        pane_id: id,
                        status: process_exit,
                    });
                    debug!(%session_id, pane_id = %id, ?process_id, %status, "pane process exited");
                }
                Err(error) => {
                    error!(%session_id, pane_id = %id, ?process_id, %error, "failed waiting for pane process");
                }
            })?;

        Ok(Arc::new(Self {
            id,
            master,
            writer,
            killer: Mutex::new(killer),
            terminal,
            exit_status,
            process_id,
            title,
            foreground_command: Mutex::new(None),
            secret_input: Mutex::new(false),
            shell_named_pending: Mutex::new(None),
            output_injector,
        }))
    }

    pub fn write(&self, bytes: &[u8]) -> Result<(), PaneError> {
        let mut writer = self.writer.lock();
        writer.write_all(bytes)?;
        writer.flush()?;
        Ok(())
    }

    pub fn resize(&self, size: TerminalSize) -> Result<(), PaneError> {
        let size = size.validate()?;
        // A program that asked for in-band size reports (mode 2048), such as
        // Neovim, waits for the report rather than SIGWINCH. The terminal
        // queues it on resize; send it now, not with the pane's next output,
        // which may never come.
        let responses = {
            // The terminal is held while the pty resizes, as Ghostty does,
            // so the redraw a SIGWINCH sets off is never read against the
            // old size.
            let mut terminal = self.terminal.lock();
            self.master.lock().resize(to_pty_size(size))?;
            terminal.resize(size)?;
            terminal.take_pty_responses()?
        };
        if !responses.is_empty() {
            self.write(&responses)?;
        }
        Ok(())
    }

    pub fn attachment(&self) -> Result<mux_protocol::PaneAttachment, PaneError> {
        let terminal = self.terminal.lock().attachment()?;
        terminal.validate_sequence_contract()?;
        // Asked apart: the exit status is held while the attachment is built.
        let program = self.running_program();
        let (title, command_endings) = {
            let scanner = self.title.lock();
            let endings = scanner.command_endings().iter().copied().collect();
            (scanner.title().map(str::to_owned), endings)
        };
        Ok(mux_protocol::PaneAttachment {
            pane_id: self.id,
            terminal,
            exit_status: *self.exit_status.lock(),
            title,
            program,
            command_endings,
        })
    }

    pub fn kill(&self) -> Result<(), PaneError> {
        self.killer.lock().kill()?;
        Ok(())
    }

    /// The name of the program in the terminal's foreground, when it isn't
    /// the pane's own shell: what closing the pane would cut off.
    #[must_use]
    pub fn running_program(&self) -> Option<String> {
        if self.exit_status.lock().is_some() {
            return None;
        }
        let foreground = foreground_process_id(&self.master)?;
        if Some(foreground) == self.process_id {
            return None;
        }
        Some(process_name(foreground))
    }

    /// What the pane shows, history first, as plain text.
    #[must_use]
    pub fn screen_text(&self) -> Option<String> {
        self.terminal.lock().screen_text()
    }

    /// Clear the pane, as Cmd+K does in a Mac terminal. At the shell's
    /// prompt the screen and its history are wiped and the shell draws its
    /// prompt again at the top. With a program in front, only the history
    /// goes: an editor or a pager keeps the screen it drew.
    pub fn clear(&self) -> Result<(), PaneError> {
        if self.exit_status.lock().is_some() {
            return Ok(());
        }
        let at_prompt = foreground_process_id(&self.master)
            .is_none_or(|foreground| Some(foreground) == self.process_id);
        let clear = if at_prompt {
            CLEAR_SCREEN_AND_HISTORY
        } else {
            CLEAR_HISTORY
        };
        if let Some(injector) = self.output_injector.lock().as_ref() {
            // Behind the output already read, so the clear lands in order
            // for this terminal and every attached client's.
            let _ = injector.send(clear.to_vec());
        }
        if at_prompt {
            self.write(REDRAW_PROMPT)?;
        }
        Ok(())
    }

    /// Put the terminal back as it started, for a program that left it
    /// garbled or stuck in a mode; a shell at its prompt draws it again.
    pub fn reset(&self) -> Result<(), PaneError> {
        if self.exit_status.lock().is_some() {
            return Ok(());
        }
        if let Some(injector) = self.output_injector.lock().as_ref() {
            let _ = injector.send(FULL_RESET.to_vec());
        }
        let at_prompt = foreground_process_id(&self.master)
            .is_none_or(|foreground| Some(foreground) == self.process_id);
        if at_prompt {
            self.write(REDRAW_PROMPT)?;
        }
        Ok(())
    }

    /// Look at what is in front of the shell, and say what has changed since
    /// the last look: a program the shell ran has taken the terminal, or has
    /// handed it back, and how long it ran. A pipeline or a program that
    /// starts others counts as the one it started with.
    pub fn look_at_foreground(&self) -> Option<ForegroundChange> {
        let shell = self.process_id?;
        let foreground = if self.exit_status.lock().is_some() {
            None
        } else {
            foreground_process_id(&self.master)
        };
        let mut command = self.foreground_command.lock();
        match foreground {
            Some(process_id) if process_id != shell => {
                if command.is_some() {
                    return None;
                }
                let name = process_name(process_id);
                let mut pending = self.shell_named_pending.lock();
                if name == process_name(shell) && *pending != Some(process_id) {
                    *pending = Some(process_id);
                    return None;
                }
                *pending = None;
                *command = Some(ForegroundCommand {
                    since: Instant::now(),
                    name: name.clone(),
                });
                Some(ForegroundChange::Started(name))
            }
            _ => command.take().map(|finished| {
                ForegroundChange::Finished(finished.name, finished.since.elapsed())
            }),
        }
    }

    /// Whether the terminal has started or stopped reading a password since
    /// the last look: the new answer, if it changed.
    pub fn look_at_secret_input(&self) -> Option<bool> {
        let secret = self.exit_status.lock().is_none() && reads_secret(&self.master);
        let mut was = self.secret_input.lock();
        (*was != secret).then(|| {
            *was = secret;
            secret
        })
    }

    /// Ask the operating system for the shell process's live working
    /// directory. This follows `cd` without parsing prompts or shell output.
    #[must_use]
    pub fn current_working_directory(&self) -> Option<std::path::PathBuf> {
        let process_id = self.process_id?;
        let pid = Pid::from_u32(process_id);
        let mut system = System::new();
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing().with_cwd(UpdateKind::Always),
        );
        system
            .process(pid)
            .and_then(|process| process.cwd())
            .filter(|cwd| cwd.is_dir())
            .map(std::path::Path::to_path_buf)
    }
}

#[cfg(feature = "ghostty")]
fn create_terminal_engine(
    size: TerminalSize,
    _replay_bytes: usize,
) -> Result<Box<dyn TerminalEngine>, PaneError> {
    static THEME: OnceLock<GhosttyTheme> = OnceLock::new();
    let theme = THEME.get_or_init(|| match GhosttyTheme::load_user() {
        Ok(theme) => theme,
        Err(error) => {
            warn!(%error, "could not load Ghostty colour configuration");
            GhosttyTheme::default()
        }
    });
    GhosttyEngine::new_with_theme(size, theme)
        .map(|engine| Box::new(engine) as Box<dyn TerminalEngine>)
        .map_err(|error| PaneError::Terminal(TerminalError::Engine(error.to_string())))
}

#[cfg(not(feature = "ghostty"))]
#[allow(clippy::unnecessary_wraps)]
fn create_terminal_engine(
    size: TerminalSize,
    replay_bytes: usize,
) -> Result<Box<dyn TerminalEngine>, PaneError> {
    Ok(Box::new(ReplayEngine::new(size, replay_bytes)))
}

// Keep PTY reads, batching, terminal mutation, response writes, and event
// publication together so their ordering remains explicit and single-owner.
fn spawn_reader(
    mut reader: Box<dyn Read + Send>,
    output: PaneOutputWorker,
) -> Result<OutputInjector, PaneError> {
    let (output_sender, output_receiver) = sync_channel::<Vec<u8>>(256);
    let injector: OutputInjector = Arc::new(Mutex::new(Some(output_sender.clone())));
    let closing = Arc::clone(&injector);
    let session_id = output.session_id;
    let pane_id = output.pane_id;
    thread::Builder::new()
        .name(format!("mux-pane-pty-read-{pane_id}"))
        .spawn(move || {
            let mut buffer = vec![0_u8; READ_BUFFER_SIZE];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(length) => {
                        if output_sender.send(buffer[..length].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(error) if error.raw_os_error() == Some(5) => {
                        // macOS and Linux PTY masters may report EIO after the slave exits.
                        break;
                    }
                    Err(error) => {
                        warn!(%session_id, %pane_id, %error, "pane reader stopped");
                        break;
                    }
                }
            }
            // The output stream ends with the PTY's.
            closing.lock().take();
        })?;

    thread::Builder::new()
        .name(format!("mux-pane-output-{pane_id}"))
        .spawn(move || output.run(&output_receiver))?;
    Ok(injector)
}

/// A process's name as the system knows it, or a stand-in when it has none.
fn process_name(process_id: u32) -> String {
    let pid = Pid::from_u32(process_id);
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing(),
    );
    system
        .process(pid)
        .map(|process| process.name().to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "a program".to_owned())
}

#[cfg(unix)]
fn foreground_process_id(master: &Mutex<Box<dyn MasterPty + Send>>) -> Option<u32> {
    master
        .lock()
        .process_group_leader()
        .and_then(|process_id| u32::try_from(process_id).ok())
}

/// Whether the terminal reads whole lines without showing them, as `sudo`
/// and `ssh` do while they read a password. A shell at its prompt, or an
/// editor, reads key by key, so is never taken for one.
#[cfg(unix)]
fn reads_secret(master: &Mutex<Box<dyn MasterPty + Send>>) -> bool {
    use nix::sys::termios::LocalFlags;
    master.lock().get_termios().is_some_and(|termios| {
        let flags = termios.local_flags;
        flags.contains(LocalFlags::ICANON) && !flags.contains(LocalFlags::ECHO)
    })
}

#[cfg(not(unix))]
fn reads_secret(_: &Mutex<Box<dyn MasterPty + Send>>) -> bool {
    false
}

#[cfg(not(unix))]
fn foreground_process_id(_: &Mutex<Box<dyn MasterPty + Send>>) -> Option<u32> {
    None
}

fn to_pty_size(size: TerminalSize) -> PtySize {
    let pixel_width = u32::from(size.cols).saturating_mul(size.cell_width_px);
    let pixel_height = u32::from(size.rows).saturating_mul(size.cell_height_px);
    PtySize {
        rows: size.rows,
        cols: size.cols,
        pixel_width: u16::try_from(pixel_width).unwrap_or(u16::MAX),
        pixel_height: u16::try_from(pixel_height).unwrap_or(u16::MAX),
    }
}

#[derive(Debug, Error)]
pub enum PaneError {
    #[error("PTY operation failed: {0}")]
    Pty(#[from] anyhow::Error),
    #[error("pane I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("terminal engine failed: {0}")]
    Terminal(#[from] TerminalError),
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::sync_channel;
    use std::thread;
    use std::time::{Duration, Instant};

    use super::{
        ForegroundProcessTracker, HISTORY_RULE, OUTPUT_BATCH_SIZE, collect_output_batch,
        history_output, names_another_terminal,
    };

    #[test]
    fn history_comes_back_as_plain_lines_under_a_rule() {
        let output = history_output("$ ls\nsrc  \x1b[31mtarget\n\n\n", HISTORY_RULE);
        let output = String::from_utf8(output).expect("text");
        assert_eq!(
            output,
            "$ ls\r\nsrc  [31mtarget\r\n\x1b[2m── before Mux restarted ──\x1b[0m\r\n"
        );
    }

    #[test]
    fn the_terminal_mux_was_started_from_is_not_passed_on() {
        for key in [
            "GHOSTTY_RESOURCES_DIR",
            "ITERM_SESSION_ID",
            "TERMINFO",
            "WT_SESSION",
        ] {
            assert!(names_another_terminal(key), "{key}");
        }
        for key in ["PATH", "TERM", "HOME", "TERM_PROGRAM", "GHOSTTY"] {
            assert!(!names_another_terminal(key), "{key}");
        }
    }

    #[test]
    fn queued_output_joins_the_batch_without_a_wait() {
        let (sender, receiver) = sync_channel(8);
        sender.send(b"b".to_vec()).unwrap();
        sender.send(b"c".to_vec()).unwrap();

        let started = Instant::now();
        let (bytes, disconnected) = collect_output_batch(b"a".to_vec(), &receiver, None);

        assert_eq!(bytes, b"abc");
        assert!(!disconnected);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_stream_waits_for_output_that_follows() {
        let (sender, receiver) = sync_channel(8);
        let writer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            sender.send(b"b".to_vec()).unwrap();
        });

        let (bytes, disconnected) =
            collect_output_batch(b"a".to_vec(), &receiver, Some(Duration::from_secs(10)));

        writer.join().unwrap();
        assert_eq!(bytes, b"ab");
        assert!(disconnected);
    }

    #[test]
    fn a_batch_stops_growing_once_it_is_full() {
        let (sender, receiver) = sync_channel(8);
        sender.send(vec![b'b'; OUTPUT_BATCH_SIZE]).unwrap();
        sender.send(b"c".to_vec()).unwrap();

        let (bytes, _) = collect_output_batch(b"a".to_vec(), &receiver, None);

        assert_eq!(bytes.len(), OUTPUT_BATCH_SIZE + 1);
        assert_eq!(receiver.try_recv().unwrap(), b"c");
    }

    #[test]
    fn foreground_tracker_reports_a_child_to_shell_transition_once() {
        let mut tracker = ForegroundProcessTracker::new(Some(41));

        assert!(!tracker.observe(Some(41)));
        assert!(!tracker.observe(Some(72)));
        assert!(!tracker.observe(Some(72)));
        assert!(tracker.observe(Some(41)));
        assert!(!tracker.observe(Some(41)));
    }

    #[test]
    fn foreground_tracker_ignores_unknown_and_untracked_process_groups() {
        let mut unavailable = ForegroundProcessTracker::new(None);
        assert!(!unavailable.observe(Some(72)));
        assert!(!unavailable.observe(None));

        let mut tracker = ForegroundProcessTracker::new(Some(41));
        assert!(!tracker.observe(None));
        assert!(!tracker.observe(Some(41)));
    }
}
