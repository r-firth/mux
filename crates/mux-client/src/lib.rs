//! GUI-facing client for the local workspace daemon.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use mux_acp::{AgentConfigValueSelection, AgentPrompt, AgentSessionSnapshot, AgentSpec};
use mux_protocol::{
    ClientHello, ClientMessage, CodecError, CreateSession, FrameReader, PROTOCOL_VERSION,
    RemoteError, Request, Response, ServerEvent, ServerMessage, SessionAttachment, SessionSelector,
    SessionSummary, UNACKNOWLEDGED_REQUEST_ID, read_frame, write_frame,
};
use mux_terminal::TerminalSize;
use mux_workspace::{AgentSessionId, PaneId, SessionId, WorkspaceCommand};
use thiserror::Error;
use tokio::net::UnixStream;
use tokio::net::unix::OwnedWriteHalf;

pub struct Client {
    reader: FrameReader<ServerMessage>,
    writer: OwnedWriteHalf,
    next_request_id: u64,
    pending_events: VecDeque<ServerEvent>,
    next_output_sequence: HashMap<PaneId, u64>,
    desynced_session: Option<SessionId>,
    daemon_pid: u32,
}

impl Client {
    pub async fn connect(socket_path: &Path, client_name: &str) -> Result<Self, ClientError> {
        let stream = UnixStream::connect(socket_path).await?;
        let (mut reader, mut writer) = stream.into_split();
        write_frame(
            &mut writer,
            &ClientMessage::Hello(ClientHello {
                protocol_version: PROTOCOL_VERSION,
                client_name: client_name.to_owned(),
            }),
        )
        .await?;
        let hello: ServerMessage = read_frame(&mut reader).await?;
        let ServerMessage::Hello(hello) = hello else {
            return Err(ClientError::ExpectedHello);
        };
        if hello.protocol_version != PROTOCOL_VERSION {
            return Err(ClientError::ProtocolMismatch {
                client: PROTOCOL_VERSION,
                server: hello.protocol_version,
            });
        }

        Ok(Self {
            reader: FrameReader::spawn(reader),
            writer,
            next_request_id: 1,
            pending_events: VecDeque::new(),
            next_output_sequence: HashMap::new(),
            desynced_session: None,
            daemon_pid: hello.daemon_pid,
        })
    }

    #[must_use]
    pub const fn daemon_pid(&self) -> u32 {
        self.daemon_pid
    }

    pub async fn health(&mut self) -> Result<(), ClientError> {
        expect_acknowledgement(self.request(Request::Health).await?, &Response::Pong)
    }

    pub async fn list_sessions(&mut self) -> Result<Vec<SessionSummary>, ClientError> {
        match self.request(Request::ListSessions).await? {
            Response::Sessions(sessions) => Ok(sessions),
            response => Err(ClientError::UnexpectedResponse(Box::new(response))),
        }
    }

    pub async fn create_session(
        &mut self,
        request: CreateSession,
    ) -> Result<SessionSummary, ClientError> {
        match self.request(Request::CreateSession(request)).await? {
            Response::SessionCreated(session) => Ok(session),
            response => Err(ClientError::UnexpectedResponse(Box::new(response))),
        }
    }

    pub async fn create_session_for_pane(
        &mut self,
        name: String,
        pane_id: PaneId,
    ) -> Result<SessionSummary, ClientError> {
        match self
            .request(Request::CreateSessionForPane { name, pane_id })
            .await?
        {
            Response::SessionCreated(session) => Ok(session),
            response => Err(ClientError::UnexpectedResponse(Box::new(response))),
        }
    }

    pub async fn rename_session(
        &mut self,
        session_id: SessionId,
        name: String,
    ) -> Result<(), ClientError> {
        expect_acknowledgement(
            self.request(Request::RenameSession { session_id, name })
                .await?,
            &Response::Ack,
        )
    }

    pub async fn kill_session(&mut self, session_id: SessionId) -> Result<(), ClientError> {
        expect_acknowledgement(
            self.request(Request::KillSession { session_id }).await?,
            &Response::Ack,
        )
    }

    pub async fn attach(
        &mut self,
        session: SessionSelector,
    ) -> Result<SessionAttachment, ClientError> {
        match self.request(Request::AttachSession { session }).await? {
            Response::Attached(attachment) => {
                self.pending_events.clear();
                self.next_output_sequence = attachment
                    .panes
                    .iter()
                    .map(|pane| (pane.pane_id, pane.terminal.next_sequence))
                    .collect();
                self.desynced_session = None;
                Ok(attachment)
            }
            response => Err(ClientError::UnexpectedResponse(Box::new(response))),
        }
    }

