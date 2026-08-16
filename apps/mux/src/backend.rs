use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use mux_acp::{AgentConfigValueSelection, AgentPrompt, AgentSpec};
use mux_client::{Client, ClientError, default_state_dir, socket_path};
use mux_protocol::{
    CreateSession, ErrorCode, ServerEvent, SessionAttachment, SessionSelector, SpawnCommand,
};
use mux_terminal::TerminalSize;
use mux_workspace::{AgentSessionId, Direction, PaneId, Session, SessionId, WorkspaceCommand};
use tokio::sync::mpsc;
use tracing::info;

use crate::UserEvent;

type EventSender = async_channel::Sender<UserEvent>;
const MAX_AGENT_REFERENCE_BYTES: u64 = 256 * 1024;
const MAX_AGENT_REFERENCES_BYTES: u64 = 1024 * 1024;

#[derive(Debug)]
pub enum CommandMessage {
    WriteFocused {
        bytes: Vec<u8>,
    },
    Write {
        pane_id: PaneId,
        bytes: Vec<u8>,
    },
    Resize {
        pane_id: PaneId,
        size: TerminalSize,
    },
    Workspace {
        session_id: SessionId,
        command: WorkspaceCommand,
    },
    ListSessions,
    AttachSession(SessionId),
    CreateSessionForPane {
        name: String,
        pane_id: PaneId,
    },
    RenameSession {
        session_id: SessionId,
        name: String,
    },
    KillSession(SessionId),
    ListAgents,
    StartAgent {
        spec: AgentSpec,
        pane_id: PaneId,
        cwd_override: Option<PathBuf>,
    },
    PromptAgent {
        session_id: AgentSessionId,
        prompt: AgentPrompt,
    },
    AuthenticateAgent {
        session_id: AgentSessionId,
        method_id: String,
    },
    ResolveAgentPermission {
        session_id: AgentSessionId,
        request_id: String,
        option_id: Option<String>,
    },
    CancelAgent(AgentSessionId),
    CloseAgent(AgentSessionId),
    SetAgentMode {
        session_id: AgentSessionId,
        mode_id: String,
    },
    SetAgentConfig {
        session_id: AgentSessionId,
        config_id: String,
        value: AgentConfigValueSelection,
    },
    RefreshAgentFiles {
        pane_id: PaneId,
    },
}

#[derive(Clone)]
pub struct BackendHandle {
    sender: mpsc::UnboundedSender<CommandMessage>,
}

impl BackendHandle {
    pub fn send(&self, command: CommandMessage) {
        let _ = self.sender.send(command);
    }
}

pub fn spawn(events: EventSender, state_dir: Option<PathBuf>) -> BackendHandle {
    let (sender, receiver) = mpsc::unbounded_channel();
    thread::Builder::new()
        .name("mux-backend".to_owned())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_name("mux-backend-io")
                .build()
                .expect("create backend runtime");
            if let Err(error) = runtime.block_on(run(receiver, &events, state_dir)) {
                let _ = events.send_blocking(UserEvent::BackendError(error.to_string()));
            }
        })
        .expect("spawn backend thread");
    BackendHandle { sender }
}

async fn run(
    mut commands: mpsc::UnboundedReceiver<CommandMessage>,
    events: &EventSender,
    state_dir: Option<PathBuf>,
) -> Result<()> {
    let state_dir = state_dir
        .or_else(default_state_dir)
        .ok_or_else(|| anyhow!("no application data directory"))?;
    let socket = socket_path(&state_dir);
    let mut client = connect_or_start_daemon(&state_dir, &socket).await?;
    let sessions = client.list_sessions().await?;
    send_event(events, UserEvent::Sessions(sessions.clone()))?;
    let session_id = if let Some(session) = sessions.first() {
        session.id
    } else {
        create_default_session(&mut client).await?.id
    };
    let attachment = client.attach(SessionSelector::Id(session_id)).await?;
    let mut connection = BackendConnection::new(client, &attachment);
    send_event(events, UserEvent::Attached(attachment))?;

    loop {
        tokio::select! {
            Some(command) = commands.recv() => {
                connection.handle_command(command, events).await?;
            }
            event = connection.client.next_event() => {
                connection.handle_server_event(event?, events).await?;
            }
            else => return Ok(()),
        }
    }
}

