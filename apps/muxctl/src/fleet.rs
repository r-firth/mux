//! `muxctl fleet` and `muxctl hooks`: the coding agents in Mux's panes, for
//! scripts and for the agents themselves. An agent in a Mux pane can list
//! its fellows, hand one a prompt and wait for it to finish, with the
//! same view of them the window has.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use mux_client::Client;
use mux_fleet::install::{self, Homes, HookStatus};
use mux_fleet::{AgentKind, FleetAgent, FleetEvent, FleetState, Signal};
use mux_protocol::ServerEvent;
use mux_workspace::PaneId;

#[derive(Debug, Subcommand)]
pub enum FleetCommand {
    /// List the agents, the ones waiting on you first.
    List,
    /// Wait until an agent, or with no pane every agent, has stopped
    /// working. Exits 0 once it has, 2 if the time runs out.
    Wait {
        /// The agent's pane: its id, the start of it, or `this`.
        pane: Option<String>,
        /// What to wait for: `settled` (no longer working), or one state:
        /// `idle`, `working`, `blocked`, `done`, `failed`, `ended`.
        #[arg(long, default_value = "settled")]
        until: String,
        #[arg(long, default_value_t = 600)]
        timeout_secs: u64,
    },
    /// Type a prompt into an agent's pane and send it.
    Prompt {
        pane: String,
        /// The prompt; several words are joined with spaces.
        #[arg(required = true)]
        text: Vec<String>,
        /// Wait for the agent to finish the turn, as `wait` does.
        #[arg(long)]
        wait: bool,
        #[arg(long, default_value_t = 600)]
        timeout_secs: u64,
        /// Type even into an agent that is waiting on an answer, where the
        /// text would land in its question rather than its prompt.
        #[arg(long)]
        force: bool,
    },
    /// Print the end of what an agent's pane shows.
    Read {
        pane: String,
        #[arg(long, default_value_t = 60)]
        lines: u32,
    },
    /// Print each change to the fleet as a line of JSON, until stopped.
    Events,
    /// Say what Mux knows about the agent in a pane, and how it knows.
    Explain { pane: String },
    /// Let go of an ended session kept to pick up.
    Forget { pane: String },
}

#[derive(Debug, Subcommand)]
pub enum HooksCommand {
    /// Say which agents are here and whether Mux's hooks are in them.
    Status,
    /// Put Mux's hooks into these agents, or into every one found.
    Install { agents: Vec<String> },
    /// Take Mux's hooks out of these agents, or out of every one.
    Uninstall { agents: Vec<String> },
}

