use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufWriter, Write as _};
use std::path::PathBuf;
use std::sync::Arc;

use mux_acp::{
    AgentConfigValueSelection, AgentEvent, AgentManager, AgentPrompt, AgentSessionSnapshot,
    AgentSpec,
};
use mux_protocol::{
    CreateSession, ErrorCode, RemoteError, ServerEvent, SessionAttachment, SessionSelector,
    SessionSummary, SpawnCommand,
};
use mux_terminal::TerminalSize;
use mux_workspace::{AgentSessionId, PaneId, Session, SessionId, WorkspaceCommand};
use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use tokio::sync::broadcast;

use crate::pane::PaneRuntime;

pub struct DaemonState {
    state_dir: PathBuf,
    replay_bytes_per_pane: usize,
    sessions: RwLock<HashMap<SessionId, Arc<SessionRuntime>>>,
    agents: AgentManager,
}

impl DaemonState {
    pub fn new(state_dir: PathBuf, replay_bytes_per_pane: usize) -> Self {
        Self {
            state_dir,
            replay_bytes_per_pane,
            sessions: RwLock::new(HashMap::new()),
            agents: AgentManager::new(),
        }
    }

    pub fn list_sessions(&self) -> Vec<SessionSummary> {
        let mut sessions: Vec<_> = self
            .sessions
            .read()
            .values()
            .map(|session| session.summary())
            .collect();
        sessions.sort_by(|left, right| left.name.cmp(&right.name));
        sessions
    }

    #[must_use]
    pub fn subscribe_agent_events(&self) -> broadcast::Receiver<AgentEvent> {
        self.agents.subscribe()
    }

    #[must_use]
    pub fn list_agents(&self) -> Vec<AgentSessionSnapshot> {
        self.agents.list()
    }

    pub fn start_agent(
        &self,
        spec: &AgentSpec,
        cwd: PathBuf,
    ) -> Result<AgentSessionSnapshot, RemoteError> {
        self.agents
            .start(spec, cwd)
            .map_err(|error| agent_error(&error))
    }

    pub fn start_agent_for_pane(
        &self,
        spec: &AgentSpec,
        pane_id: PaneId,
    ) -> Result<AgentSessionSnapshot, RemoteError> {
        let cwd = self
            .sessions
            .read()
            .values()
            .find_map(|session| session.cwd_for_pane(pane_id))
            .ok_or_else(|| {
                RemoteError::new(ErrorCode::NotFound, format!("pane not found: {pane_id}"))
            })?;
        self.start_agent(spec, cwd)
    }

    pub fn prompt_agent(
        &self,
        session_id: AgentSessionId,
        prompt: AgentPrompt,
    ) -> Result<(), RemoteError> {
        self.agents
            .prompt(session_id, prompt)
            .map_err(|error| agent_error(&error))
    }

    pub fn authenticate_agent(
        &self,
        session_id: AgentSessionId,
        method_id: String,
    ) -> Result<(), RemoteError> {
        self.agents
            .authenticate(session_id, method_id)
            .map_err(|error| agent_error(&error))
    }

    pub fn set_agent_mode(
        &self,
        session_id: AgentSessionId,
        mode_id: String,
    ) -> Result<(), RemoteError> {
        self.agents
            .set_mode(session_id, mode_id)
            .map_err(|error| agent_error(&error))
    }

    pub fn set_agent_config(
        &self,
        session_id: AgentSessionId,
        config_id: String,
        value: AgentConfigValueSelection,
    ) -> Result<(), RemoteError> {
        self.agents
            .set_config(session_id, config_id, value)
            .map_err(|error| agent_error(&error))
    }

    pub fn resolve_agent_permission(
        &self,
        session_id: AgentSessionId,
        request_id: String,
        option_id: Option<String>,
    ) -> Result<(), RemoteError> {
        self.agents
            .resolve_permission(session_id, request_id, option_id)
            .map_err(|error| agent_error(&error))
    }

    pub fn cancel_agent(&self, session_id: AgentSessionId) -> Result<(), RemoteError> {
        self.agents
            .cancel(session_id)
            .map_err(|error| agent_error(&error))
    }