    pub async fn write_input(
        &mut self,
        pane_id: PaneId,
        bytes: Vec<u8>,
    ) -> Result<(), ClientError> {
        write_frame(
            &mut self.writer,
            &ClientMessage::Request {
                request_id: UNACKNOWLEDGED_REQUEST_ID,
                request: Request::WriteInput { pane_id, bytes },
            },
        )
        .await?;
        Ok(())
    }

    pub async fn resize_pane(
        &mut self,
        pane_id: PaneId,
        size: TerminalSize,
    ) -> Result<(), ClientError> {
        expect_acknowledgement(
            self.request(Request::ResizePane { pane_id, size }).await?,
            &Response::Ack,
        )
    }

    pub async fn workspace_command(
        &mut self,
        session_id: SessionId,
        command: WorkspaceCommand,
    ) -> Result<SessionAttachment, ClientError> {
        match self
            .request(Request::WorkspaceCommand {
                session_id,
                command,
            })
            .await?
        {
            Response::Attached(attachment) => {
                self.next_output_sequence = attachment
                    .panes
                    .iter()
                    .map(|pane| (pane.pane_id, pane.terminal.next_sequence))
                    .collect();
                Ok(attachment)
            }
            response => Err(ClientError::UnexpectedResponse(Box::new(response))),
        }
    }

    pub async fn list_agent_sessions(&mut self) -> Result<Vec<AgentSessionSnapshot>, ClientError> {
        match self.request(Request::ListAgentSessions).await? {
            Response::AgentSessions(sessions) => Ok(sessions),
            response => Err(ClientError::UnexpectedResponse(Box::new(response))),
        }
    }

    pub async fn start_agent(
        &mut self,
        spec: AgentSpec,
        cwd: PathBuf,
    ) -> Result<AgentSessionSnapshot, ClientError> {
        match self.request(Request::StartAgent { spec, cwd }).await? {
            Response::AgentStarted(session) => Ok(*session),
            response => Err(ClientError::UnexpectedResponse(Box::new(response))),
        }
    }

    pub async fn start_agent_for_pane(
        &mut self,
        spec: AgentSpec,
        pane_id: PaneId,
        cwd_override: Option<PathBuf>,
    ) -> Result<AgentSessionSnapshot, ClientError> {
        match self
            .request(Request::StartAgentForPane {
                spec,
                pane_id,
                cwd_override,
            })
            .await?
        {
            Response::AgentStarted(session) => Ok(*session),
            response => Err(ClientError::UnexpectedResponse(Box::new(response))),
        }
    }

    pub async fn prompt_agent(
        &mut self,
        session_id: AgentSessionId,
        text: String,
    ) -> Result<(), ClientError> {
        self.prompt_agent_with_context(session_id, text.into())
            .await
    }

    pub async fn prompt_agent_with_context(
        &mut self,
        session_id: AgentSessionId,
        prompt: AgentPrompt,
    ) -> Result<(), ClientError> {
        expect_acknowledgement(
            self.request(Request::PromptAgent { session_id, prompt })
                .await?,
            &Response::Ack,
        )
    }

    pub async fn authenticate_agent(
        &mut self,
        session_id: AgentSessionId,
        method_id: String,
    ) -> Result<(), ClientError> {
        expect_acknowledgement(
            self.request(Request::AuthenticateAgent {
                session_id,
                method_id,
            })
            .await?,
            &Response::Ack,
        )
    }

    pub async fn set_agent_mode(
        &mut self,
        session_id: AgentSessionId,
        mode_id: String,
    ) -> Result<(), ClientError> {
        expect_acknowledgement(
            self.request(Request::SetAgentMode {
                session_id,
                mode_id,
            })
            .await?,
            &Response::Ack,
        )
    }

    pub async fn set_agent_config(
        &mut self,
        session_id: AgentSessionId,
        config_id: String,
        value: AgentConfigValueSelection,
    ) -> Result<(), ClientError> {
        expect_acknowledgement(
            self.request(Request::SetAgentConfig {
                session_id,
                config_id,
                value,
            })
            .await?,
            &Response::Ack,
        )
    }

    pub async fn resolve_agent_permission(
        &mut self,
        session_id: AgentSessionId,
        request_id: String,
        option_id: Option<String>,
    ) -> Result<(), ClientError> {
        expect_acknowledgement(
            self.request(Request::ResolveAgentPermission {
                session_id,
                request_id,
                option_id,
            })
            .await?,
            &Response::Ack,
        )
    }

    pub async fn cancel_agent(&mut self, session_id: AgentSessionId) -> Result<(), ClientError> {
        expect_acknowledgement(
            self.request(Request::CancelAgent { session_id }).await?,
            &Response::Ack,
        )
    }