pub fn hooks(command: Option<HooksCommand>) -> Result<()> {
    let homes = Homes::from_environment().context("HOME is not set")?;
    let named = |agents: &[String]| -> Result<Vec<AgentKind>> {
        if agents.is_empty() {
            return Ok(AgentKind::HOOKED
                .into_iter()
                .filter(|kind| install::status(*kind, &homes) != HookStatus::Absent)
                .collect());
        }
        agents
            .iter()
            .map(|name| {
                AgentKind::from_command(name)
                    .filter(|kind| AgentKind::HOOKED.contains(kind))
                    .with_context(|| {
                        format!("Mux has no hooks for `{name}` (claude, codex, copilot)")
                    })
            })
            .collect()
    };
    match command.unwrap_or(HooksCommand::Status) {
        HooksCommand::Status => {
            for kind in AgentKind::HOOKED {
                let file = homes.hooks_file(kind).unwrap_or_default();
                println!(
                    "{:<12} {:<18} {}",
                    kind.name(),
                    install::status(kind, &homes).word(),
                    file.display()
                );
            }
        }
        HooksCommand::Install { agents } => {
            let kinds = named(&agents)?;
            if kinds.is_empty() {
                println!("No agents found to hook (Claude Code, Codex, Copilot).");
            }
            for kind in kinds {
                let changed = install::install(kind, &homes)?;
                println!(
                    "{}: {}",
                    kind.name(),
                    if changed { "hooked" } else { "already hooked" }
                );
                if changed && kind == AgentKind::Codex {
                    println!(
                        "  Codex runs a new hook only once you trust it: run /hooks in Codex, once."
                    );
                }
                if changed && kind == AgentKind::Copilot {
                    println!(
                        "  Copilot reads its hooks as it starts: restart any that is running."
                    );
                }
            }
        }
        HooksCommand::Uninstall { agents } => {
            for kind in named(&agents)? {
                let changed = install::uninstall(kind, &homes)?;
                println!(
                    "{}: {}",
                    kind.name(),
                    if changed {
                        "hooks removed"
                    } else {
                        "no hooks to remove"
                    }
                );
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_lines)] // Flat CLI dispatch, as in main.
pub async fn run(client: &mut Client, command: Option<FleetCommand>, json: bool) -> Result<()> {
    match command.unwrap_or(FleetCommand::List) {
        FleetCommand::List => {
            let agents = client.list_fleet().await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&agents)?);
            } else if agents.is_empty() {
                println!("No agents are running in Mux.");
            } else {
                print_table(&agents);
            }
        }
        FleetCommand::Wait {
            pane,
            until,
            timeout_secs,
        } => {
            let until = Until::parse(&until)?;
            let pane_id = match pane {
                Some(pane) => Some(resolve(client, &pane).await?.pane_id),
                None => None,
            };
            let reached = wait(client, pane_id, until, false, timeout_secs).await?;
            report_wait(reached.as_deref(), json)?;
        }
        FleetCommand::Prompt {
            pane,
            text,
            wait: and_wait,
            timeout_secs,
            force,
        } => {
            let agent = resolve(client, &pane).await?;
            if !agent.state.is_live() {
                bail!(
                    "the {} in that pane has ended{}",
                    agent.kind.name(),
                    agent
                        .resume_command()
                        .map(|command| format!("; pick it up with `{command}`"))
                        .unwrap_or_default()
                );
            }
            if agent.state == FleetState::Blocked && !force {
                bail!(
                    "{} is waiting on an answer{}; a prompt typed now would land in its question. \
                     Answer it, or pass --force",
                    agent.kind.name(),
                    agent
                        .message
                        .as_deref()
                        .map(|message| format!(" ({message})"))
                        .unwrap_or_default()
                );
            }
            let text = text.join(" ");
            client.write_input(agent.pane_id, typed(&text)).await?;
            // Sent apart from the text, as a key pressed after a paste is.
            tokio::time::sleep(Duration::from_millis(80)).await;
            client.write_input(agent.pane_id, b"\r".to_vec()).await?;
            if and_wait {
                let reached = wait(
                    client,
                    Some(agent.pane_id),
                    Until::Settled,
                    true,
                    timeout_secs,
                )
                .await?;
                report_wait(reached.as_deref(), json)?;
            } else if !json {
                println!("sent to {}", agent.kind.name());
            }
        }
        FleetCommand::Read { pane, lines } => {
            let pane_id = match pane.parse::<PaneId>() {
                // Any pane can be read, agent or not.
                Ok(pane_id) => pane_id,
                Err(_) => resolve(client, &pane).await?.pane_id,
            };
            println!("{}", client.pane_text(pane_id, lines).await?);
        }
        FleetCommand::Events => loop {
            if let ServerEvent::Fleet(event) = client.next_event().await? {
                println!("{}", serde_json::to_string(&event)?);
            }
        },
        FleetCommand::Explain { pane } => {
            let agent = resolve(client, &pane).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&agent)?);
            } else {
                explain(&agent);
            }
        }
        FleetCommand::Forget { pane } => {
            let agent = resolve(client, &pane).await?;
            if agent.state.is_live() {
                bail!("that {} is still running", agent.kind.name());
            }
            client.forget_fleet_agent(agent.pane_id).await?;
        }
    }
    Ok(())
}

