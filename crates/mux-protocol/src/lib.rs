//! Versioned local IPC messages and framing.

use std::io;
use std::path::PathBuf;

use mux_acp::{
    AgentConfigValueSelection, AgentEvent, AgentPrompt, AgentSessionSnapshot, AgentSpec,
};
use mux_terminal::{TerminalAttachment, TerminalSize};
use mux_workspace::{AgentSessionId, PaneId, Session, SessionId, WorkspaceCommand};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub const PROTOCOL_VERSION: u16 = 1;
pub const MAX_FRAME_LENGTH: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ClientMessage {
    Hello(ClientHello),
    Request { request_id: u64, request: Request },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ClientHello {
    pub protocol_version: u16,
    pub client_name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ServerMessage {
    Hello(ServerHello),
    Response {
        request_id: u64,
        response: Result<Response, RemoteError>,
    },
    Event(ServerEvent),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ServerHello {
    pub protocol_version: u16,
    pub daemon_pid: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Request {
    Health,
    ListSessions,
    CreateSession(CreateSession),
    AttachSession {
        session: SessionSelector,
    },
    WriteInput {
        pane_id: PaneId,
        bytes: Vec<u8>,
    },
    ResizePane {
        pane_id: PaneId,
        size: TerminalSize,
    },
    WorkspaceCommand {
        session_id: SessionId,
        command: WorkspaceCommand,
    },
    ListAgentSessions,
    StartAgent {
        spec: AgentSpec,
        cwd: PathBuf,
    },
    PromptAgent {
        session_id: AgentSessionId,
        prompt: AgentPrompt,
    },
    ResolveAgentPermission {
        session_id: AgentSessionId,
        request_id: String,
        option_id: Option<String>,
    },
    CancelAgent {
        session_id: AgentSessionId,
    },
    CloseAgent {
        session_id: AgentSessionId,
    },
    StartAgentForPane {
        spec: AgentSpec,
        pane_id: PaneId,
    },
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CreateSession {
    pub name: String,
    pub cwd: PathBuf,
    pub command: SpawnCommand,
    pub initial_panes: u16,
    pub initial_size: TerminalSize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SpawnCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub environment: Vec<(String, String)>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SessionSelector {
    Id(SessionId),
    Name(String),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Response {
    Pong,
    Sessions(Vec<SessionSummary>),
    SessionCreated(SessionSummary),
    Attached(SessionAttachment),
    AgentSessions(Vec<AgentSessionSnapshot>),
    AgentStarted(AgentSessionSnapshot),
    Ack,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SessionSummary {
    pub id: SessionId,
    pub name: String,
    pub pane_count: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SessionAttachment {
    pub session: Session,
    pub panes: Vec<PaneAttachment>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PaneAttachment {
    pub pane_id: PaneId,
    pub terminal: TerminalAttachment,
    pub exit_status: Option<ProcessExit>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ServerEvent {
    PaneOutput {
        session_id: SessionId,
        pane_id: PaneId,
        sequence: u64,
        bytes: Vec<u8>,
    },
    PaneExited {
        session_id: SessionId,
        pane_id: PaneId,
        status: ProcessExit,
    },
    ResyncRequired {
        session_id: SessionId,
    },
    WorkspaceChanged {
        session_id: SessionId,
    },
    Agent(AgentEvent),
    AgentResyncRequired,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProcessExit {
    pub code: Option<u32>,
    pub success: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ErrorCode {
    InvalidRequest,
    NotFound,
    Conflict,
    Internal,
    ProtocolMismatch,
}

#[derive(Clone, Debug, Deserialize, Eq, Error, PartialEq, Serialize)]
#[error("{code:?}: {message}")]
pub struct RemoteError {
    pub code: ErrorCode,
    pub message: String,
}

impl RemoteError {
    #[must_use]
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

pub async fn write_frame<T, W>(writer: &mut W, value: &T) -> Result<(), CodecError>
where
    T: Serialize,
    W: AsyncWrite + Unpin,
{
    let payload = postcard::to_allocvec(value)?;
    if payload.len() > MAX_FRAME_LENGTH {
        return Err(CodecError::FrameTooLarge(payload.len()));
    }
    let length =
        u32::try_from(payload.len()).map_err(|_| CodecError::FrameTooLarge(payload.len()))?;
    writer.write_u32(length).await?;
    writer.write_all(&payload).await?;
    writer.flush().await?;
    Ok(())
}

pub async fn read_frame<T, R>(reader: &mut R) -> Result<T, CodecError>
where
    T: DeserializeOwned,
    R: AsyncRead + Unpin,
{
    let length = reader.read_u32().await? as usize;
    if length > MAX_FRAME_LENGTH {
        return Err(CodecError::FrameTooLarge(length));
    }
    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload).await?;
    Ok(postcard::from_bytes(&payload)?)
}

/// Owns an uninterrupted framed read loop and exposes complete messages over
/// a cancellation-safe channel. Calling `read_frame` directly inside
/// `tokio::select!` can discard a partially read length or payload when another
/// branch wins, permanently desynchronizing the byte stream.
pub struct FrameReader<T> {
    receiver: mpsc::UnboundedReceiver<Result<T, CodecError>>,
    task: JoinHandle<()>,
}

impl<T> FrameReader<T>
where
    T: DeserializeOwned + Send + 'static,
{
    pub fn spawn<R>(mut reader: R) -> Self
    where
        R: AsyncRead + Unpin + Send + 'static,
    {
        let (sender, receiver) = mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            loop {
                let message = read_frame(&mut reader).await;
                let failed = message.is_err();
                if sender.send(message).is_err() || failed {
                    break;
                }
            }
        });
        Self { receiver, task }
    }

    pub async fn next(&mut self) -> Option<Result<T, CodecError>> {
        self.receiver.recv().await
    }
}

impl<T> Drop for FrameReader<T> {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Debug, Error)]
pub enum CodecError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("invalid protocol payload: {0}")]
    Postcard(#[from] postcard::Error),
    #[error("protocol frame is too large: {0} bytes")]
    FrameTooLarge(usize),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn framed_messages_round_trip_binary_terminal_bytes() {
        let (mut writer, mut reader) = tokio::io::duplex(1024);
        let expected = ClientMessage::Request {
            request_id: 7,
            request: Request::WriteInput {
                pane_id: PaneId::new(),
                bytes: vec![0, 1, 2, 255],
            },
        };

        write_frame(&mut writer, &expected).await.expect("write");
        let actual: ClientMessage = read_frame(&mut reader).await.expect("read");
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn framed_reader_survives_cancellation_after_a_partial_header() {
        use std::time::Duration;

        let (mut writer, reader) = tokio::io::duplex(1024);
        let expected = ClientMessage::Request {
            request_id: 11,
            request: Request::Health,
        };
        let payload = postcard::to_allocvec(&expected).expect("serialize message");
        let header = u32::try_from(payload.len())
            .expect("small test payload")
            .to_be_bytes();
        let mut frames = FrameReader::<ClientMessage>::spawn(reader);

        writer
            .write_all(&header[..2])
            .await
            .expect("partial header");
        assert!(
            tokio::time::timeout(Duration::from_millis(5), frames.next())
                .await
                .is_err()
        );
        writer
            .write_all(&header[2..])
            .await
            .expect("rest of header");
        writer.write_all(&payload).await.expect("payload");

        assert_eq!(
            frames.next().await.expect("reader active").expect("frame"),
            expected
        );
    }
}