struct BackendConnection {
    client: Client,
    current_session: Session,
    focused_pane: Option<PaneId>,
}

impl BackendConnection {
    fn new(client: Client, attachment: &SessionAttachment) -> Self {
        Self {
            client,
            current_session: attachment.session.clone(),
            focused_pane: attachment.session.active_tab().map(|tab| tab.focused_pane),
        }
    }

    fn publish_attachment(
        &mut self,
        events: &EventSender,
        attachment: SessionAttachment,
    ) -> Result<()> {
        self.current_session = attachment.session.clone();
        self.focused_pane = attachment.session.active_tab().map(|tab| tab.focused_pane);
        send_event(events, UserEvent::Attached(attachment))
    }

    fn publish_workspace_update(
        &mut self,
        events: &EventSender,
        attachment: SessionAttachment,
    ) -> Result<()> {
        self.current_session = attachment.session.clone();
        self.focused_pane = attachment.session.active_tab().map(|tab| tab.focused_pane);
        send_event(events, UserEvent::WorkspaceUpdated(attachment))
    }

    async fn publish_sessions(&mut self, events: &EventSender) -> Result<()> {
        match self.client.list_sessions().await {
            Ok(sessions) => send_event(events, UserEvent::Sessions(sessions)),
            Err(error) => {
                report_backend_error(events, error);
                Ok(())
            }
        }
    }

    async fn attach_session(&mut self, events: &EventSender, session_id: SessionId) -> Result<()> {
        match self.client.attach(SessionSelector::Id(session_id)).await {
            Ok(attachment) => self.publish_attachment(events, attachment),
            Err(error) => {
                report_backend_error(events, error);
                Ok(())
            }
        }
    }

    async fn apply_workspace_command(
        &mut self,
        events: &EventSender,
        session_id: SessionId,
        command: WorkspaceCommand,
    ) -> Result<()> {
        let command = translate_workspace_command(&self.current_session, command);
        match self.client.workspace_command(session_id, command).await {
            Ok(attachment) => self.publish_workspace_update(events, attachment),
            Err(error) => {
                report_backend_error(events, error);
                Ok(())
            }
        }
    }

    async fn create_session_for_pane(
        &mut self,
        events: &EventSender,
        name: String,
        pane_id: PaneId,
    ) -> Result<()> {
        match self.client.create_session_for_pane(name, pane_id).await {
            Ok(session) => {
                self.attach_session(events, session.id).await?;
                self.publish_sessions(events).await
            }
            Err(error) => {
                report_backend_error(events, error);
                Ok(())
            }
        }
    }

    async fn rename_session(
        &mut self,
        events: &EventSender,
        session_id: SessionId,
        name: String,
    ) -> Result<()> {
        match self.client.rename_session(session_id, name).await {
            Ok(()) => self.publish_sessions(events).await,
            Err(error) => {
                report_backend_error(events, error);
                Ok(())
            }
        }
    }

    async fn kill_session(&mut self, events: &EventSender, session_id: SessionId) -> Result<()> {
        if let Err(error) = self.client.kill_session(session_id).await {
            report_backend_error(events, error);
            return Ok(());
        }
        if self.current_session.id == session_id {
            let sessions = self.client.list_sessions().await?;
            let next = if let Some(session) = sessions.first() {
                session.clone()
            } else {
                create_default_session(&mut self.client).await?
            };
            let attachment = self.client.attach(SessionSelector::Id(next.id)).await?;
            self.publish_attachment(events, attachment)?;
        }
        self.publish_sessions(events).await
    }