/// The agent a pane argument names: a pane id, the start of one, or
/// `this` for the pane muxctl was run in.
async fn resolve(client: &mut Client, pane: &str) -> Result<FleetAgent> {
    let wanted = if pane == "this" || pane == "." {
        std::env::var("MUX_PANE_ID").context("`this` only means something inside a Mux pane")?
    } else {
        pane.to_lowercase()
    };
    let agents = client.list_fleet().await?;
    let mut matching = agents
        .into_iter()
        .filter(|agent| agent.pane_id.to_string().starts_with(&wanted));
    match (matching.next(), matching.next()) {
        (Some(agent), None) => Ok(agent),
        (None, _) => bail!("no agent is in a pane named `{pane}` (see `muxctl fleet`)"),
        (Some(_), Some(_)) => bail!("`{pane}` names more than one pane; give more of the id"),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Until {
    /// No longer working: waiting on the user, done, idle, failed or gone.
    Settled,
    State(FleetState),
}

impl Until {
    fn parse(word: &str) -> Result<Self> {
        Ok(match word {
            "settled" => Self::Settled,
            "idle" => Self::State(FleetState::Idle),
            "working" => Self::State(FleetState::Working),
            "blocked" => Self::State(FleetState::Blocked),
            "done" => Self::State(FleetState::Done),
            "failed" | "error" => Self::State(FleetState::Error),
            "ended" => Self::State(FleetState::Ended),
            other => bail!(
                "cannot wait until `{other}`: settled, idle, working, blocked, done, failed or ended"
            ),
        })
    }

    fn met(self, agent: &FleetAgent) -> bool {
        match self {
            // An agent that says nothing of itself can't be said to have
            // settled; one seen only by its process counts as working.
            Self::Settled => !matches!(agent.state, FleetState::Working | FleetState::Unknown),
            // Done is done whether or not it has since been looked at.
            Self::State(FleetState::Done) => {
                matches!(agent.state, FleetState::Done | FleetState::Idle)
            }
            Self::State(state) => agent.state == state,
        }
    }
}

/// Wait for one pane's agent, or for all of them, to meet `until`. After
/// a prompt (`after_prompt`) the agent must first be seen to take it up,
/// so the state it was in before is not mistaken for the turn's end.
/// Gives the agents as they stood, or nothing if the time ran out.
async fn wait(
    client: &mut Client,
    pane_id: Option<PaneId>,
    until: Until,
    after_prompt: bool,
    timeout_secs: u64,
) -> Result<Option<Vec<FleetAgent>>> {
    /// How long an agent is given to take a prompt up.
    const TAKE_UP: Duration = Duration::from_secs(20);
    let started = tokio::time::Instant::now();
    let deadline = started + Duration::from_secs(timeout_secs);
    let mut agents = client.list_fleet().await?;
    let mut taken_up = !after_prompt;
    loop {
        let watched: Vec<_> = agents
            .iter()
            .filter(|agent| pane_id.is_none_or(|pane_id| agent.pane_id == pane_id))
            .filter(|agent| agent.state.is_live() || pane_id.is_some())
            .cloned()
            .collect();
        if pane_id.is_some() && watched.is_empty() {
            // It exited with no session to keep: as settled as can be.
            return Ok(Some(Vec::new()));
        }
        taken_up |= watched
            .iter()
            .any(|agent| matches!(agent.state, FleetState::Working | FleetState::Blocked));
        if taken_up && watched.iter().all(|agent| until.met(agent)) {
            return Ok(Some(watched));
        }
        if !taken_up && started.elapsed() > TAKE_UP {
            bail!("the agent did not take the prompt up; read its pane with `muxctl fleet read`");
        }
        let next = if taken_up {
            deadline
        } else {
            (started + TAKE_UP).min(deadline)
        };
        match tokio::time::timeout_at(next, client.next_event()).await {
            Ok(Ok(ServerEvent::Fleet(FleetEvent::Changed(agent)))) => {
                agents.retain(|known| known.pane_id != agent.pane_id);
                agents.push(*agent);
            }
            Ok(Ok(ServerEvent::Fleet(FleetEvent::Removed { pane_id }))) => {
                agents.retain(|known| known.pane_id != pane_id);
            }
            Ok(Err(error)) => return Err(error.into()),
            Err(_) if tokio::time::Instant::now() >= deadline => return Ok(None),
            // Some other event, or the wait to take the prompt up ran out.
            Ok(Ok(_)) | Err(_) => {}
        }
    }
}

fn report_wait(reached: Option<&[FleetAgent]>, json: bool) -> Result<()> {
    let Some(agents) = reached else {
        eprintln!("timed out");
        std::process::exit(2);
    };
    if json {
        println!("{}", serde_json::to_string_pretty(agents)?);
    } else {
        print_table(agents);
    }
    Ok(())
}

/// A prompt as bytes to type. One of several lines goes as a paste, so
/// its line breaks are taken as text and not as sending it early.
fn typed(text: &str) -> Vec<u8> {
    let text: String = text
        .chars()
        .filter(|character| *character == '\n' || !character.is_control())
        .collect();
    if text.contains('\n') {
        format!("\x1b[200~{text}\x1b[201~").into_bytes()
    } else {
        text.into_bytes()
    }
}

fn print_table(agents: &[FleetAgent]) {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        });
    for agent in agents {
        let pane = agent.pane_id.to_string();
        let place = match &agent.project.branch {
            Some(branch) => format!("{}:{branch}", agent.project.name),
            None => agent.project.name.clone(),
        };
        let line = agent
            .headline()
            .or(agent.task.as_deref())
            .unwrap_or_default();
        println!(
            "{}  {:<12} {:<15} {:>5}  {:<28} {}",
            &pane[..pane.len().min(8)],
            agent.kind.name(),
            agent.state.word(),
            age(now_ms.saturating_sub(agent.since_ms)),
            place,
            line
        );
    }
}