    pub fn close_agent(&self, session_id: AgentSessionId) -> Result<(), RemoteError> {
        self.agents
            .close(session_id)
            .map_err(|error| agent_error(&error))
    }

    pub fn create_session(&self, request: CreateSession) -> Result<SessionSummary, RemoteError> {
        if request.name.trim().is_empty() {
            return Err(RemoteError::new(
                ErrorCode::InvalidRequest,
                "session name cannot be empty",
            ));
        }
        if request.initial_panes == 0 || request.initial_panes > 32 {
            return Err(RemoteError::new(
                ErrorCode::InvalidRequest,
                "initial pane count must be between 1 and 32",
            ));
        }
        if !request.cwd.is_dir() {
            return Err(RemoteError::new(
                ErrorCode::InvalidRequest,
                format!(
                    "working directory does not exist: {}",
                    request.cwd.display()
                ),
            ));
        }
        request.initial_size.validate().map_err(internal_error)?;

        let mut sessions = self.sessions.write();
        if sessions
            .values()
            .any(|session| session.model.read().name == request.name)
        {
            return Err(RemoteError::new(
                ErrorCode::Conflict,
                format!("session already exists: {}", request.name),
            ));
        }

        let pane_ids: Vec<_> = (0..request.initial_panes).map(|_| PaneId::new()).collect();
        let model = Session::with_panes(request.name, &pane_ids).map_err(internal_error)?;
        let (events, _) = broadcast::channel(2_048);
        let mut panes = HashMap::new();
        for pane_id in pane_ids {
            let pane = PaneRuntime::spawn(
                model.id,
                pane_id,
                &request.cwd,
                &request.command,
                request.initial_size,
                self.replay_bytes_per_pane,
                events.clone(),
            )
            .map_err(internal_error)?;
            panes.insert(pane_id, pane);
        }

        let runtime = Arc::new(SessionRuntime {
            model: RwLock::new(model),
            panes: RwLock::new(panes),
            events,
            spawn: SpawnTemplate {
                cwd: request.cwd,
                command: request.command,
                size: request.initial_size,
            },
            mutation: Mutex::new(()),
        });
        let summary = runtime.summary();
        sessions.insert(summary.id, runtime);
        drop(sessions);
        self.persist_metadata().map_err(internal_error)?;
        Ok(summary)
    }

    pub fn create_session_for_pane(
        &self,
        name: String,
        pane_id: PaneId,
    ) -> Result<SessionSummary, RemoteError> {
        let source = self
            .sessions
            .read()
            .values()
            .find(|session| session.panes.read().contains_key(&pane_id))
            .cloned()
            .ok_or_else(|| {
                RemoteError::new(ErrorCode::NotFound, format!("pane not found: {pane_id}"))
            })?;
        let cwd = source
            .cwd_for_pane(pane_id)
            .unwrap_or_else(|| source.spawn.cwd.clone());
        self.create_session(CreateSession {
            name,
            cwd,
            command: source.spawn.command.clone(),
            initial_panes: 1,
            initial_size: source.spawn.size,
        })
    }

    pub fn rename_session(&self, session_id: SessionId, name: &str) -> Result<(), RemoteError> {
        let name = validated_name("session", name).map_err(internal_error)?;
        let sessions = self.sessions.read();
        if sessions.values().any(|session| {
            let model = session.model.read();
            model.id != session_id && model.name == name
        }) {
            return Err(RemoteError::new(
                ErrorCode::Conflict,
                format!("session already exists: {name}"),
            ));
        }
        let session = sessions.get(&session_id).cloned().ok_or_else(|| {
            RemoteError::new(
                ErrorCode::NotFound,
                format!("session not found: {session_id}"),
            )
        })?;
        drop(sessions);
        session.model.write().name = name;
        let _ = session
            .events
            .send(ServerEvent::WorkspaceChanged { session_id });
        self.persist_metadata().map_err(internal_error)
    }

    pub fn kill_session(&self, session_id: SessionId) -> Result<(), RemoteError> {
        let session = self.sessions.write().remove(&session_id).ok_or_else(|| {
            RemoteError::new(
                ErrorCode::NotFound,
                format!("session not found: {session_id}"),
            )
        })?;
        session.kill();
        self.persist_metadata().map_err(internal_error)
    }