    #[allow(clippy::too_many_lines)]
    async fn handle_command(
        &mut self,
        command: CommandMessage,
        events: &EventSender,
    ) -> Result<()> {
        match command {
            CommandMessage::WriteFocused { bytes } => {
                let Some(pane_id) = self.focused_pane else {
                    return Ok(());
                };
                report_pane_result(events, self.client.write_input(pane_id, bytes).await);
            }
            CommandMessage::Write { pane_id, bytes } => {
                report_pane_result(events, self.client.write_input(pane_id, bytes).await);
            }
            CommandMessage::Resize { pane_id, size } => {
                report_pane_result(events, self.client.resize_pane(pane_id, size).await);
            }
            CommandMessage::Workspace {
                session_id,
                command,
            } => {
                self.apply_workspace_command(events, session_id, command)
                    .await?;
            }
            CommandMessage::ListSessions => self.publish_sessions(events).await?,
            CommandMessage::AttachSession(session_id) => {
                self.attach_session(events, session_id).await?;
            }
            CommandMessage::CreateSessionForPane { name, pane_id } => {
                self.create_session_for_pane(events, name, pane_id).await?;
            }
            CommandMessage::RenameSession { session_id, name } => {
                self.rename_session(events, session_id, name).await?;
            }
            CommandMessage::KillSession(session_id) => {
                self.kill_session(events, session_id).await?;
            }
            CommandMessage::ListAgents => match self.client.list_agent_sessions().await {
                Ok(agents) => send_event(events, UserEvent::Agents(agents))?,
                Err(error) => report_backend_error(events, error),
            },
            CommandMessage::StartAgent {
                spec,
                pane_id,
                cwd_override,
            } => {
                // Finder-launched macOS apps do not inherit PATH from the
                // user's shell. Resolve it off the UI thread and pass it to
                // the live daemon with the existing request.
                let spec = spec.resolve_runtime_environment();
                match self
                    .client
                    .start_agent_for_pane(spec, pane_id, cwd_override)
                    .await
                {
                    Ok(agent) => send_event(events, UserEvent::AgentStarted(agent))?,
                    Err(error) => report_backend_error(events, error),
                }
            }
            CommandMessage::PromptAgent {
                session_id,
                mut prompt,
            } => {
                if let Err(error) = hydrate_agent_references(&mut prompt).await {
                    report_backend_error(events, error);
                    return Ok(());
                }
                report_client_result(
                    events,
                    self.client
                        .prompt_agent_with_context(session_id, prompt)
                        .await,
                );
            }
            CommandMessage::AuthenticateAgent {
                session_id,
                method_id,
            } => {
                report_client_result(
                    events,
                    self.client.authenticate_agent(session_id, method_id).await,
                );
            }
            CommandMessage::ResolveAgentPermission {
                session_id,
                request_id,
                option_id,
            } => {
                report_client_result(
                    events,
                    self.client
                        .resolve_agent_permission(session_id, request_id, option_id)
                        .await,
                );
            }
            CommandMessage::CancelAgent(session_id) => {
                report_client_result(events, self.client.cancel_agent(session_id).await);
            }
            CommandMessage::CloseAgent(session_id) => {
                report_client_result(events, self.client.close_agent(session_id).await);
            }
            CommandMessage::SetAgentMode {
                session_id,
                mode_id,
            } => {
                report_client_result(
                    events,
                    self.client.set_agent_mode(session_id, mode_id).await,
                );
            }
            CommandMessage::SetAgentConfig {
                session_id,
                config_id,
                value,
            } => {
                report_client_result(
                    events,
                    self.client
                        .set_agent_config(session_id, config_id, value)
                        .await,
                );
            }
            CommandMessage::RefreshAgentFiles { pane_id } => {
                match self.client.pane_working_directory(pane_id).await {
                    Ok(cwd) => {
                        let scan_root = cwd.clone();
                        let files = tokio::task::spawn_blocking(move || {
                            crate::agent_completion::index_files(&scan_root)
                        })
                        .await
                        .context("index agent reference files")?;
                        send_event(
                            events,
                            UserEvent::AgentFiles {
                                pane_id,
                                cwd,
                                files,
                            },
                        )?;
                    }
                    Err(error) => report_backend_error(events, error),
                }
            }
        }
        Ok(())
    }

