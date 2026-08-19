use std::io::{Read, Write};
use std::sync::Arc;
#[cfg(feature = "ghostty")]
use std::sync::OnceLock;
use std::sync::mpsc::{RecvTimeoutError, sync_channel};
use std::thread;
use std::time::Duration;

use mux_protocol::{ProcessExit, ServerEvent, SpawnCommand};
#[cfg(not(feature = "ghostty"))]
use mux_terminal::ReplayEngine;
use mux_terminal::{TerminalEngine, TerminalError, TerminalSize};
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

pub struct PaneRuntime {
    pub id: PaneId,
    master: Mutex<Box<dyn MasterPty + Send>>,
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
        spawn_reader(
            session_id,
            id,
            reader,
            Arc::clone(&terminal),
            Arc::clone(&writer),
            events.clone(),
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
            master: Mutex::new(pair.master),
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
#[allow(clippy::cognitive_complexity)]
fn spawn_reader(
    session_id: SessionId,
    pane_id: PaneId,
    mut reader: Box<dyn Read + Send>,
    terminal: Arc<Mutex<Box<dyn TerminalEngine>>>,
    response_writer: Arc<Mutex<Box<dyn Write + Send>>>,
    events: broadcast::Sender<ServerEvent>,
) -> Result<(), PaneError> {
    let (output_sender, output_receiver) = sync_channel::<Vec<u8>>(256);
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
        .spawn(move || {
            while let Ok(first) = output_receiver.recv() {
                let mut bytes = first;
                let mut disconnected = false;
                while bytes.len() < OUTPUT_BATCH_SIZE {
                    match output_receiver.recv_timeout(OUTPUT_BATCH_IDLE) {
                        Ok(next) => bytes.extend_from_slice(&next),
                        Err(RecvTimeoutError::Timeout) => break,
                        Err(RecvTimeoutError::Disconnected) => {
                            disconnected = true;
                            break;
                        }
                    }
                }

                let (output_sequence, responses) = {
                    let mut terminal = terminal.lock();
                    let output_sequence = terminal.next_output_sequence();
                    if let Err(error) = terminal.apply_output(output_sequence, &bytes) {
                        error!(%session_id, %pane_id, %error, "terminal engine rejected pane output");
                        break;
                    }
                    match terminal.take_pty_responses() {
                        Ok(responses) => (output_sequence, responses),
                        Err(error) => {
                            error!(%session_id, %pane_id, %error, "terminal engine failed to generate PTY responses");
                            break;
                        }
                    }
                };
                if !responses.is_empty() {
                    let mut writer = response_writer.lock();
                    if writer
                        .write_all(&responses)
                        .and_then(|()| writer.flush())
                        .is_err()
                    {
                        warn!(%session_id, %pane_id, "failed writing terminal response to PTY");
                        break;
                    }
                }
                let _ = events.send(ServerEvent::PaneOutput {
                    session_id,
                    pane_id,
                    sequence: output_sequence,
                    bytes,
                });
                if disconnected {
                    break;
                }
            }
        })?;
    Ok(())
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
