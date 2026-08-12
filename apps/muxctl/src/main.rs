use std::io::Write as _;
use std::path::PathBuf;
use std::str::FromStr as _;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use mux_acp::{AgentConfigValueSelection, AgentContext, AgentContextKind, AgentPrompt, AgentSpec};
use mux_client::{Client, default_state_dir, socket_path};
use mux_protocol::{CreateSession, ServerEvent, SessionSelector, SpawnCommand};
use mux_terminal::TerminalSize;
use mux_workspace::{AgentSessionId, PaneId, SessionId};
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
    /// List daemon-owned ACP agent sessions.
    AgentList,
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
        Command::AgentList => {
            println!(
                "{}",
                serde_json::to_string_pretty(&client.list_agent_sessions().await?)?
            );
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
                    },
                )
                .await?;
            println!("prompt accepted");
        }
        Command::AgentCancel { session } => {
            client.cancel_agent(session).await?;
            println!("cancel requested");
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
            Event(Result<ServerEvent, mux_client::ClientError>),
        }

        let next = tokio::select! {
            result = stdin.read(&mut input) => Next::Input(result),
            result = client.next_event() => Next::Event(result),
        };

        match next {
            Next::Input(Ok(0)) => return Ok(()),
            Next::Input(Ok(length)) => {
                client
                    .write_input(pane_id, input[..length].to_vec())
                    .await?;
            }
            Next::Input(Err(error)) => return Err(error.into()),
            Next::Event(Ok(ServerEvent::PaneOutput {
                pane_id: event_pane,
                bytes,
                ..
            })) if event_pane == pane_id => {
                let stdout = std::io::stdout();
                let mut output = stdout.lock();
                output.write_all(&bytes)?;
                output.flush()?;
            }
            Next::Event(Ok(ServerEvent::PaneExited {
                pane_id: event_pane,
                status,
                ..
            })) if event_pane == pane_id => {
                eprintln!(
                    "\n[pane exited: code={:?}, success={}]",
                    status.code, status.success
                );
                return Ok(());
            }
            Next::Event(Ok(ServerEvent::ResyncRequired { .. })) => {
                bail!("client fell behind the daemon stream; reattach to resynchronize");
            }
            Next::Event(Ok(_)) => {}
            Next::Event(Err(error)) => return Err(error.into()),
        }
    }
}

fn parse_session_selector(value: &str) -> SessionSelector {
    SessionId::from_str(value).map_or_else(
        |_| SessionSelector::Name(value.to_owned()),
        SessionSelector::Id,
    )
}
