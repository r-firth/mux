use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mux_protocol::{
    ClientHello, ClientMessage, CodecError, FrameReader, PROTOCOL_VERSION, Request, Response,
    ServerEvent, ServerHello, ServerMessage, read_frame, write_frame,
};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use crate::state::DaemonState;

#[derive(Clone, Debug)]
pub struct DaemonConfig {
    pub state_dir: PathBuf,
    pub socket_path: PathBuf,
    pub replay_bytes_per_pane: usize,
}

impl DaemonConfig {
    #[must_use]
    pub fn in_state_dir(state_dir: PathBuf) -> Self {
        Self {
            socket_path: default_socket_path(&state_dir),
            state_dir,
            replay_bytes_per_pane: 4 * 1024 * 1024,
        }
    }
}

#[must_use]
pub fn default_socket_path(state_dir: &Path) -> PathBuf {
    state_dir.join("daemon.sock")
}

pub struct DaemonServer {
    config: DaemonConfig,
    state: Arc<DaemonState>,
}

impl DaemonServer {
    #[must_use]
    pub fn new(config: DaemonConfig) -> Self {
        let state = Arc::new(DaemonState::new(
            config.state_dir.clone(),
            config.replay_bytes_per_pane,
        ));
        Self { config, state }
    }

    pub async fn run(self) -> Result<(), ServerError> {
        std::fs::create_dir_all(&self.config.state_dir)?;
        reject_live_or_remove_stale_socket(&self.config.socket_path).await?;
        let listener = UnixListener::bind(&self.config.socket_path)?;
        set_private_socket_permissions(&self.config.socket_path)?;
        info!(socket = %self.config.socket_path.display(), "workspace daemon listening");

        loop {
            let (stream, _) = listener.accept().await?;
            let state = Arc::clone(&self.state);
            tokio::spawn(async move {
                if let Err(error) = handle_connection(stream, state).await {
                    debug!(%error, "client connection ended");
                }
            });
        }
    }
}

async fn reject_live_or_remove_stale_socket(path: &Path) -> Result<(), ServerError> {
    if !path.exists() {
        return Ok(());
    }
    if UnixStream::connect(path).await.is_ok() {
        Err(ServerError::AlreadyRunning(path.to_path_buf()))
    } else {
        warn!(socket = %path.display(), "removing stale daemon socket");
        std::fs::remove_file(path)?;
        Ok(())
    }
}

#[cfg(unix)]
fn set_private_socket_permissions(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

async fn handle_connection(
    stream: UnixStream,
    state: Arc<DaemonState>,
) -> Result<(), ConnectionError> {
    let (mut reader, mut writer) = stream.into_split();
    let hello: ClientMessage = read_frame(&mut reader).await?;
    let ClientMessage::Hello(ClientHello {
        protocol_version,
        client_name,
    }) = hello
    else {
        return Err(ConnectionError::ExpectedHello);
    };

    write_frame(
        &mut writer,
        &ServerMessage::Hello(ServerHello {
            protocol_version: PROTOCOL_VERSION,
            daemon_pid: std::process::id(),
        }),
    )
    .await?;
    if protocol_version != PROTOCOL_VERSION {
        return Err(ConnectionError::ProtocolMismatch {
            client: protocol_version,
            server: PROTOCOL_VERSION,
        });
    }
    debug!(%client_name, "client attached to daemon transport");

    let mut incoming = FrameReader::<ClientMessage>::spawn(reader);
    let mut agent_events = state.subscribe_agent_events();
    let mut subscription: Option<(mux_workspace::SessionId, broadcast::Receiver<ServerEvent>)> =
        None;

    loop {
        if let Some((session_id, receiver)) = subscription.as_mut() {
            tokio::select! {
                incoming = incoming.next() => {
                    let message = incoming.ok_or(ConnectionError::ClientDisconnected)??;
                    process_message(message, &state, &mut writer, &mut subscription).await?;
                }
                event = receiver.recv() => {
                    match event {
                        Ok(event) => write_frame(&mut writer, &ServerMessage::Event(event)).await?,
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            write_frame(
                                &mut writer,
                                &ServerMessage::Event(ServerEvent::ResyncRequired {
                                    session_id: *session_id,
                                }),
                            ).await?;
                        }
                        Err(broadcast::error::RecvError::Closed) => subscription = None,
                    }
                }
                event = agent_events.recv() => {
                    match event {
                        Ok(event) => write_frame(&mut writer, &ServerMessage::Event(ServerEvent::Agent(event))).await?,
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            write_frame(
                                &mut writer,
                                &ServerMessage::Event(ServerEvent::AgentResyncRequired),
                            ).await?;
                        }
                        Err(broadcast::error::RecvError::Closed) => {}
                    }
                }
            }
        } else {
            tokio::select! {
                incoming = incoming.next() => {
                    let message = incoming.ok_or(ConnectionError::ClientDisconnected)??;
                    process_message(message, &state, &mut writer, &mut subscription).await?;
                }
                event = agent_events.recv() => {
                    match event {
                        Ok(event) => write_frame(&mut writer, &ServerMessage::Event(ServerEvent::Agent(event))).await?,
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            write_frame(
                                &mut writer,
                                &ServerMessage::Event(ServerEvent::AgentResyncRequired),
                            ).await?;
                        }
                        Err(broadcast::error::RecvError::Closed) => {}
                    }
                }
            }
        }
    }
}