    pub fn prepare_attach(
        &self,
        selector: &SessionSelector,
    ) -> Result<(SessionAttachment, broadcast::Receiver<ServerEvent>), RemoteError> {
        let session = self.resolve(selector)?;
        // Subscribe before taking the snapshot. Sequence cursors let clients
        // discard duplicates that race with snapshot creation.
        let receiver = session.events.subscribe();
        let attachment = session.attachment().map_err(internal_error)?;
        Ok((attachment, receiver))
    }

    pub fn write_input(&self, pane_id: PaneId, bytes: &[u8]) -> Result<(), RemoteError> {
        let pane = self.find_pane(pane_id)?;
        pane.write(bytes).map_err(internal_error)
    }

    pub fn resize_pane(
        &self,
        pane_id: PaneId,
        size: mux_terminal::TerminalSize,
    ) -> Result<(), RemoteError> {
        let pane = self.find_pane(pane_id)?;
        pane.resize(size).map_err(internal_error)
    }

    pub fn workspace_command(
        &self,
        session_id: SessionId,
        command: WorkspaceCommand,
    ) -> Result<SessionAttachment, RemoteError> {
        if let WorkspaceCommand::RenameSession(name) = &command {
            self.rename_session(session_id, name)?;
            return self
                .resolve(&SessionSelector::Id(session_id))?
                .attachment()
                .map_err(internal_error);
        }
        let session = self.resolve(&SessionSelector::Id(session_id))?;
        let attachment = session
            .apply_command(command, self.replay_bytes_per_pane)
            .map_err(internal_error)?;
        let _ = session
            .events
            .send(ServerEvent::WorkspaceChanged { session_id });
        self.persist_metadata().map_err(internal_error)?;
        Ok(attachment)
    }

    fn resolve(&self, selector: &SessionSelector) -> Result<Arc<SessionRuntime>, RemoteError> {
        let sessions = self.sessions.read();
        let session = match selector {
            SessionSelector::Id(id) => sessions.get(id),
            SessionSelector::Name(name) => sessions
                .values()
                .find(|session| session.model.read().name == *name),
        };
        session.cloned().ok_or_else(|| {
            RemoteError::new(
                ErrorCode::NotFound,
                format!("session not found: {selector:?}"),
            )
        })
    }

    fn find_pane(&self, pane_id: PaneId) -> Result<Arc<PaneRuntime>, RemoteError> {
        self.sessions
            .read()
            .values()
            .find_map(|session| session.panes.read().get(&pane_id).cloned())
            .ok_or_else(|| {
                RemoteError::new(ErrorCode::NotFound, format!("pane not found: {pane_id}"))
            })
    }

    fn persist_metadata(&self) -> Result<(), std::io::Error> {
        let state = PersistedDaemonState {
            daemon_pid: std::process::id(),
            sessions: self.list_sessions(),
        };
        fs::create_dir_all(&self.state_dir)?;
        let destination = self.state_dir.join("state.json");
        let temporary = self
            .state_dir
            .join(format!("state.json.tmp-{}", std::process::id()));
        let file = File::create(&temporary)?;
        let mut writer = BufWriter::new(file);
        serde_json::to_writer_pretty(&mut writer, &state).map_err(std::io::Error::other)?;
        writer.flush()?;
        writer.get_ref().sync_all()?;
        fs::rename(temporary, destination)?;
        Ok(())
    }
}

struct SessionRuntime {
    model: RwLock<Session>,
    panes: RwLock<HashMap<PaneId, Arc<PaneRuntime>>>,
    events: broadcast::Sender<ServerEvent>,
    spawn: SpawnTemplate,
    mutation: Mutex<()>,
}

impl SessionRuntime {
    fn summary(&self) -> SessionSummary {
        let model = self.model.read();
        SessionSummary {
            id: model.id,
            name: model.name.clone(),
            pane_count: self.panes.read().len(),
        }
    }

