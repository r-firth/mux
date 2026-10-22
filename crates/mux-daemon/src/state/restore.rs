//! The workspace kept on disk, so a daemon that starts again, after a
//! restart of the Mac or an upgrade that replaced the old one, brings back
//! the sessions it had: their tabs and splits, names and colours, each pane
//! in the directory it was last in. The programs that were running and what
//! the panes showed are gone with the old daemon; each pane starts a new
//! shell.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufWriter, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mux_protocol::SpawnCommand;
use mux_terminal::TerminalSize;
use mux_workspace::{PaneId, Session};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tracing::{info, warn};

use super::{DaemonState, SessionRuntime, SpawnTemplate};
use crate::pane::PaneRuntime;

const WORKSPACE_FILE: &str = "workspace.json";
const WORKSPACE_VERSION: u32 = 1;

#[derive(Debug, Deserialize, Serialize)]
struct SavedWorkspace {
    version: u32,
    sessions: Vec<SavedSession>,
}

#[derive(Debug, Deserialize, Serialize)]
struct SavedSession {
    model: Session,
    /// Where a pane with no directory of its own starts, and how.
    cwd: PathBuf,
    command: SpawnCommand,
    size: TerminalSize,
    /// The directory each pane's shell was last in.
    pane_cwds: HashMap<PaneId, PathBuf>,
}

impl DaemonState {
    /// Write the workspace down if it has changed since it was last written.
    pub fn save_workspace(&self) {
        let saved = SavedWorkspace {
            version: WORKSPACE_VERSION,
            sessions: self.saved_sessions(),
        };
        let Ok(text) = serde_json::to_string_pretty(&saved) else {
            return;
        };
        let mut last = self.saved_workspace.lock();
        if *last == text {
            return;
        }
        match write_private(&self.state_dir, WORKSPACE_FILE, &text) {
            Ok(()) => *last = text,
            Err(error) => warn!(%error, "could not save the workspace"),
        }
    }

    fn saved_sessions(&self) -> Vec<SavedSession> {
        let mut sessions = self
            .sessions
            .read()
            .values()
            .map(|session| {
                let pane_cwds = session
                    .panes
                    .read()
                    .iter()
                    .filter_map(|(pane_id, pane)| {
                        Some((*pane_id, pane.current_working_directory()?))
                    })
                    .collect();
                SavedSession {
                    model: session.model.read().clone(),
                    cwd: session.spawn.cwd.clone(),
                    command: session.spawn.command.clone(),
                    size: session.spawn.size,
                    pane_cwds,
                }
            })
            .collect::<Vec<_>>();
        sessions.sort_by(|left, right| left.model.name.cmp(&right.model.name));
        sessions
    }

    /// Bring back the sessions written down by the daemon before this one.
    /// A session that can't be brought back is left out; the rest come back.
    pub fn restore_workspace(&self) {
        let Some((saved, text)) = read_saved_workspace(&self.state_dir.join(WORKSPACE_FILE)) else {
            return;
        };
        for session in saved.sessions {
            let name = session.model.name.clone();
            match self.restore_session(session) {
                Ok(panes) => info!(%name, panes, "restored session"),
                Err(error) => warn!(%name, %error, "could not restore session"),
            }
        }
        *self.saved_workspace.lock() = text;
    }

    fn restore_session(&self, saved: SavedSession) -> Result<usize, crate::pane::PaneError> {
        let mut pane_ids = Vec::new();
        for tab in &saved.model.tabs {
            tab.layout.pane_ids(&mut pane_ids);
        }
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let session_cwd = [Some(saved.cwd.clone()), home]
            .into_iter()
            .flatten()
            .find(|cwd| cwd.is_dir())
            .unwrap_or_else(|| PathBuf::from("/"));
        let (events, _) = broadcast::channel(2_048);
        let mut panes = HashMap::new();
        for pane_id in pane_ids {
            let cwd = saved
                .pane_cwds
                .get(&pane_id)
                .filter(|cwd| cwd.is_dir())
                .unwrap_or(&session_cwd);
            let pane = PaneRuntime::spawn(
                saved.model.id,
                pane_id,
                cwd,
                &saved.command,
                saved.size,
                self.replay_bytes_per_pane,
                events.clone(),
            );
            match pane {
                Ok(pane) => {
                    panes.insert(pane_id, pane);
                }
                Err(error) => {
                    for pane in panes.into_values() {
                        let _ = pane.kill();
                    }
                    return Err(error);
                }
            }
        }
        let count = panes.len();
        let id = saved.model.id;
        let runtime = Arc::new(SessionRuntime {
            model: RwLock::new(saved.model),
            panes: RwLock::new(panes),
            events,
            spawn: SpawnTemplate {
                cwd: session_cwd,
                command: saved.command,
                size: saved.size,
            },
            mutation: Mutex::new(()),
            closed_tabs: Mutex::new(Vec::new()),
        });
        self.sessions.write().insert(id, runtime);
        Ok(count)
    }
}