    pub async fn close_agent(&mut self, session_id: AgentSessionId) -> Result<(), ClientError> {
        expect_acknowledgement(
            self.request(Request::CloseAgent { session_id }).await?,
            &Response::Ack,
        )
    }

    pub async fn next_event(&mut self) -> Result<ServerEvent, ClientError> {
        if let Some(event) = self.pending_events.pop_front() {
            return Ok(event);
        }
        loop {
            match self.receive_message().await? {
                ServerMessage::Event(event) => {
                    if let Some(event) = self.normalize_event(event) {
                        return Ok(event);
                    }
                }
                ServerMessage::Response {
                    request_id: UNACKNOWLEDGED_REQUEST_ID,
                    response,
                } => handle_unacknowledged_response(response)?,
                ServerMessage::Response { request_id, .. } => {
                    return Err(ClientError::UnexpectedResponseId(request_id));
                }
                ServerMessage::Hello(_) => return Err(ClientError::UnexpectedHello),
            }
        }
    }

    async fn request(&mut self, request: Request) -> Result<Response, ClientError> {
        let request_id = self.next_request_id;
        self.next_request_id += 1;
        write_frame(
            &mut self.writer,
            &ClientMessage::Request {
                request_id,
                request,
            },
        )
        .await?;

        loop {
            match self.receive_message().await? {
                ServerMessage::Response {
                    request_id: actual,
                    response,
                } if actual == request_id => return response.map_err(ClientError::Remote),
                ServerMessage::Response {
                    request_id: UNACKNOWLEDGED_REQUEST_ID,
                    response,
                } => handle_unacknowledged_response(response)?,
                ServerMessage::Response {
                    request_id: actual, ..
                } => return Err(ClientError::UnexpectedResponseId(actual)),
                ServerMessage::Event(event) => {
                    if let Some(event) = self.normalize_event(event) {
                        self.pending_events.push_back(event);
                    }
                }
                ServerMessage::Hello(_) => return Err(ClientError::UnexpectedHello),
            }
        }
    }

    fn normalize_event(&mut self, event: ServerEvent) -> Option<ServerEvent> {
        match event {
            ServerEvent::PaneOutput {
                session_id,
                pane_id,
                sequence,
                bytes,
            } => {
                if self.desynced_session == Some(session_id) {
                    return None;
                }
                let Some(expected) = self.next_output_sequence.get_mut(&pane_id) else {
                    return Some(ServerEvent::PaneOutput {
                        session_id,
                        pane_id,
                        sequence,
                        bytes,
                    });
                };
                if sequence < *expected {
                    return None;
                }
                if sequence > *expected {
                    self.desynced_session = Some(session_id);
                    return Some(ServerEvent::ResyncRequired { session_id });
                }
                *expected += 1;
                Some(ServerEvent::PaneOutput {
                    session_id,
                    pane_id,
                    sequence,
                    bytes,
                })
            }
            ServerEvent::ResyncRequired { session_id } => {
                if self.desynced_session.replace(session_id) == Some(session_id) {
                    None
                } else {
                    Some(ServerEvent::ResyncRequired { session_id })
                }
            }
            event @ (ServerEvent::PaneExited { .. }
            | ServerEvent::WorkspaceChanged { .. }
            | ServerEvent::Agent(_)
            | ServerEvent::AgentResyncRequired) => Some(event),
        }
    }

    async fn receive_message(&mut self) -> Result<ServerMessage, ClientError> {
        self.reader
            .next()
            .await
            .ok_or(ClientError::ConnectionClosed)?
            .map_err(Into::into)
    }
}

#[must_use]
pub fn default_state_dir() -> Option<PathBuf> {
    state_dir_for("Mux")
}

/// Resolve a per-user state directory for a named Mux application profile.
/// Distinct preview/dev bundles can use an isolated daemon without colliding
/// with the installed product's persistent workspace.
#[must_use]
pub fn state_dir_for(application: &str) -> Option<PathBuf> {
    ProjectDirs::from("io", "mux", application).map(|dirs| dirs.data_local_dir().to_path_buf())
}

#[must_use]
pub fn socket_path(state_dir: &Path) -> PathBuf {
    state_dir.join("daemon.sock")
}

fn expect_acknowledgement(actual: Response, expected: &Response) -> Result<(), ClientError> {
    if &actual == expected {
        Ok(())
    } else {
        Err(ClientError::UnexpectedResponse(Box::new(actual)))
    }
}