    fn attachment(&self) -> Result<SessionAttachment, crate::pane::PaneError> {
        let model = self.model.read();
        let panes = self.panes.read();
        let mut pane_ids = Vec::new();
        for tab in &model.tabs {
            tab.layout.pane_ids(&mut pane_ids);
        }
        let attachments = pane_ids
            .into_iter()
            .filter_map(|pane_id| panes.get(&pane_id))
            .map(|pane| pane.attachment())
            .collect::<Result<Vec<_>, _>>()?;
        Ok(SessionAttachment {
            session: model.clone(),
            panes: attachments,
        })
    }

    fn apply_command(
        &self,
        command: WorkspaceCommand,
        replay_bytes: usize,
    ) -> Result<SessionAttachment, SessionMutationError> {
        let _mutation = self.mutation.lock();
        match command {
            WorkspaceCommand::SplitPane(axis) => {
                let session_id = self.model.read().id;
                let pane_id = PaneId::new();
                let pane = self.spawn_pane_in_focused_cwd(session_id, pane_id, replay_bytes)?;
                let result = self
                    .model
                    .write()
                    .active_tab_mut()
                    .ok_or(SessionMutationError::NoActiveTab)?
                    .split_focused(pane_id, axis);
                if let Err(error) = result {
                    let _ = pane.kill();
                    return Err(error.into());
                }
                self.panes.write().insert(pane_id, pane);
            }
            WorkspaceCommand::FocusPane(direction) => {
                self.model
                    .write()
                    .active_tab_mut()
                    .ok_or(SessionMutationError::NoActiveTab)?
                    .focus_neighbor(direction)?;
            }
            WorkspaceCommand::FocusPaneOrTab(direction) => {
                self.model.write().move_focus_or_tab(direction)?;
            }
            WorkspaceCommand::ResizePane(direction) => {
                let mut model = self.model.write();
                let tab = model
                    .active_tab_mut()
                    .ok_or(SessionMutationError::NoActiveTab)?;
                let focused = tab.focused_pane;
                tab.layout.resize_toward(focused, direction)?;
            }
            WorkspaceCommand::SetFocusedPane(pane_id) => {
                self.model
                    .write()
                    .active_tab_mut()
                    .ok_or(SessionMutationError::NoActiveTab)?
                    .focus(pane_id)?;
            }
            WorkspaceCommand::ClosePane => {
                let pane_id = self
                    .model
                    .write()
                    .active_tab_mut()
                    .ok_or(SessionMutationError::NoActiveTab)?
                    .close_focused()?;
                self.remove_panes(&[pane_id]);
            }
            WorkspaceCommand::TogglePaneZoom => {
                self.model
                    .write()
                    .active_tab_mut()
                    .ok_or(SessionMutationError::NoActiveTab)?
                    .toggle_zoom();
            }
            WorkspaceCommand::NewTab => {
                let session_id = self.model.read().id;
                let pane_id = PaneId::new();
                let pane = self.spawn_pane_in_focused_cwd(session_id, pane_id, replay_bytes)?;
                if let Err(error) = self.model.write().add_tab(pane_id) {
                    let _ = pane.kill();
                    return Err(error.into());
                }
                self.panes.write().insert(pane_id, pane);
            }
            WorkspaceCommand::CloseTab => {
                let pane_ids = self.model.write().close_active_tab()?;
                self.remove_panes(&pane_ids);
            }
            WorkspaceCommand::RenameTab(title) => {
                let title = validated_name("tab", &title)?;
                self.model
                    .write()
                    .active_tab_mut()
                    .ok_or(SessionMutationError::NoActiveTab)?
                    .title = title;
            }
            WorkspaceCommand::SelectTab(tab_id) => self.model.write().select_tab(tab_id)?,
            WorkspaceCommand::NextTab => self.model.write().cycle_tab(1)?,
            WorkspaceCommand::PreviousTab => self.model.write().cycle_tab(-1)?,
            WorkspaceCommand::RenameSession(name) => {
                self.model.write().name = validated_name("session", &name)?;
            }
        }
        Ok(self.attachment()?)
    }