/// How long ago, in the largest unit that fits.
fn age(milliseconds: u64) -> String {
    let seconds = milliseconds / 1_000;
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3_600 => format!("{}m", seconds / 60),
        3_600..86_400 => format!("{}h", seconds / 3_600),
        _ => format!("{}d", seconds / 86_400),
    }
}

fn explain(agent: &FleetAgent) {
    println!("{} in pane {}", agent.kind.name(), agent.pane_id);
    println!("  state     {}", agent.state.word());
    println!(
        "  project   {}{}",
        agent.project.name,
        agent
            .project
            .branch
            .as_deref()
            .map(|branch| format!(" on {branch}"))
            .unwrap_or_default()
    );
    println!("  directory {}", agent.cwd.display());
    if let Some(task) = &agent.task {
        println!("  task      {task}");
    }
    if let Some(line) = agent.headline() {
        println!("  now       {line}");
    }
    if let Some(session) = &agent.agent_session {
        println!("  session   {session}");
    }
    if let Some(command) = agent.resume_command() {
        println!("  resume    {command}");
    }
    let hooked = Homes::from_environment().map(|homes| install::status(agent.kind, &homes));
    println!(
        "  known by  {}",
        match agent.signal {
            Signal::Hooks => "Mux's hooks in the agent: each prompt, tool, question and turn's end",
            Signal::Status => "the agent's own status reports (OSC 7501): working, waiting, done",
            Signal::Notice =>
                "its notifications only: Mux can tell when it wants you, not when it works",
            Signal::Process => "its process only: Mux can tell it is there, not what it is doing",
        }
    );
    if agent.signal != Signal::Hooks {
        match hooked {
            Some(HookStatus::Installed) if agent.kind == AgentKind::Codex => println!(
                "  hooks     installed, but nothing heard: run /hooks in Codex to trust them"
            ),
            Some(HookStatus::Installed) => println!(
                "  hooks     installed, but nothing heard: an agent started before they were \
                 reads them when next started"
            ),
            Some(HookStatus::Missing | HookStatus::Outdated) => println!(
                "  hooks     not installed: `muxctl hooks install {}` gives its task, its \
                 questions and a session to resume",
                agent.kind.command()
            ),
            _ => println!("  hooks     Mux has none for {}", agent.kind.name()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prompt_of_several_lines_is_typed_as_a_paste() {
        assert_eq!(typed("run the tests"), b"run the tests");
        assert_eq!(typed("a\nb"), b"\x1b[200~a\nb\x1b[201~");
        // Nothing in a prompt can act as a key or an escape.
        assert_eq!(typed("a\x1b[201~\rb\x03"), b"a[201~b");
    }

    #[test]
    fn ages_are_said_in_the_largest_unit_that_fits() {
        assert_eq!(age(900), "0s");
        assert_eq!(age(59_000), "59s");
        assert_eq!(age(61_000), "1m");
        assert_eq!(age(2 * 3_600_000), "2h");
        assert_eq!(age(3 * 86_400_000), "3d");
    }

    #[test]
    fn done_is_met_by_an_agent_since_looked_at() {
        assert_eq!(
            Until::parse("failed").ok(),
            Some(Until::State(FleetState::Error))
        );
        assert!(Until::parse("soon").is_err());
    }
}
