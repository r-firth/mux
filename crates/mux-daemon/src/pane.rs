use std::io::{Read, Write};
use std::sync::Arc;
#[cfg(feature = "ghostty")]
use std::sync::OnceLock;
use std::sync::mpsc::{Receiver, RecvTimeoutError, sync_channel};
use std::thread;
use std::time::Duration;

use mux_protocol::{ProcessExit, ServerEvent, SpawnCommand};
#[cfg(not(feature = "ghostty"))]
use mux_terminal::ReplayEngine;
use mux_terminal::{
    KITTY_KEYBOARD_REPORT_EVENTS, KITTY_KEYBOARD_RESET_SEQUENCE, TerminalEngine, TerminalError,
    TerminalSize,
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
    events: broadcast::Sender<ServerEvent>,
}

impl PaneOutputWorker {
    fn run(self, output_receiver: &Receiver<Vec<u8>>) {
        let mut foreground = ForegroundProcessTracker::new(self.shell_process_id);
        while let Ok(first) = output_receiver.recv() {
            let (mut bytes, disconnected) = collect_output_batch(first, output_receiver);
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

fn collect_output_batch(first: Vec<u8>, receiver: &Receiver<Vec<u8>>) -> (Vec<u8>, bool) {
    let mut bytes = first;
    while bytes.len() < OUTPUT_BATCH_SIZE {
        match receiver.recv_timeout(OUTPUT_BATCH_IDLE) {
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
        let size = size.validate()?;
        let pair = native_pty_system().openpty(to_pty_size(size))?;

        let mut builder = CommandBuilder::new(&command.program);
        builder.args(&command.args);
        builder.cwd(cwd);
        builder.env("TERM", "xterm-256color");
        builder.env("COLORTERM", "truecolor");
        for (key, value) in &command.environment {
            builder.env(key, value);
        }

        let mut child = pair.slave.spawn_command(builder)?;
        let process_id = child.process_id();
        let killer = child.clone_killer();
        let reader = pair.master.try_clone_reader()?;
        let writer = Arc::new(Mutex::new(pair.master.take_writer()?));
        drop(pair.slave);

        let terminal: Arc<Mutex<Box<dyn TerminalEngine>>> =
            Arc::new(Mutex::new(create_terminal_engine(size, replay_bytes)?));
        let exit_status = Arc::new(Mutex::new(None));
        let master = Arc::new(Mutex::new(pair.master));
        spawn_reader(
            reader,
            PaneOutputWorker {
                session_id,
                pane_id: id,
                terminal: Arc::clone(&terminal),
                response_writer: Arc::clone(&writer),
                master: Arc::clone(&master),
                shell_process_id: process_id,
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
        self.master.lock().resize(to_pty_size(size))?;
        self.terminal.lock().resize(size)?;
        Ok(())
    }

    pub fn attachment(&self) -> Result<mux_protocol::PaneAttachment, PaneError> {
        let terminal = self.terminal.lock().attachment()?;
        terminal.validate_sequence_contract()?;
        Ok(mux_protocol::PaneAttachment {
            pane_id: self.id,
            terminal,
            exit_status: *self.exit_status.lock(),
        })
    }

    pub fn kill(&self) -> Result<(), PaneError> {
        self.killer.lock().kill()?;
        Ok(())
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
) -> Result<(), PaneError> {
    let (output_sender, output_receiver) = sync_channel::<Vec<u8>>(256);
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
        })?;

    thread::Builder::new()
        .name(format!("mux-pane-output-{pane_id}"))
        .spawn(move || output.run(&output_receiver))?;
    Ok(())
}

#[cfg(unix)]
fn foreground_process_id(master: &Mutex<Box<dyn MasterPty + Send>>) -> Option<u32> {
    master
        .lock()
        .process_group_leader()
        .and_then(|process_id| u32::try_from(process_id).ok())
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
    use super::ForegroundProcessTracker;

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
