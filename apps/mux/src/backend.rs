use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use mux_acp::{AgentConfigValueSelection, AgentPrompt, AgentSpec};
use mux_client::{Client, ClientError, default_state_dir, socket_path};
use mux_protocol::{CreateSession, ErrorCode, ServerEvent, SessionSelector, SpawnCommand};
use mux_terminal::TerminalSize;
use mux_workspace::{AgentSessionId, Direction, PaneId, Session, SessionId, WorkspaceCommand};
use tokio::sync::mpsc;

use crate::UserEvent;

type EventSender = async_channel::Sender<UserEvent>;

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

#[allow(clippy::too_many_lines)]
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
    let mut current_session = attachment.session.clone();
    let mut focused_pane = attachment.session.active_tab().map(|tab| tab.focused_pane);
    send_event(events, UserEvent::Attached(attachment))?;

    loop {
        tokio::select! {
            Some(command) = commands.recv() => {
                match command {
                    CommandMessage::WriteFocused { bytes } => {
                        if let Some(pane_id) = focused_pane
                            && let Err(error) = client.write_input(pane_id, bytes).await
                            && !is_stale_pane_error(&error)
                        {
                            report_backend_error(events, error);
                        }
                    }
                    CommandMessage::Write { pane_id, bytes } => {
                        if let Err(error) = client.write_input(pane_id, bytes).await
                            && !is_stale_pane_error(&error)
                        {
                            report_backend_error(events, error);
                        }
                    }
                    CommandMessage::Resize { pane_id, size } => {
                        if let Err(error) = client.resize_pane(pane_id, size).await
                            && !is_stale_pane_error(&error)
                        {
                            report_backend_error(events, error);
                        }
                    }
                    CommandMessage::Workspace { session_id, command } => {
                        let command = translate_workspace_command(&current_session, command);
                        match client.workspace_command(session_id, command).await {
                            Ok(attachment) => {
                                current_session = attachment.session.clone();
                                focused_pane = attachment
                                    .session
                                    .active_tab()
                                    .map(|tab| tab.focused_pane);
                                send_event(events, UserEvent::Attached(attachment))?;
                            }
                            Err(error) => report_backend_error(events, error),
                        }
                    }
                    CommandMessage::ListSessions => {
                        match client.list_sessions().await {
                            Ok(sessions) => send_event(events, UserEvent::Sessions(sessions))?,
                            Err(error) => report_backend_error(events, error),
                        }
                    }
                    CommandMessage::AttachSession(session_id) => {
                        match client.attach(SessionSelector::Id(session_id)).await {
                            Ok(attachment) => {
                                current_session = attachment.session.clone();
                                focused_pane = current_session
                                    .active_tab()
                                    .map(|tab| tab.focused_pane);
                                send_event(events, UserEvent::Attached(attachment))?;
                            }
                            Err(error) => report_backend_error(events, error),
                        }
                    }
                    CommandMessage::CreateSessionForPane { name, pane_id } => {
                        match client.create_session_for_pane(name, pane_id).await {
                            Ok(session) => match client.attach(SessionSelector::Id(session.id)).await {
                                Ok(attachment) => {
                                    current_session = attachment.session.clone();
                                    focused_pane = current_session
                                        .active_tab()
                                        .map(|tab| tab.focused_pane);
                                    send_event(events, UserEvent::Attached(attachment))?;
                                    let sessions = client.list_sessions().await?;
                                    send_event(events, UserEvent::Sessions(sessions))?;
                                }
                                Err(error) => report_backend_error(events, error),
                            },
                            Err(error) => report_backend_error(events, error),
                        }
                    }
                    CommandMessage::RenameSession { session_id, name } => {
                        match client.rename_session(session_id, name).await {
                            Ok(()) => {
                                let sessions = client.list_sessions().await?;
                                send_event(events, UserEvent::Sessions(sessions))?;
                            }
                            Err(error) => report_backend_error(events, error),
                        }
                    }
                    CommandMessage::KillSession(session_id) => {
                        match client.kill_session(session_id).await {
                            Ok(()) if current_session.id == session_id => {
                                let sessions = client.list_sessions().await?;
                                let next = if let Some(session) = sessions.first() {
                                    session.clone()
                                } else {
                                    create_default_session(&mut client).await?
                                };
                                let attachment = client.attach(SessionSelector::Id(next.id)).await?;
                                current_session = attachment.session.clone();
                                focused_pane = current_session
                                    .active_tab()
                                    .map(|tab| tab.focused_pane);
                                send_event(events, UserEvent::Attached(attachment))?;
                                let sessions = client.list_sessions().await?;
                                send_event(events, UserEvent::Sessions(sessions))?;
                            }
                            Ok(()) => {
                                let sessions = client.list_sessions().await?;
                                send_event(events, UserEvent::Sessions(sessions))?;
                            }
                            Err(error) => report_backend_error(events, error),
                        }
                    }
                    CommandMessage::ListAgents => {
                        match client.list_agent_sessions().await {
                            Ok(agents) => send_event(events, UserEvent::Agents(agents))?,
                            Err(error) => report_backend_error(events, error),
                        }
                    }
                    CommandMessage::StartAgent {
                        spec,
                        pane_id,
                        cwd_override,
                    } => {
                        let result = if let Some(cwd) = cwd_override {
                            client.start_agent(spec, cwd).await
                        } else {
                            client.start_agent_for_pane(spec, pane_id).await
                        };
                        match result {
                            Ok(agent) => send_event(events, UserEvent::AgentStarted(agent))?,
                            Err(error) => report_backend_error(events, error),
                        }
                    }
                    CommandMessage::PromptAgent { session_id, prompt } => {
                        if let Err(error) = client.prompt_agent_with_context(session_id, prompt).await {
                            report_backend_error(events, error);
                        }
                    }
                    CommandMessage::AuthenticateAgent {
                        session_id,
                        method_id,
                    } => {
                        if let Err(error) = client.authenticate_agent(session_id, method_id).await {
                            report_backend_error(events, error);
                        }
                    }
                    CommandMessage::ResolveAgentPermission {
                        session_id,
                        request_id,
                        option_id,
                    } => {
                        if let Err(error) = client
                            .resolve_agent_permission(session_id, request_id, option_id)
                            .await
                        {
                            report_backend_error(events, error);
                        }
                    }
                    CommandMessage::CancelAgent(session_id) => {
                        if let Err(error) = client.cancel_agent(session_id).await {
                            report_backend_error(events, error);
                        }
                    }
                    CommandMessage::CloseAgent(session_id) => {
                        if let Err(error) = client.close_agent(session_id).await {
                            report_backend_error(events, error);
                        }
                    }
                    CommandMessage::SetAgentMode { session_id, mode_id } => {
                        if let Err(error) = client.set_agent_mode(session_id, mode_id).await {
                            report_backend_error(events, error);
                        }
                    }
                    CommandMessage::SetAgentConfig {
                        session_id,
                        config_id,
                        value,
                    } => {
                        if let Err(error) = client
                            .set_agent_config(session_id, config_id, value)
                            .await
                        {
                            report_backend_error(events, error);
                        }
                    }
                }
            }
            event = client.next_event() => {
                match event? {
                    event @ (ServerEvent::PaneOutput { .. } | ServerEvent::PaneExited { .. }) => {
                        send_event(events, UserEvent::Server(event))?;
                    }
                    ServerEvent::ResyncRequired { session_id }
                    | ServerEvent::WorkspaceChanged { session_id } => {
                        let attachment = client.attach(SessionSelector::Id(session_id)).await?;
                        current_session = attachment.session.clone();
                        focused_pane = attachment
                            .session
                            .active_tab()
                            .map(|tab| tab.focused_pane);
                        send_event(events, UserEvent::Attached(attachment))?;
                    }
                    ServerEvent::Agent(event) => send_event(events, UserEvent::Agent(event))?,
                    ServerEvent::AgentResyncRequired => {
                        let agents = client.list_agent_sessions().await?;
                        send_event(events, UserEvent::Agents(agents))?;
                    }
                }
            }
            else => return Ok(()),
        }
    }
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

async fn connect_or_start_daemon(
    state_dir: &std::path::Path,
    socket: &std::path::Path,
) -> Result<Client> {
    match Client::connect(socket, "mux-gui").await {
        Ok(client) => return Ok(client),
        Err(ClientError::ProtocolMismatch { client, server }) => {
            return Err(anyhow!(
                "A different Mux build owns this workspace (daemon protocol {server}, this app {client}). Its shells are still running. Reopen the matching Mux build, or deliberately stop that daemon before starting this update."
            ));
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