    async fn handle_server_event(
        &mut self,
        event: ServerEvent,
        events: &EventSender,
    ) -> Result<()> {
        match event {
            event @ (ServerEvent::PaneOutput { .. } | ServerEvent::PaneExited { .. }) => {
                send_event(events, UserEvent::Server(event))
            }
            ServerEvent::ResyncRequired { session_id } => {
                let attachment = self.client.attach(SessionSelector::Id(session_id)).await?;
                self.publish_attachment(events, attachment)
            }
            ServerEvent::WorkspaceChanged { session_id } => {
                let attachment = self.client.attach(SessionSelector::Id(session_id)).await?;
                self.publish_workspace_update(events, attachment)
            }
            ServerEvent::Agent(event) => send_event(events, UserEvent::Agent(event)),
            ServerEvent::AgentResyncRequired => {
                let agents = self.client.list_agent_sessions().await?;
                send_event(events, UserEvent::Agents(agents))
            }
        }
    }
}

async fn hydrate_agent_references(prompt: &mut AgentPrompt) -> Result<()> {
    let mut total = 0_u64;
    for reference in &mut prompt.files {
        let metadata = tokio::fs::metadata(&reference.path)
            .await
            .with_context(|| format!("read referenced file {}", reference.path.display()))?;
        if !metadata.is_file() {
            return Err(anyhow!(
                "referenced path is not a file: {}",
                reference.path.display()
            ));
        }
        if metadata.len() > MAX_AGENT_REFERENCE_BYTES {
            return Err(anyhow!(
                "referenced file is larger than 256 KiB: {}",
                reference.path.display()
            ));
        }
        total = total.saturating_add(metadata.len());
        if total > MAX_AGENT_REFERENCES_BYTES {
            return Err(anyhow!("referenced files exceed the 1 MiB prompt limit"));
        }
        let bytes = tokio::fs::read(&reference.path)
            .await
            .with_context(|| format!("read referenced file {}", reference.path.display()))?;
        if bytes.contains(&0) {
            return Err(anyhow!(
                "binary files cannot be attached to an agent prompt: {}",
                reference.path.display()
            ));
        }
        reference.text = String::from_utf8_lossy(&bytes).into_owned();
    }
    Ok(())
}

fn translate_workspace_command(session: &Session, command: WorkspaceCommand) -> WorkspaceCommand {
    let WorkspaceCommand::FocusPaneOrTab(direction) = command else {
        return command;
    };
    if session.active_tab().is_none() {
        return WorkspaceCommand::FocusPane(direction);
    }
    let before_tab = session.active_tab;
    let mut projected = session.clone();
    if projected.move_focus_or_tab(direction).is_err() {
        return WorkspaceCommand::FocusPane(direction);
    }
    if projected.active_tab != before_tab {
        return match direction {
            Direction::Left => WorkspaceCommand::PreviousTab,
            Direction::Right => WorkspaceCommand::NextTab,
            Direction::Up | Direction::Down => WorkspaceCommand::FocusPane(direction),
        };
    }
    WorkspaceCommand::FocusPane(direction)
}

fn is_stale_pane_error(error: &ClientError) -> bool {
    matches!(
        error,
        ClientError::Remote(remote) if remote.code == ErrorCode::NotFound
    )
}

fn send_event(events: &EventSender, event: UserEvent) -> Result<()> {
    events
        .send_blocking(event)
        .map_err(|_| anyhow!("GUI event loop stopped"))
}

fn report_backend_error(events: &EventSender, error: impl std::fmt::Display) {
    let _ = events.send_blocking(UserEvent::BackendError(error.to_string()));
}

fn report_client_result(events: &EventSender, result: Result<(), ClientError>) {
    if let Err(error) = result {
        report_backend_error(events, error);
    }
}

fn report_pane_result(events: &EventSender, result: Result<(), ClientError>) {
    if let Err(error) = result
        && !is_stale_pane_error(&error)
    {
        report_backend_error(events, error);
    }
}