async fn process_message<W>(
    message: ClientMessage,
    state: &DaemonState,
    writer: &mut W,
    subscription: &mut Option<(mux_workspace::SessionId, broadcast::Receiver<ServerEvent>)>,
) -> Result<(), ConnectionError>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    let ClientMessage::Request {
        request_id,
        request,
    } = message
    else {
        return Err(ConnectionError::UnexpectedHello);
    };

    let response = match request {
        Request::Health => Ok(Response::Pong),
        Request::ListSessions => Ok(Response::Sessions(state.list_sessions())),
        Request::CreateSession(request) => {
            state.create_session(request).map(Response::SessionCreated)
        }
        Request::AttachSession { session } => match state.prepare_attach(&session) {
            Ok((attachment, receiver)) => {
                let session_id = attachment.session.id;
                *subscription = Some((session_id, receiver));
                Ok(Response::Attached(attachment))
            }
            Err(error) => Err(error),
        },
        Request::WriteInput { pane_id, bytes } => {
            state.write_input(pane_id, &bytes).map(|()| Response::Ack)
        }
        Request::ResizePane { pane_id, size } => {
            state.resize_pane(pane_id, size).map(|()| Response::Ack)
        }
        Request::WorkspaceCommand {
            session_id,
            command,
        } => state
            .workspace_command(session_id, command)
            .map(Response::Attached),
        Request::ListAgentSessions => Ok(Response::AgentSessions(state.list_agents())),
        Request::StartAgent { spec, cwd } => {
            state.start_agent(&spec, cwd).map(Response::AgentStarted)
        }
        Request::PromptAgent { session_id, prompt } => state
            .prompt_agent(session_id, prompt)
            .map(|()| Response::Ack),
        Request::ResolveAgentPermission {
            session_id,
            request_id,
            option_id,
        } => state
            .resolve_agent_permission(session_id, request_id, option_id)
            .map(|()| Response::Ack),
        Request::CancelAgent { session_id } => {
            state.cancel_agent(session_id).map(|()| Response::Ack)
        }
        Request::CloseAgent { session_id } => state.close_agent(session_id).map(|()| Response::Ack),
        Request::StartAgentForPane { spec, pane_id } => state
            .start_agent_for_pane(&spec, pane_id)
            .map(Response::AgentStarted),
        Request::SetAgentMode {
            session_id,
            mode_id,
        } => state
            .set_agent_mode(session_id, mode_id)
            .map(|()| Response::Ack),
        Request::SetAgentConfig {
            session_id,
            config_id,
            value,
        } => state
            .set_agent_config(session_id, config_id, value)
            .map(|()| Response::Ack),
        Request::CreateSessionForPane { name, pane_id } => state
            .create_session_for_pane(name, pane_id)
            .map(Response::SessionCreated),
        Request::RenameSession { session_id, name } => state
            .rename_session(session_id, &name)
            .map(|()| Response::Ack),
        Request::KillSession { session_id } => {
            state.kill_session(session_id).map(|()| Response::Ack)
        }
        Request::AuthenticateAgent {
            session_id,
            method_id,
        } => state
            .authenticate_agent(session_id, method_id)
            .map(|()| Response::Ack),
    };

    write_frame(
        writer,
        &ServerMessage::Response {
            request_id,
            response,
        },
    )
    .await?;
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("daemon is already running at {0}")]
    AlreadyRunning(PathBuf),
    #[error("daemon I/O failed: {0}")]
    Io(#[from] io::Error),
}

#[derive(Debug, thiserror::Error)]
enum ConnectionError {
    #[error("protocol codec failed: {0}")]
    Codec(#[from] CodecError),
    #[error("client did not begin with a hello message")]
    ExpectedHello,
    #[error("client sent a second hello message")]
    UnexpectedHello,
    #[error("client disconnected")]
    ClientDisconnected,
    #[error("protocol mismatch: client={client}, server={server}")]
    ProtocolMismatch { client: u16, server: u16 },
}