fn handle_unacknowledged_response(
    response: Result<Response, RemoteError>,
) -> Result<(), ClientError> {
    expect_acknowledgement(response.map_err(ClientError::Remote)?, &Response::Ack)
}

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("daemon connection failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("protocol codec failed: {0}")]
    Codec(#[from] CodecError),
    #[error("daemon rejected the request: {0}")]
    Remote(#[from] RemoteError),
    #[error("daemon did not begin with a hello message")]
    ExpectedHello,
    #[error("daemon sent an unexpected hello message")]
    UnexpectedHello,
    #[error("daemon connection closed")]
    ConnectionClosed,
    #[error("protocol mismatch: client={client}, server={server}")]
    ProtocolMismatch { client: u16, server: u16 },
    #[error("daemon sent an unexpected response id: {0}")]
    UnexpectedResponseId(u64),
    #[error("daemon sent an unexpected response: {0:?}")]
    UnexpectedResponse(Box<Response>),
}

#[cfg(test)]
mod tests {
    use super::*;
    use mux_protocol::{ClientMessage, ServerHello, ServerMessage, read_frame, write_frame};
    use tokio::net::UnixListener;

    #[test]
    fn named_application_profiles_have_isolated_state_directories() {
        let product = default_state_dir().expect("product state directory");
        let preview = state_dir_for("MuxPreview").expect("preview state directory");

        assert_ne!(preview, product);
        assert!(preview.to_string_lossy().contains("MuxPreview"));
    }

    #[tokio::test]
    async fn incompatible_daemon_is_rejected_during_the_hello_exchange() {
        let directory = tempfile::tempdir().expect("temporary state directory");
        let socket = directory.path().join("daemon.sock");
        let listener = UnixListener::bind(&socket).expect("bind fake daemon");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept client");
            let hello: ClientMessage = read_frame(&mut stream).await.expect("read client hello");
            assert!(matches!(hello, ClientMessage::Hello(_)));
            write_frame(
                &mut stream,
                &ServerMessage::Hello(ServerHello {
                    protocol_version: PROTOCOL_VERSION - 1,
                    daemon_pid: 42,
                }),
            )
            .await
            .expect("write server hello");
        });

        let error = match Client::connect(&socket, "version-test").await {
            Ok(_) => panic!("old daemon must not be decoded by the new client"),
            Err(error) => error,
        };
        assert!(matches!(
            error,
            ClientError::ProtocolMismatch {
                client: PROTOCOL_VERSION,
                server,
            } if server == PROTOCOL_VERSION - 1
        ));
        server.await.expect("fake daemon task");
    }

    #[tokio::test]
    async fn terminal_input_does_not_wait_for_daemon_acknowledgement() {
        let directory = tempfile::tempdir().expect("temporary state directory");
        let socket = directory.path().join("daemon.sock");
        let listener = UnixListener::bind(&socket).expect("bind fake daemon");
        let pane_id = PaneId::new();
        let (release_ack, wait_for_release) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept client");
            let hello: ClientMessage = read_frame(&mut stream).await.expect("read client hello");
            assert!(matches!(hello, ClientMessage::Hello(_)));
            write_frame(
                &mut stream,
                &ServerMessage::Hello(ServerHello {
                    protocol_version: PROTOCOL_VERSION,
                    daemon_pid: 42,
                }),
            )
            .await
            .expect("write server hello");

            let input: ClientMessage = read_frame(&mut stream).await.expect("read input");
            assert_eq!(
                input,
                ClientMessage::Request {
                    request_id: UNACKNOWLEDGED_REQUEST_ID,
                    request: Request::WriteInput {
                        pane_id,
                        bytes: b"j".to_vec(),
                    },
                }
            );
            wait_for_release
                .await
                .expect("release delayed acknowledgement");
            // A protocol-v4 daemon may still acknowledge request zero. New
            // clients discard that compatibility response without disturbing
            // the event stream.
            write_frame(
                &mut stream,
                &ServerMessage::Response {
                    request_id: UNACKNOWLEDGED_REQUEST_ID,
                    response: Ok(Response::Ack),
                },
            )
            .await
            .expect("write delayed acknowledgement");
            write_frame(
                &mut stream,
                &ServerMessage::Event(ServerEvent::AgentResyncRequired),
            )
            .await
            .expect("write event after acknowledgement");
        });

        let mut client = Client::connect(&socket, "input-latency-test")
            .await
            .expect("connect client");
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            client.write_input(pane_id, b"j".to_vec()),
        )
        .await
        .expect("input waited for an acknowledgement")
        .expect("write input");
        release_ack.send(()).expect("release fake daemon");
        assert_eq!(
            client.next_event().await.expect("next event"),
            ServerEvent::AgentResyncRequired
        );
        server.await.expect("fake daemon task");
    }
}