    fn spawn_pane_in_focused_cwd(
        &self,
        session_id: SessionId,
        pane_id: PaneId,
        replay_bytes: usize,
    ) -> Result<Arc<PaneRuntime>, crate::pane::PaneError> {
        let cwd = self
            .focused_pane_cwd()
            .unwrap_or_else(|| self.spawn.cwd.clone());
        PaneRuntime::spawn(
            session_id,
            pane_id,
            &cwd,
            &self.spawn.command,
            self.spawn.size,
            replay_bytes,
            self.events.clone(),
        )
    }

    fn focused_pane_cwd(&self) -> Option<PathBuf> {
        let focused = self.model.read().active_tab()?.focused_pane;
        self.cwd_for_pane(focused)
    }

    fn cwd_for_pane(&self, pane_id: PaneId) -> Option<PathBuf> {
        self.panes.read().get(&pane_id).map(|pane| {
            pane.current_working_directory()
                .unwrap_or_else(|| self.spawn.cwd.clone())
        })
    }

    fn remove_panes(&self, pane_ids: &[PaneId]) {
        let mut panes = self.panes.write();
        for pane_id in pane_ids {
            if let Some(pane) = panes.remove(pane_id) {
                let _ = pane.kill();
            }
        }
    }

    fn kill(&self) {
        let panes = std::mem::take(&mut *self.panes.write());
        for pane in panes.into_values() {
            let _ = pane.kill();
        }
    }
}

#[derive(Clone)]
struct SpawnTemplate {
    cwd: PathBuf,
    command: SpawnCommand,
    size: TerminalSize,
}