/// The workspace written down at `path`, and its text, if there is one
/// this daemon can read.
fn read_saved_workspace(path: &Path) -> Option<(SavedWorkspace, String)> {
    let text = fs::read_to_string(path).ok()?;
    match serde_json::from_str::<SavedWorkspace>(&text) {
        Ok(saved) if saved.version == WORKSPACE_VERSION => Some((saved, text)),
        Ok(saved) => {
            warn!(
                version = saved.version,
                "saved workspace is from another version"
            );
            None
        }
        Err(error) => {
            warn!(%error, "saved workspace could not be read");
            None
        }
    }
}

/// Write `text` to `name` in `directory`, readable only by its owner, by way
/// of a file beside it so a crash never leaves half of it.
fn write_private(directory: &Path, name: &str, text: &str) -> std::io::Result<()> {
    fs::create_dir_all(directory)?;
    let temporary = directory.join(format!("{name}.tmp-{}", std::process::id()));
    let file = File::create(&temporary)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    let mut writer = BufWriter::new(file);
    writer.write_all(text.as_bytes())?;
    writer.flush()?;
    writer.get_ref().sync_all()?;
    fs::rename(temporary, directory.join(name))
}

#[cfg(all(test, unix))]
mod tests {
    use std::time::{Duration, Instant};

    use mux_protocol::{CreateSession, SessionSelector};
    use mux_workspace::{SplitAxis, WorkspaceCommand};

    use super::*;

    #[test]
    fn a_new_daemon_brings_back_the_workspace_of_the_last() {
        let state_directory = tempfile::tempdir().expect("state directory");
        let start = tempfile::tempdir().expect("start directory");
        let elsewhere = start.path().join("elsewhere");
        fs::create_dir(&elsewhere).expect("elsewhere");

        let before = DaemonState::new(state_directory.path().to_path_buf(), 64 * 1024);
        let summary = before
            .create_session(CreateSession {
                name: "work".to_owned(),
                cwd: start.path().to_path_buf(),
                command: SpawnCommand {
                    program: PathBuf::from("/bin/sh"),
                    args: Vec::new(),
                    environment: Vec::new(),
                },
                initial_panes: 1,
                initial_size: TerminalSize::default(),
            })
            .expect("create session");
        for command in [
            WorkspaceCommand::SplitPane(SplitAxis::Horizontal),
            WorkspaceCommand::RenameTab("build".to_owned()),
            WorkspaceCommand::SetTabInk(2),
            WorkspaceCommand::NewTab,
        ] {
            before
                .workspace_command(summary.id, command)
                .expect("workspace command");
        }
        let runtime = before
            .resolve(&SessionSelector::Id(summary.id))
            .expect("session");
        let model = runtime.model.read().clone();
        let moved = model.tabs[0].focused_pane;
        let pane = runtime.panes.read()[&moved].clone();
        pane.write(format!("cd '{}'\n", elsewhere.display()).as_bytes())
            .expect("cd");
        let deadline = Instant::now() + Duration::from_secs(3);
        while pane
            .current_working_directory()
            .and_then(|cwd| cwd.canonicalize().ok())
            != elsewhere.canonicalize().ok()
        {
            assert!(
                Instant::now() < deadline,
                "the shell never changed directory"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        before.save_workspace();
        drop(pane);
        drop(runtime);
        for session in before.sessions.write().drain() {
            session.1.kill();
        }

        let after = DaemonState::new(state_directory.path().to_path_buf(), 64 * 1024);
        after.restore_workspace();
        let restored = after
            .resolve(&SessionSelector::Name("work".to_owned()))
            .expect("restored session");
        assert_eq!(
            *restored.model.read(),
            model,
            "tabs, splits, names and inks"
        );
        assert_eq!(restored.panes.read().len(), 3, "a shell in every pane");
        let cwd = restored
            .cwd_for_pane(moved)
            .and_then(|cwd| cwd.canonicalize().ok());
        assert_eq!(cwd, elsewhere.canonicalize().ok(), "back where it was");
        restored.kill();
    }
}
