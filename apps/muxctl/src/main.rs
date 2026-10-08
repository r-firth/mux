use std::io::Write as _;
use std::path::PathBuf;
use std::str::FromStr as _;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use mux_acp::{
    AgentConfigValueSelection, AgentContext, AgentContextKind, AgentPrompt, AgentSessionStatus,
    AgentSpec, AgentTimelineItem,
};
use mux_client::{Client, default_state_dir, socket_path};
use mux_protocol::{CreateSession, ServerEvent, SessionSelector, SpawnCommand};
use mux_terminal::TerminalSize;
use mux_workspace::{AgentSessionId, Direction, PaneId, SessionId, SplitAxis, WorkspaceCommand};
use tokio::io::AsyncReadExt as _;

#[derive(Debug, Parser)]
#[command(
    name = "muxctl",
    about = "Diagnostic client for the persistent mux workspace daemon"
)]
struct Arguments {
    #[arg(long, global = true)]
    state_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Verify that the daemon is accepting requests.
    Health,
    /// List live sessions.
    List,
    /// Create a session containing independent PTYs.
    New {
        #[arg(long)]
        name: String,
        #[arg(long, default_value_t = 2)]
        panes: u16,
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[arg(long)]
        program: Option<PathBuf>,
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Print the current attach snapshot as JSON.
    Inspect { session: String },
    /// Attach stdin/stdout to one pane as a development harness.
    Attach {
        session: String,
        #[arg(long)]
        pane: Option<PaneId>,
    },
    /// Type text into a pane (the focused one unless --pane is given).
    Type {
        session: String,
        #[arg(long)]
        pane: Option<PaneId>,
        /// Pause between characters, as if typed by hand.
        #[arg(long, default_value_t = 0)]
        delay_ms: u64,
        /// Press Return after the text.
        #[arg(long)]
        enter: bool,
        text: String,
    },
    /// Change the workspace as the window's keys would.
    Do { session: String, action: Action },
    /// Rename the active tab.
    RenameTab { session: String, name: String },
    /// Rename a live session.
    Rename { session: SessionId, name: String },
    /// Kill a live session and all of its PTYs.
    Kill { session: SessionId },
    /// List daemon-owned ACP agent sessions.
    AgentList,
    /// Turn a pane (the focused one unless --pane is given) into an agent pane.
    AgentPane {
        session: String,
        #[arg(long)]
        pane: Option<PaneId>,
        #[arg(long, value_enum, default_value_t = Agent::Claude)]
        agent: Agent,
        /// Working directory for the agent (the pane's own by default).
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    /// Wait until an agent is idle, having ended at least --turns prompt turns.
    AgentWait {
        session: AgentSessionId,
        #[arg(long, default_value_t = 0)]
        turns: usize,
        #[arg(long, default_value_t = 60)]
        timeout_secs: u64,
    },
    /// Start a persistent Codex ACP session.
    AgentNew {
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    /// Send a prompt to a persistent ACP agent session.
    AgentPrompt {
        session: AgentSessionId,
        /// Diagnostic terminal context attached as a separate ACP content block.
        #[arg(long)]
        terminal_context: Option<String>,
        prompt: String,
    },
    /// Cancel the active turn in an ACP agent session.
    AgentCancel { session: AgentSessionId },
    /// Run an agent-advertised ACP authentication method.
    AgentLogin {
        session: AgentSessionId,
        method: String,
    },
    /// Set an ACP session mode.
    AgentMode {
        session: AgentSessionId,
        mode: String,
    },
    /// Set an ACP session configuration option.
    AgentConfig {
        session: AgentSessionId,
        config: String,
        value: String,
        #[arg(long)]
        boolean: bool,
    },
    /// End the external process for an ACP agent session.
    AgentEnd { session: AgentSessionId },
}

#[tokio::main]
#[allow(clippy::too_many_lines)] // Flat CLI dispatch is clearer than one wrapper per subcommand.
async fn main() -> Result<()> {
    let arguments = Arguments::parse();
    let state_dir = arguments
        .state_dir
        .or_else(default_state_dir)
        .context("could not determine a per-user state directory")?;
    let socket = socket_path(&state_dir);
    let mut client = Client::connect(&socket, "muxctl")
        .await
        .with_context(|| format!("connect to daemon at {}", socket.display()))?;

    match arguments.command {
        Command::Health => {
            client.health().await?;
            println!("muxd {} is healthy", client.daemon_pid());
        }
        Command::List => {
            for session in client.list_sessions().await? {
                println!(
                    "{}\t{}\t{} panes",
                    session.id, session.name, session.pane_count
                );
            }
        }
        Command::New {
            name,
            panes,
            cwd,
            program,
            args,
        } => {
            let cwd = cwd.unwrap_or(std::env::current_dir()?);
            let program = program
                .or_else(|| std::env::var_os("SHELL").map(PathBuf::from))
                .unwrap_or_else(|| PathBuf::from("/bin/sh"));
            let session = client
                .create_session(CreateSession {
                    name,
                    cwd,
                    command: SpawnCommand {
                        program,
                        args,
                        environment: Vec::new(),
                    },
                    initial_panes: panes,
                    initial_size: TerminalSize::default(),
                })
                .await?;
            println!(
                "{}\t{}\t{} panes",
                session.id, session.name, session.pane_count
            );
        }
        Command::Inspect { session } => {
            let attachment = client.attach(parse_session_selector(&session)).await?;
            println!("{}", serde_json::to_string_pretty(&attachment)?);
        }
        Command::Attach { session, pane } => {
            attach(&mut client, parse_session_selector(&session), pane).await?;
        }
        Command::Type {
            session,
            pane,
            delay_ms,
            enter,
            text,
        } => {
            let attachment = client.attach(parse_session_selector(&session)).await?;
            let pane_id = match pane {
                Some(pane) => pane,
                None => {
                    attachment
                        .session
                        .active_tab()
                        .context("attached session has no active tab")?
                        .focused_pane
                }
            };
            let mut text = text;
            if enter {
                text.push('\r');
            }
            if delay_ms == 0 {
                client.write_input(pane_id, text.into_bytes()).await?;
            } else {
                let delay = std::time::Duration::from_millis(delay_ms);
                for character in text.chars() {
                    client
                        .write_input(pane_id, character.to_string().into_bytes())
                        .await?;
                    tokio::time::sleep(delay).await;
                }
            }
            // The input is sent unacknowledged; a request after it returns
            // once the daemon has taken it.
            client.health().await?;
        }
        Command::Do { session, action } => {
            let attachment = client.attach(parse_session_selector(&session)).await?;
            client
                .workspace_command(attachment.session.id, action.command())
                .await?;
        }
        Command::RenameTab { session, name } => {
            let attachment = client.attach(parse_session_selector(&session)).await?;
            client
                .workspace_command(attachment.session.id, WorkspaceCommand::RenameTab(name))
                .await?;
        }
        Command::Rename { session, name } => {
            client.rename_session(session, name.clone()).await?;
            println!("session renamed to {name}");
        }
        Command::Kill { session } => {
            client.kill_session(session).await?;
            println!("session killed");
        }
        Command::AgentList => {
            println!(
                "{}",
                serde_json::to_string_pretty(&client.list_agent_sessions().await?)?
            );
        }
        Command::AgentPane {
            session,
            pane,
            agent,
            cwd,
        } => {
            let attachment = client.attach(parse_session_selector(&session)).await?;
            let pane_id = match pane {
                Some(pane) => pane,
                None => {
                    attachment
                        .session
                        .active_tab()
                        .context("attached session has no active tab")?
                        .focused_pane
                }
            };
            let spec = agent.spec().resolve_runtime_environment();
            let agent = client.start_agent_for_pane(spec, pane_id, cwd).await?;
            println!("{}", agent.id);
        }
        Command::AgentWait {
            session,
            turns,
            timeout_secs,
        } => {
            agent_wait(&mut client, session, turns, timeout_secs).await?;
        }
        Command::AgentNew { cwd } => {
            let cwd = cwd.unwrap_or(std::env::current_dir()?);
            let agent = client.start_agent(AgentSpec::codex(), cwd).await?;
            println!("{}", serde_json::to_string_pretty(&agent)?);
        }
        Command::AgentPrompt {
            session,
            terminal_context,
            prompt,
        } => {
            let context = terminal_context.map_or_else(Vec::new, |text| {
                vec![AgentContext {
                    kind: AgentContextKind::TerminalViewport,
                    pane_id: PaneId::new(),
                    label: "muxctl diagnostic terminal context".to_owned(),
                    text,
                }]
            });
            client
                .prompt_agent_with_context(
                    session,
                    AgentPrompt {
                        text: prompt,
                        context,
                        files: Vec::new(),
                    },
                )
                .await?;
            println!("prompt accepted");
        }
        Command::AgentCancel { session } => {
            client.cancel_agent(session).await?;
            println!("cancel requested");
        }
        Command::AgentLogin { session, method } => {
            client.authenticate_agent(session, method).await?;
            println!("authentication started");
        }
        Command::AgentMode { session, mode } => {
            client.set_agent_mode(session, mode).await?;
            println!("mode update requested");
        }
        Command::AgentConfig {
            session,
            config,
            value,
            boolean,
        } => {
            let value = if boolean {
                AgentConfigValueSelection::Boolean(value.parse()?)
            } else {
                AgentConfigValueSelection::Choice(value)
            };
            client.set_agent_config(session, config, value).await?;
            println!("configuration update requested");
        }
        Command::AgentEnd { session } => {
            client.close_agent(session).await?;
            println!("agent session ended");
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Agent {
    Claude,
    Codex,
    Gemini,
    Copilot,
}

impl Agent {
    fn spec(self) -> AgentSpec {
        match self {
            Self::Claude => AgentSpec::claude(),
            Self::Codex => AgentSpec::codex(),
            Self::Gemini => AgentSpec::gemini(),
            Self::Copilot => AgentSpec::copilot(),
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Action {
    SplitRight,
    SplitDown,
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
    ClosePane,
    Zoom,
    NewTab,
    CloseTab,
    NextTab,
    PreviousTab,
}

impl Action {
    const fn command(self) -> WorkspaceCommand {
        match self {
            Self::SplitRight => WorkspaceCommand::SplitPane(SplitAxis::Horizontal),
            Self::SplitDown => WorkspaceCommand::SplitPane(SplitAxis::Vertical),
            Self::FocusLeft => WorkspaceCommand::FocusPane(Direction::Left),
            Self::FocusRight => WorkspaceCommand::FocusPane(Direction::Right),
            Self::FocusUp => WorkspaceCommand::FocusPane(Direction::Up),
            Self::FocusDown => WorkspaceCommand::FocusPane(Direction::Down),
            Self::ClosePane => WorkspaceCommand::ClosePane,
            Self::Zoom => WorkspaceCommand::TogglePaneZoom,
            Self::NewTab => WorkspaceCommand::NewTab,
            Self::CloseTab => WorkspaceCommand::CloseTab,
            Self::NextTab => WorkspaceCommand::NextTab,
            Self::PreviousTab => WorkspaceCommand::PreviousTab,
        }
    }
}

async fn agent_wait(
    client: &mut Client,
    session: AgentSessionId,
    turns: usize,
    timeout_secs: u64,
) -> Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    loop {
        let sessions = client.list_agent_sessions().await?;
        let agent = sessions
            .iter()
            .find(|agent| agent.id == session)
            .with_context(|| format!("agent session not found: {session}"))?;
        let ended = agent
            .timeline
            .iter()
            .filter(|item| matches!(item, AgentTimelineItem::TurnEnded { .. }))
            .count();
        match agent.status {
            AgentSessionStatus::Idle if ended >= turns => return Ok(()),
            AgentSessionStatus::Starting
            | AgentSessionStatus::Authenticating
            | AgentSessionStatus::Idle
            | AgentSessionStatus::Working => {}
            status => bail!("agent is {status:?}"),
        }
        if std::time::Instant::now() >= deadline {
            bail!("agent still {:?} after {timeout_secs}s", agent.status);
        }
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }
}

async fn attach(
    client: &mut Client,
    selector: SessionSelector,
    requested_pane: Option<PaneId>,
) -> Result<()> {
    let attachment = client.attach(selector).await?;
    let focused = attachment
        .session
        .active_tab()
        .context("attached session has no active tab")?
        .focused_pane;
    let pane_id = requested_pane.unwrap_or(focused);
    let pane = attachment
        .panes
        .iter()
        .find(|pane| pane.pane_id == pane_id)
        .with_context(|| format!("pane {pane_id} is not in the session"))?;

    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    for chunk in &pane.terminal.replay {
        output.write_all(&chunk.bytes)?;
    }
    output.flush()?;
    drop(output);

    let mut stdin = tokio::io::stdin();
    let mut input = vec![0_u8; 16 * 1024];
    loop {
        enum Next {
            Input(std::io::Result<usize>),
            Event(Box<ServerEvent>),
            EventError(mux_client::ClientError),
        }

        let next = tokio::select! {
            result = stdin.read(&mut input) => Next::Input(result),
            result = client.next_event() => match result {
                Ok(event) => Next::Event(Box::new(event)),
                Err(error) => Next::EventError(error),
            },
        };

        match next {
            Next::Input(Ok(0)) => return Ok(()),
            Next::Input(Ok(length)) => {
                client
                    .write_input(pane_id, input[..length].to_vec())
                    .await?;
            }
            Next::Input(Err(error)) => return Err(error.into()),
            Next::Event(event) => match *event {
                ServerEvent::PaneOutput {
                    pane_id: event_pane,
                    bytes,
                    ..
                } if event_pane == pane_id => {
                    let stdout = std::io::stdout();
                    let mut output = stdout.lock();
                    output.write_all(&bytes)?;
                    output.flush()?;
                }
                ServerEvent::PaneExited {
                    pane_id: event_pane,
                    status,
                    ..
                } if event_pane == pane_id => {
                    eprintln!(
                        "\n[pane exited: code={:?}, success={}]",
                        status.code, status.success
                    );
                    return Ok(());
                }
                ServerEvent::ResyncRequired { .. } => {
                    bail!("client fell behind the daemon stream; reattach to resynchronize");
                }
                _ => {}
            },
            Next::EventError(error) => return Err(error.into()),
        }
    }
}

fn parse_session_selector(value: &str) -> SessionSelector {
    SessionId::from_str(value).map_or_else(
        |_| SessionSelector::Name(value.to_owned()),
        SessionSelector::Id,
    )
}