#[derive(Debug, thiserror::Error)]
enum SessionMutationError {
    #[error("session has no active tab")]
    NoActiveTab,
    #[error(transparent)]
    Workspace(#[from] mux_workspace::WorkspaceError),
    #[error(transparent)]
    Pane(#[from] crate::pane::PaneError),
    #[error("{kind} name cannot be empty")]
    EmptyName { kind: &'static str },
}

fn validated_name(kind: &'static str, value: &str) -> Result<String, SessionMutationError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Err(SessionMutationError::EmptyName { kind })
    } else {
        Ok(trimmed.to_owned())
    }
}

#[derive(Serialize)]
struct PersistedDaemonState {
    daemon_pid: u32,
    sessions: Vec<SessionSummary>,
}

fn internal_error(error: impl std::fmt::Display) -> RemoteError {
    RemoteError::new(ErrorCode::Internal, error.to_string())
}

fn agent_error(error: &mux_acp::AgentError) -> RemoteError {
    let code = match error {
        mux_acp::AgentError::InvalidSpec(_)
        | mux_acp::AgentError::InvalidWorkingDirectory(_)
        | mux_acp::AgentError::EmptyPrompt
        | mux_acp::AgentError::NotAwaitingAuthentication(_) => ErrorCode::InvalidRequest,
        mux_acp::AgentError::SessionNotFound(_) => ErrorCode::NotFound,
        mux_acp::AgentError::SessionClosed(_) => ErrorCode::Conflict,
        mux_acp::AgentError::Protocol(_) => ErrorCode::Internal,
    };
    RemoteError::new(code, error.to_string())
}

#[cfg(all(test, unix))]
mod tests {
    use std::time::{Duration, Instant};

    use mux_workspace::SplitAxis;

    use super::*;

    #[test]
    fn new_panes_and_tabs_inherit_the_focused_shells_live_working_directory() {
        let state_directory = tempfile::tempdir().expect("state directory");
        let initial_directory = tempfile::tempdir().expect("initial directory");
        let inherited_directory = initial_directory.path().join("nested");
        let tab_directory = inherited_directory.join("tab");
        std::fs::create_dir(&inherited_directory).expect("nested directory");
        std::fs::create_dir(&tab_directory).expect("tab directory");
        let state = DaemonState::new(state_directory.path().to_path_buf(), 64 * 1024);
        let session = state
            .create_session(CreateSession {
                name: "cwd-inheritance".to_owned(),
                cwd: initial_directory.path().to_path_buf(),
                command: SpawnCommand {
                    program: PathBuf::from("/bin/sh"),
                    args: Vec::new(),
                    environment: Vec::new(),
                },
                initial_panes: 1,
                initial_size: TerminalSize::default(),
            })
            .expect("create session");
        let runtime = state
            .resolve(&SessionSelector::Id(session.id))
            .expect("session runtime");
        let original_id = runtime
            .model
            .read()
            .active_tab()
            .expect("active tab")
            .focused_pane;
        let original = runtime
            .panes
            .read()
            .get(&original_id)
            .expect("original pane")
            .clone();
        original
            .write(format!("cd '{}'\n", inherited_directory.display()).as_bytes())
            .expect("change shell directory");
        wait_for_cwd(&original, &inherited_directory);

        runtime
            .apply_command(
                WorkspaceCommand::SplitPane(SplitAxis::Horizontal),
                64 * 1024,
            )
            .expect("split pane");
        let inherited = runtime
            .panes
            .read()
            .values()
            .find(|pane| pane.id != original_id)
            .expect("new pane")
            .clone();
        wait_for_cwd(&inherited, &inherited_directory);

        inherited
            .write(format!("cd '{}'\n", tab_directory.display()).as_bytes())
            .expect("change focused shell directory before opening a tab");
        wait_for_cwd(&inherited, &tab_directory);
        runtime
            .apply_command(WorkspaceCommand::NewTab, 64 * 1024)
            .expect("new tab");
        let tab_pane_id = runtime
            .model
            .read()
            .active_tab()
            .expect("new active tab")
            .focused_pane;
        let tab_pane = runtime
            .panes
            .read()
            .get(&tab_pane_id)
            .expect("new tab pane")
            .clone();
        wait_for_cwd(&tab_pane, &tab_directory);

        for pane in runtime.panes.read().values() {
            let _ = pane.kill();
        }
    }

    #[test]
    fn session_lifecycle_is_owned_and_validated_by_the_daemon() {
        let state_directory = tempfile::tempdir().expect("state directory");
        let working_directory = tempfile::tempdir().expect("working directory");
        let state = DaemonState::new(state_directory.path().to_path_buf(), 64 * 1024);
        let first = state
            .create_session(CreateSession {
                name: "first".to_owned(),
                cwd: working_directory.path().to_path_buf(),
                command: SpawnCommand {
                    program: PathBuf::from("/bin/sh"),
                    args: Vec::new(),
                    environment: Vec::new(),
                },
                initial_panes: 1,
                initial_size: TerminalSize::default(),
            })
            .expect("create first session");
        let first_runtime = state
            .resolve(&SessionSelector::Id(first.id))
            .expect("first runtime");
        let source_pane = first_runtime
            .model
            .read()
            .active_tab()
            .expect("active tab")
            .focused_pane;

        let second = state
            .create_session_for_pane("second".to_owned(), source_pane)
            .expect("create inherited session");
        let second_runtime = state
            .resolve(&SessionSelector::Id(second.id))
            .expect("second runtime");
        let second_pane = second_runtime
            .model
            .read()
            .active_tab()
            .expect("active tab")
            .focused_pane;
        let second_process = second_runtime
            .panes
            .read()
            .get(&second_pane)
            .expect("second pane")
            .clone();
        wait_for_cwd(&second_process, working_directory.path());

        state
            .rename_session(second.id, "renamed")
            .expect("rename session");
        assert_eq!(second_runtime.model.read().name, "renamed");
        assert!(state.rename_session(second.id, "first").is_err());

        state.kill_session(second.id).expect("kill second session");
        assert_eq!(state.list_sessions(), vec![first.clone()]);
        state.kill_session(first.id).expect("kill first session");
        assert!(state.list_sessions().is_empty());
    }

    fn wait_for_cwd(pane: &PaneRuntime, expected: &std::path::Path) {
        let deadline = Instant::now() + Duration::from_secs(2);
        let expected = expected.canonicalize().expect("canonical expected cwd");
        loop {
            let actual = pane
                .current_working_directory()
                .and_then(|cwd| cwd.canonicalize().ok());
            if actual.as_deref() == Some(expected.as_path()) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "pane did not enter {}; last cwd was {actual:?}",
                expected.display(),
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