async fn connect_or_start_daemon(
    state_dir: &std::path::Path,
    socket: &std::path::Path,
) -> Result<Client> {
    match Client::connect(socket, "mux-gui").await {
        Ok(client) => return Ok(client),
        Err(ClientError::ProtocolMismatch {
            client,
            server,
            daemon_pid,
        }) => {
            info!(
                client,
                server, daemon_pid, "replacing incompatible workspace daemon"
            );
            stop_incompatible_daemon(daemon_pid).await?;
        }
        Err(_) => {}
    }

    let executable = std::env::current_exe().context("resolve mux executable")?;
    Command::new(executable)
        .arg("--daemon")
        .arg("--state-dir")
        .arg(state_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("start persistent workspace daemon")?;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        match Client::connect(socket, "mux-gui").await {
            Ok(client) => return Ok(client),
            Err(error) if tokio::time::Instant::now() < deadline => {
                let _ = error;
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            Err(error) => return Err(error).context("workspace daemon did not become ready"),
        }
    }
}

async fn stop_incompatible_daemon(daemon_pid: u32) -> Result<()> {
    let status = Command::new("/bin/kill")
        .arg("-TERM")
        .arg(daemon_pid.to_string())
        .status()
        .context("stop incompatible workspace daemon")?;
    if !status.success() {
        return Err(anyhow!(
            "could not stop incompatible workspace daemon {daemon_pid}"
        ));
    }

    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while process_is_alive(daemon_pid) && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    if !process_is_alive(daemon_pid) {
        return Ok(());
    }

    info!(daemon_pid, "forcing incompatible workspace daemon to stop");
    let status = Command::new("/bin/kill")
        .arg("-KILL")
        .arg(daemon_pid.to_string())
        .status()
        .context("force-stop incompatible workspace daemon")?;
    if !status.success() {
        return Err(anyhow!(
            "could not force-stop incompatible workspace daemon {daemon_pid}"
        ));
    }
    Ok(())
}

fn process_is_alive(pid: u32) -> bool {
    Command::new("/bin/kill")
        .arg("-0")
        .arg(pid.to_string())
        .status()
        .is_ok_and(|status| status.success())
}

async fn create_default_session(client: &mut Client) -> Result<mux_protocol::SessionSummary> {
    let shell = std::env::var_os("SHELL")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from("/bin/zsh"));
    client
        .create_session(CreateSession {
            name: "main".to_owned(),
            cwd: default_shell_cwd()?,
            command: SpawnCommand {
                program: shell,
                args: vec!["-l".to_owned()],
                environment: Vec::new(),
            },
            initial_panes: 1,
            initial_size: TerminalSize::default(),
        })
        .await
        .map_err(Into::into)
}

fn default_shell_cwd() -> Result<PathBuf> {
    directories::BaseDirs::new()
        .map(|directories| directories.home_dir().to_path_buf())
        .or_else(|| std::env::current_dir().ok())
        .context("resolve home directory")
}

pub async fn run_daemon(state_dir: PathBuf) -> Result<()> {
    mux_daemon::DaemonServer::new(mux_daemon::DaemonConfig::in_state_dir(state_dir))
        .run()
        .await
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_workspace_starts_in_the_users_home_directory() {
        let expected = directories::BaseDirs::new()
            .expect("platform home directory")
            .home_dir()
            .to_path_buf();
        assert_eq!(default_shell_cwd().expect("default cwd"), expected);
    }

    #[test]
    fn closed_pane_races_do_not_stop_the_backend() {
        let stale = ClientError::Remote(mux_protocol::RemoteError::new(
            ErrorCode::NotFound,
            "pane already closed",
        ));
        let real = ClientError::Remote(mux_protocol::RemoteError::new(
            ErrorCode::Internal,
            "terminal failure",
        ));
        assert!(is_stale_pane_error(&stale));
        assert!(!is_stale_pane_error(&real));
    }

    #[test]
    fn focus_or_tab_is_translated_to_stable_daemon_commands() {
        let left = PaneId::new();
        let right = PaneId::new();
        let other = PaneId::new();
        let mut session = Session::with_panes("daily", &[left, right]).expect("session");
        let first_tab = session.active_tab;
        session.add_tab(other).expect("other tab");
        session.select_tab(first_tab).expect("first tab");

        assert_eq!(
            translate_workspace_command(
                &session,
                WorkspaceCommand::FocusPaneOrTab(Direction::Right),
            ),
            WorkspaceCommand::FocusPane(Direction::Right),
        );
        session
            .active_tab_mut()
            .expect("active tab")
            .focus(right)
            .expect("right pane");
        assert_eq!(
            translate_workspace_command(
                &session,
                WorkspaceCommand::FocusPaneOrTab(Direction::Right),
            ),
            WorkspaceCommand::NextTab,
        );
    }
}
