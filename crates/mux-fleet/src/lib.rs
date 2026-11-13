//! The coding agents running in Mux's panes, and what each is doing.
//!
//! An agent is a fact about a terminal pane: `claude`, `codex` or `copilot`
//! run from a shell is the agent, in its own interface. Mux learns about it
//! from what the agent itself says, never from reading its screen:
//!
//! - the hooks Mux installs into the agent, which report each prompt, tool,
//!   request for permission and turn's end, with the agent's session;
//! - the program status protocol (OSC 7501), for agents that speak it;
//! - a desktop notification or a bell, for agents that say nothing else;
//! - the process in front of the pane's shell, which says an agent is there
//!   and when it has gone.
//!
//! [`Fleet`] keeps one [`FleetAgent`] for each pane with an agent in it and
//! takes those signals in as they come, the latest winning. It keeps no
//! clock and touches no files beyond a pane's directory, so the daemon that
//! owns it says when things happened.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use mux_workspace::{PaneId, SessionId};
use serde::{Deserialize, Serialize};

pub mod hooks;
pub mod install;
mod project;

pub use project::Project;

/// A coding agent Mux knows by name.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    Claude,
    Codex,
    Copilot,
    Gemini,
    OpenCode,
    Cursor,
    Amp,
    Aider,
    Goose,
    Droid,
}

impl AgentKind {
    pub const ALL: [Self; 10] = [
        Self::Claude,
        Self::Codex,
        Self::Copilot,
        Self::Gemini,
        Self::OpenCode,
        Self::Cursor,
        Self::Amp,
        Self::Aider,
        Self::Goose,
        Self::Droid,
    ];

    /// The agents Mux installs hooks into.
    pub const HOOKED: [Self; 3] = [Self::Claude, Self::Codex, Self::Copilot];

    /// The name for people.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
            Self::Copilot => "Copilot",
            Self::Gemini => "Gemini",
            Self::OpenCode => "OpenCode",
            Self::Cursor => "Cursor Agent",
            Self::Amp => "Amp",
            Self::Aider => "Aider",
            Self::Goose => "Goose",
            Self::Droid => "Droid",
        }
    }

    /// The name typed to run it, which also names it to `muxctl`.
    #[must_use]
    pub const fn command(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Copilot => "copilot",
            Self::Gemini => "gemini",
            Self::OpenCode => "opencode",
            Self::Cursor => "cursor-agent",
            Self::Amp => "amp",
            Self::Aider => "aider",
            Self::Goose => "goose",
            Self::Droid => "droid",
        }
    }

    /// The agent `muxctl` was given by name.
    #[must_use]
    pub fn from_command(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.command().eq_ignore_ascii_case(name))
    }

    /// Directory names that only this agent's own files sit under, for
    /// telling it from the runtime it was started with.
    const fn package_names(self) -> &'static [&'static str] {
        match self {
            Self::Claude => &["claude-code", "claude"],
            Self::Codex => &["codex"],
            Self::Copilot => &["copilot"],
            Self::Gemini => &["gemini-cli", "gemini"],
            Self::OpenCode => &["opencode", "opencode-ai"],
            Self::Cursor => &["cursor-agent"],
            Self::Amp => &["amp"],
            Self::Aider => &["aider", "aider-chat"],
            Self::Goose => &["goose"],
            Self::Droid => &["droid"],
        }
    }

    /// Which agent a process is, from its name, the file it runs and its
    /// arguments. An agent written for Node or Python runs under that
    /// runtime's name, and one installed by version, as Claude Code is,
    /// runs under a file named for the version, so the name alone can
    /// miss; the paths around it tell.
    #[must_use]
    pub fn detect(name: &str, executable: Option<&Path>, arguments: &[String]) -> Option<Self> {
        let named = |candidate: &str| {
            let stem = Path::new(candidate)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(candidate);
            let stem = stem
                .strip_suffix(".js")
                .or_else(|| stem.strip_suffix(".mjs"))
                .or_else(|| stem.strip_suffix(".cjs"))
                .or_else(|| stem.strip_suffix(".py"))
                .unwrap_or(stem);
            Self::ALL
                .into_iter()
                .find(|kind| kind.command().eq_ignore_ascii_case(stem))
        };
        if let Some(kind) = named(name) {
            return Some(kind);
        }
        if let Some(kind) = arguments.first().and_then(|first| named(first)) {
            return Some(kind);
        }
        let within = |path: &Path| {
            let components: Vec<_> = path
                .components()
                .filter_map(|component| component.as_os_str().to_str())
                .collect();
            Self::ALL.into_iter().find(|kind| {
                kind.package_names().iter().any(|package| {
                    components.windows(2).any(|pair| {
                        // `…/claude/versions/2.1.3`, `…/@openai/codex/bin/…`,
                        // `…/node_modules/@google/gemini-cli/…`.
                        pair[0].eq_ignore_ascii_case(package)
                            && (pair[1] == "versions"
                                || pair[1] == "bin"
                                || pair[1] == "dist"
                                || pair[1] == "lib")
                            || (pair[0].starts_with('@') && pair[1].eq_ignore_ascii_case(package))
                    })
                })
            })
        };
        if let Some(kind) = executable.and_then(within) {
            return Some(kind);
        }
        if is_runtime(name) {
            // The script a runtime was given is the first argument that
            // isn't one of its own flags.
            return arguments
                .iter()
                .skip(1)
                .find(|argument| !argument.starts_with('-'))
                .and_then(|script| named(script).or_else(|| within(Path::new(script))));
        }
        None
    }

    /// What to type at a shell to pick a session of this agent's up again.
    #[must_use]
    pub fn resume_command(self, agent_session: &str) -> Option<String> {
        // A session's name comes from the agent, by way of its hook: only
        // one that reads as an id is typed at a shell.
        let safe = !agent_session.is_empty()
            && agent_session.len() <= 128
            && agent_session
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "-_.".contains(character));
        if !safe {
            return None;
        }
        match self {
            Self::Claude => Some(format!("claude --resume {agent_session}")),
            Self::Codex => Some(format!("codex resume {agent_session}")),
            Self::Copilot => Some(format!("copilot --resume {agent_session}")),
            _ => None,
        }
    }
}

fn is_runtime(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    ["node", "bun", "deno", "python", "uv"]
        .iter()
        .any(|runtime| name == *runtime || name.starts_with(&format!("{runtime}3")))
        || name.starts_with("python")
        // A wrapper script by an agent's name runs under the shell of its
        // first line.
        || ["sh", "bash", "zsh", "dash", "ksh", "fish"].contains(&name.as_str())
}

/// What an agent is doing.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FleetState {
    /// An agent is there, and has not said what it is doing.
    Unknown,
    /// Waiting for its next prompt, with nothing new to show.
    Idle,
    Working,
    /// Waiting on the user before it can go on.
    Blocked,
    /// Finished a turn the user has not looked at yet.
    Done,
    /// Stopped by a failure.
    Error,
    /// The agent has exited; its session can be picked up again.
    Ended,
}

impl FleetState {
    /// Whether the agent is waiting on the user: what sorts first and
    /// what "go to what's waiting" goes to.
    #[must_use]
    pub const fn wants_user(self) -> bool {
        matches!(self, Self::Blocked | Self::Error)
    }

    /// Whether the agent is running, rather than a session to pick up.
    #[must_use]
    pub const fn is_live(self) -> bool {
        !matches!(self, Self::Ended)
    }

    /// Where the state comes in a roster, the most pressing first.
    #[must_use]
    pub const fn rank(self) -> u8 {
        match self {
            Self::Blocked => 0,
            Self::Error => 1,
            Self::Done => 2,
            Self::Working => 3,
            Self::Unknown => 4,
            Self::Idle => 5,
            Self::Ended => 6,
        }
    }

    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Unknown => "running",
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Blocked => "waiting on you",
            Self::Done => "done",
            Self::Error => "failed",
            Self::Ended => "ended",
        }
    }
}

/// What a blocked agent needs from the user.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Need {
    /// Approval to do something.
    Permission,
    /// An answer.
    Question,
    /// A login.
    Auth,
    /// It did not say what.
    Attention,
}

/// The best thing Mux has heard an agent's state from.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Signal {
    /// Only that its process is in front of the pane's shell.
    Process,
    /// A notification or a bell.
    Notice,
    /// The program status protocol, OSC 7501.
    Status,
    /// Mux's hooks in the agent.
    Hooks,
}

/// One agent in one pane.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FleetAgent {
    pub pane_id: PaneId,
    /// The Mux session the pane is in.
    pub session_id: SessionId,
    pub kind: AgentKind,
    pub state: FleetState,
    /// What it needs, while blocked.
    pub need: Option<Need>,
    /// What the user last asked of it.
    pub task: Option<String>,
    /// What it is doing now: the tool in hand.
    pub activity: Option<String>,
    /// What it asks, while blocked; else the last thing it said.
    pub message: Option<String>,
    /// The agent's own name for its session, to pick it up again by.
    pub agent_session: Option<String>,
    /// Where the agent keeps the session's transcript.
    pub transcript: Option<PathBuf>,
    pub cwd: PathBuf,
    pub project: Project,
    /// When it came into its present state, in milliseconds since 1970.
    pub since_ms: u64,
    /// When Mux first saw it.
    pub started_ms: u64,
    /// How many prompts it has been given.
    pub turns: u32,
    pub signal: Signal,
}

impl FleetAgent {
    /// What to type at the pane's shell to pick the session up again.
    #[must_use]
    pub fn resume_command(&self) -> Option<String> {
        self.kind.resume_command(self.agent_session.as_deref()?)
    }

    /// The one line that says most about it now: the question while it
    /// waits, the tool while it works, else what it last said.
    #[must_use]
    pub fn headline(&self) -> Option<&str> {
        match self.state {
            FleetState::Working => self.activity.as_deref(),
            _ => self.message.as_deref(),
        }
    }
}

/// A change to the fleet, for clients keeping a copy.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum FleetEvent {
    Changed(Box<FleetAgent>),
    Removed { pane_id: PaneId },
}

/// What a hook says an agent has just done.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentReport {
    pub kind: AgentKind,
    pub event: ReportEvent,
    pub agent_session: Option<String>,
    pub transcript: Option<PathBuf>,
    pub cwd: Option<PathBuf>,
    /// When the hook ran, in milliseconds since 1970. Hooks run alongside
    /// the agent, so two can land out of order; the later one stands.
    pub at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ReportEvent {
    /// A session began or was picked up, perhaps with a name of its own.
    SessionStarted {
        title: Option<String>,
    },
    /// The user gave it a prompt.
    Prompted {
        prompt: String,
    },
    /// It is at work; on this, if it says.
    Working {
        activity: Option<String>,
    },
    Blocked {
        need: Need,
        message: Option<String>,
    },
    /// Its turn ended, perhaps with the last thing it said.
    Finished {
        message: Option<String>,
    },
    Failed {
        message: Option<String>,
    },
    SessionEnded,
}

/// What a program said with the program status protocol, as much of it as
/// a fleet uses.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StatusReport {
    pub state: StatusState,
    pub need: Option<Need>,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatusState {
    Idle,
    Working,
    Done,
    Blocked,
    Error,
}

const MAX_TASK: usize = 240;
const MAX_LINE: usize = 240;

/// One line of at most `limit` characters from whatever a program said.
#[must_use]
pub fn one_line(text: &str, limit: usize) -> Option<String> {
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() {
            line.push(' ');
        }
        line.extend(word.chars().filter(|character| !character.is_control()));
        if line.chars().count() > limit {
            break;
        }
    }
    if line.chars().count() > limit {
        line = line.chars().take(limit.saturating_sub(1)).collect();
        line.push('…');
    }
    (!line.is_empty()).then_some(line)
}

/// Every pane's agent.
#[derive(Debug, Default)]
pub struct Fleet {
    agents: HashMap<PaneId, Entry>,
}

#[derive(Debug)]
struct Entry {
    agent: FleetAgent,
    /// The time of the last hook report taken in.
    reported_ms: u64,
}

impl Fleet {
    /// Every agent, the most pressing first, then the longest in its state.
    #[must_use]
    pub fn list(&self) -> Vec<FleetAgent> {
        let mut agents: Vec<_> = self
            .agents
            .values()
            .map(|entry| entry.agent.clone())
            .collect();
        agents.sort_by_key(|agent| (agent.state.rank(), agent.since_ms, agent.pane_id));
        agents
    }

    #[must_use]
    pub fn get(&self, pane_id: PaneId) -> Option<&FleetAgent> {
        self.agents.get(&pane_id).map(|entry| &entry.agent)
    }

    /// An agent's process has come to the front of a pane.
    pub fn process_started(
        &mut self,
        pane_id: PaneId,
        session_id: SessionId,
        kind: AgentKind,
        cwd: &Path,
        now_ms: u64,
    ) -> Option<FleetEvent> {
        if let Some(entry) = self.agents.get(&pane_id)
            && entry.agent.kind == kind
            && entry.agent.state.is_live()
        {
            // Its hook spoke first.
            return None;
        }
        self.agents.insert(
            pane_id,
            Entry {
                agent: new_agent(pane_id, session_id, kind, cwd, now_ms),
                reported_ms: 0,
            },
        );
        self.changed(pane_id)
    }

    /// Whatever was in front of a pane's shell has given the terminal
    /// back. An agent with a session to its name is kept as one to pick
    /// up again; any other is forgotten.
    pub fn process_finished(&mut self, pane_id: PaneId, now_ms: u64) -> Option<FleetEvent> {
        let entry = self.agents.get_mut(&pane_id)?;
        if !entry.agent.state.is_live() {
            return None;
        }
        if entry.agent.resume_command().is_none() {
            self.agents.remove(&pane_id);
            return Some(FleetEvent::Removed { pane_id });
        }
        let agent = &mut entry.agent;
        agent.state = FleetState::Ended;
        agent.need = None;
        agent.activity = None;
        agent.since_ms = now_ms;
        self.changed(pane_id)
    }

    /// A pane has closed, or the user has let an ended session go.
    pub fn forget(&mut self, pane_id: PaneId) -> Option<FleetEvent> {
        self.agents
            .remove(&pane_id)
            .map(|_| FleetEvent::Removed { pane_id })
    }

    /// A hook's report. `pane_cwd` is where the pane's shell is, for a
    /// report that does not say where the agent is.
    #[allow(clippy::too_many_lines)]
    pub fn report(
        &mut self,
        pane_id: PaneId,
        session_id: SessionId,
        report: AgentReport,
        pane_cwd: &Path,
        now_ms: u64,
    ) -> Option<FleetEvent> {
        let cwd = report.cwd.as_deref().unwrap_or(pane_cwd);
        let fresh = self.agents.get(&pane_id).is_none_or(|entry| {
            entry.agent.kind != report.kind
                || (!entry.agent.state.is_live()
                    && !matches!(report.event, ReportEvent::SessionEnded))
        });
        if fresh {
            if matches!(report.event, ReportEvent::SessionEnded) {
                return None;
            }
            self.agents.insert(
                pane_id,
                Entry {
                    agent: new_agent(pane_id, session_id, report.kind, cwd, now_ms),
                    reported_ms: 0,
                },
            );
        }
        let entry = self.agents.get_mut(&pane_id)?;
        if report.at_ms < entry.reported_ms {
            return None;
        }
        entry.reported_ms = report.at_ms;
        let before = entry.agent.clone();
        let agent = &mut entry.agent;
        agent.session_id = session_id;
        agent.signal = Signal::Hooks;
        if let Some(agent_session) = report.agent_session.filter(|id| !id.is_empty())
            && agent.agent_session.as_deref() != Some(&agent_session)
        {
            // A new session in the same pane, as after `/clear`.
            if agent.agent_session.is_some() {
                agent.task = None;
                agent.message = None;
                agent.turns = 0;
            }
            agent.agent_session = Some(agent_session);
        }
        if report.transcript.is_some() {
            agent.transcript = report.transcript;
        }
        if agent.cwd != cwd {
            agent.cwd = cwd.to_path_buf();
            agent.project = Project::of(cwd);
        }
        match report.event {
            ReportEvent::SessionStarted { title } => {
                if let Some(title) = title.as_deref().and_then(|title| one_line(title, MAX_TASK)) {
                    agent.task.get_or_insert(title);
                }
                if matches!(agent.state, FleetState::Unknown | FleetState::Ended) {
                    set_state(agent, FleetState::Idle, now_ms);
                }
            }
            ReportEvent::Prompted { prompt } => {
                if let Some(prompt) = one_line(&prompt, MAX_TASK) {
                    agent.task = Some(prompt);
                }
                agent.turns = agent.turns.saturating_add(1);
                agent.activity = None;
                agent.message = None;
                set_state(agent, FleetState::Working, now_ms);
            }
            ReportEvent::Working { activity } => {
                if let Some(activity) = activity
                    .as_deref()
                    .and_then(|line| one_line(line, MAX_LINE))
                {
                    agent.activity = Some(activity);
                }
                if agent.state == FleetState::Blocked {
                    agent.message = None;
                }
                set_state(agent, FleetState::Working, now_ms);
            }
            ReportEvent::Blocked { need, message } => {
                if let Some(message) = message.as_deref().and_then(|line| one_line(line, MAX_LINE))
                {
                    agent.message = Some(message);
                } else if agent.state != FleetState::Blocked {
                    agent.message = None;
                }
                set_state(agent, FleetState::Blocked, now_ms);
                agent.need = Some(need);
            }
            ReportEvent::Finished { message } => {
                agent.activity = None;
                agent.message = message.as_deref().and_then(|line| one_line(line, MAX_LINE));
                set_state(agent, FleetState::Done, now_ms);
            }
            ReportEvent::Failed { message } => {
                agent.activity = None;
                agent.message = message.as_deref().and_then(|line| one_line(line, MAX_LINE));
                set_state(agent, FleetState::Error, now_ms);
            }
            ReportEvent::SessionEnded => {
                if agent.resume_command().is_some() {
                    agent.activity = None;
                    set_state(agent, FleetState::Ended, now_ms);
                } else {
                    self.agents.remove(&pane_id);
                    return Some(FleetEvent::Removed { pane_id });
                }
            }
        }
        (*agent != before || fresh).then(|| FleetEvent::Changed(Box::new(agent.clone())))
    }

    /// What the program in a pane said with OSC 7501. Only an agent's is
    /// taken: a build saying it is working is no agent.
    pub fn status(
        &mut self,
        pane_id: PaneId,
        status: &StatusReport,
        now_ms: u64,
    ) -> Option<FleetEvent> {
        let agent = &mut self.agents.get_mut(&pane_id)?.agent;
        if !agent.state.is_live() {
            return None;
        }
        let before = agent.clone();
        agent.signal = agent.signal.max(Signal::Status);
        let said = one_line(&status.message, MAX_LINE);
        match status.state {
            StatusState::Working => {
                if agent.state == FleetState::Blocked {
                    agent.message = None;
                }
                // Hooks name the tool in hand, which says more than this.
                if agent.signal != Signal::Hooks && said.is_some() {
                    agent.activity = said;
                }
                set_state(agent, FleetState::Working, now_ms);
            }
            StatusState::Blocked => {
                // A hook's own account of what is asked says more.
                if said.is_some() && (agent.state != FleetState::Blocked || agent.message.is_none())
                {
                    agent.message = said;
                }
                let need = status.need.or(agent.need).unwrap_or(Need::Attention);
                set_state(agent, FleetState::Blocked, now_ms);
                agent.need = Some(need);
            }
            StatusState::Done => {
                agent.activity = None;
                if agent.signal != Signal::Hooks && said.is_some() {
                    agent.message = said;
                }
                if agent.state != FleetState::Idle {
                    set_state(agent, FleetState::Done, now_ms);
                }
            }
            StatusState::Idle => {
                agent.activity = None;
                // Done until looked at, however soon the agent goes idle.
                if agent.state != FleetState::Done {
                    set_state(agent, FleetState::Idle, now_ms);
                }
            }
            StatusState::Error => {
                agent.activity = None;
                if said.is_some() {
                    agent.message = said;
                }
                set_state(agent, FleetState::Error, now_ms);
            }
        }
        (*agent != before).then(|| FleetEvent::Changed(Box::new(agent.clone())))
    }

    /// A notification from a pane, or a bell (with nothing said). It is
    /// the only word an agent without hooks or status gives, so only such
    /// an agent's is read: asking leave means it waits, anything else
    /// that it is done.
    pub fn notice(&mut self, pane_id: PaneId, said: &str, now_ms: u64) -> Option<FleetEvent> {
        let agent = &mut self.agents.get_mut(&pane_id)?.agent;
        if !agent.state.is_live() || agent.signal > Signal::Notice {
            return None;
        }
        let before = agent.clone();
        agent.signal = Signal::Notice;
        let lower = said.to_lowercase();
        let asks = ["permission", "approv", "allow", "confirm"]
            .iter()
            .any(|word| lower.contains(word));
        agent.activity = None;
        agent.message = one_line(said, MAX_LINE);
        if asks {
            set_state(agent, FleetState::Blocked, now_ms);
            agent.need = Some(Need::Permission);
        } else if lower.contains("question") || lower.contains("input") && lower.contains("need") {
            set_state(agent, FleetState::Blocked, now_ms);
            agent.need = Some(Need::Question);
        } else {
            set_state(agent, FleetState::Done, now_ms);
        }
        (*agent != before).then(|| FleetEvent::Changed(Box::new(agent.clone())))
    }

    /// The user sent a line into a pane. An agent that says nothing of
    /// itself is taken to be at work on it.
    pub fn submitted(&mut self, pane_id: PaneId, now_ms: u64) -> Option<FleetEvent> {
        let agent = &mut self.agents.get_mut(&pane_id)?.agent;
        if !agent.state.is_live() || agent.signal > Signal::Notice {
            return None;
        }
        if agent.state == FleetState::Unknown {
            return None;
        }
        agent.message = None;
        set_state(agent, FleetState::Unknown, now_ms);
        self.changed(pane_id)
    }

    /// The user has looked at a pane: what was done there is no longer
    /// news.
    pub fn seen(&mut self, pane_id: PaneId, now_ms: u64) -> Option<FleetEvent> {
        let agent = &mut self.agents.get_mut(&pane_id)?.agent;
        if agent.state != FleetState::Done {
            return None;
        }
        set_state(agent, FleetState::Idle, now_ms);
        self.changed(pane_id)
    }

    /// Take up what a daemon before this one knew. Its agents went with
    /// it, so those with a session to pick up come back as ended and the
    /// rest are dropped; `live` says which panes are still there.
    pub fn restore(&mut self, agents: Vec<FleetAgent>, live: impl Fn(PaneId) -> bool) {
        for mut agent in agents {
            if !live(agent.pane_id) || agent.resume_command().is_none() {
                continue;
            }
            agent.state = FleetState::Ended;
            agent.need = None;
            agent.activity = None;
            self.agents.insert(
                agent.pane_id,
                Entry {
                    agent,
                    reported_ms: 0,
                },
            );
        }
    }

    /// Drop the agents of panes that are gone.
    pub fn retain_panes(&mut self, live: impl Fn(PaneId) -> bool) -> Vec<FleetEvent> {
        let gone: Vec<_> = self
            .agents
            .keys()
            .copied()
            .filter(|pane_id| !live(*pane_id))
            .collect();
        gone.into_iter()
            .filter_map(|pane_id| self.forget(pane_id))
            .collect()
    }

    fn changed(&self, pane_id: PaneId) -> Option<FleetEvent> {
        self.agents
            .get(&pane_id)
            .map(|entry| FleetEvent::Changed(Box::new(entry.agent.clone())))
    }
}

fn new_agent(
    pane_id: PaneId,
    session_id: SessionId,
    kind: AgentKind,
    cwd: &Path,
    now_ms: u64,
) -> FleetAgent {
    FleetAgent {
        pane_id,
        session_id,
        kind,
        state: FleetState::Unknown,
        need: None,
        task: None,
        activity: None,
        message: None,
        agent_session: None,
        transcript: None,
        cwd: cwd.to_path_buf(),
        project: Project::of(cwd),
        since_ms: now_ms,
        started_ms: now_ms,
        turns: 0,
        signal: Signal::Process,
    }
}

fn set_state(agent: &mut FleetAgent, state: FleetState, now_ms: u64) {
    if agent.state != state {
        agent.state = state;
        agent.since_ms = now_ms;
    }
    if state != FleetState::Blocked {
        agent.need = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(event: ReportEvent, at_ms: u64) -> AgentReport {
        AgentReport {
            kind: AgentKind::Claude,
            event,
            agent_session: Some("0f3c-session".to_owned()),
            transcript: None,
            cwd: None,
            at_ms,
        }
    }

    fn agent(event: Option<FleetEvent>) -> FleetAgent {
        match event {
            Some(FleetEvent::Changed(agent)) => *agent,
            other => panic!("expected a change, got {other:?}"),
        }
    }

    #[test]
    fn agents_are_told_from_the_runtimes_and_versions_they_run_under() {
        let args = |list: &[&str]| list.iter().map(|&arg| arg.to_owned()).collect::<Vec<_>>();
        assert_eq!(
            AgentKind::detect("claude", None, &[]),
            Some(AgentKind::Claude)
        );
        assert_eq!(
            AgentKind::detect(
                "2.1.323",
                Some(Path::new("/Users/r/.local/share/claude/versions/2.1.323")),
                &args(&["claude", "--resume"]),
            ),
            Some(AgentKind::Claude)
        );
        assert_eq!(
            AgentKind::detect(
                "2.1.323",
                Some(Path::new("/Users/r/.local/share/claude/versions/2.1.323")),
                &[],
            ),
            Some(AgentKind::Claude)
        );
        assert_eq!(
            AgentKind::detect(
                "node",
                Some(Path::new("/opt/homebrew/bin/node")),
                &args(&[
                    "node",
                    "/opt/homebrew/lib/node_modules/@google/gemini-cli/dist/index.js"
                ]),
            ),
            Some(AgentKind::Gemini)
        );
        assert_eq!(
            AgentKind::detect(
                "node",
                None,
                &args(&["node", "--no-warnings", "/usr/local/bin/copilot"]),
            ),
            Some(AgentKind::Copilot)
        );
        assert_eq!(
            AgentKind::detect("node", None, &args(&["node", "server.js"])),
            None
        );
        // A wrapper script runs under its shell; a shell at its prompt, or
        // one running something else, is no agent.
        assert_eq!(
            AgentKind::detect("bash", None, &args(&["/bin/sh", "./claude"])),
            Some(AgentKind::Claude)
        );
        assert_eq!(AgentKind::detect("zsh", None, &args(&["-zsh"])), None);
        assert_eq!(
            AgentKind::detect("bash", None, &args(&["bash", "-c", "claude --help | less"])),
            None
        );
        assert_eq!(
            AgentKind::detect("vim", None, &args(&["vim", "codex.md"])),
            None
        );
        assert_eq!(
            AgentKind::detect("cargo", None, &args(&["cargo", "test"])),
            None
        );
    }

    #[test]
    fn only_an_id_is_typed_at_a_shell_to_resume() {
        assert_eq!(
            AgentKind::Claude.resume_command("0f3c-9a_b.1").as_deref(),
            Some("claude --resume 0f3c-9a_b.1")
        );
        assert_eq!(
            AgentKind::Codex.resume_command("abc").as_deref(),
            Some("codex resume abc")
        );
        assert_eq!(AgentKind::Claude.resume_command("a; rm -rf ~"), None);
        assert_eq!(AgentKind::Claude.resume_command("$(id)"), None);
        assert_eq!(AgentKind::Claude.resume_command(""), None);
        assert_eq!(AgentKind::Gemini.resume_command("abc"), None);
    }

    #[test]
    fn a_turn_runs_from_prompt_through_asking_to_done_and_seen() {
        let mut fleet = Fleet::default();
        let pane = PaneId::new();
        let session = SessionId::new();
        let cwd = Path::new("/tmp");
        let started = agent(fleet.process_started(pane, session, AgentKind::Claude, cwd, 1));
        assert_eq!(started.state, FleetState::Unknown);
        assert_eq!(started.signal, Signal::Process);

        let idle = agent(fleet.report(
            pane,
            session,
            report(ReportEvent::SessionStarted { title: None }, 2),
            cwd,
            2,
        ));
        assert_eq!(idle.state, FleetState::Idle);
        assert_eq!(idle.signal, Signal::Hooks);
        assert_eq!(idle.agent_session.as_deref(), Some("0f3c-session"));

        let working = agent(fleet.report(
            pane,
            session,
            report(
                ReportEvent::Prompted {
                    prompt: "fix the\n  flaky test".to_owned(),
                },
                3,
            ),
            cwd,
            3,
        ));
        assert_eq!(working.state, FleetState::Working);
        assert_eq!(working.task.as_deref(), Some("fix the flaky test"));
        assert_eq!(working.turns, 1);

        let blocked = agent(fleet.report(
            pane,
            session,
            report(
                ReportEvent::Blocked {
                    need: Need::Permission,
                    message: Some("Bash: cargo test".to_owned()),
                },
                4,
            ),
            cwd,
            4,
        ));
        assert_eq!(blocked.state, FleetState::Blocked);
        assert_eq!(blocked.need, Some(Need::Permission));
        assert_eq!(blocked.headline(), Some("Bash: cargo test"));
        assert_eq!(blocked.since_ms, 4);

        // The agent's own status says it is working again once allowed.
        let allowed = agent(fleet.status(
            pane,
            &StatusReport {
                state: StatusState::Working,
                need: None,
                message: String::new(),
            },
            5,
        ));
        assert_eq!(allowed.state, FleetState::Working);
        assert_eq!(allowed.need, None);
        assert_eq!(allowed.message, None);
        assert_eq!(allowed.signal, Signal::Hooks);

        let done = agent(fleet.report(
            pane,
            session,
            report(
                ReportEvent::Finished {
                    message: Some("The test passes now.".to_owned()),
                },
                6,
            ),
            cwd,
            6,
        ));
        assert_eq!(done.state, FleetState::Done);
        assert_eq!(done.headline(), Some("The test passes now."));

        // Going idle straight after does not hide that it is done.
        assert!(
            fleet
                .status(
                    pane,
                    &StatusReport {
                        state: StatusState::Idle,
                        need: None,
                        message: String::new(),
                    },
                    7,
                )
                .is_none()
        );
        assert_eq!(agent(fleet.seen(pane, 8)).state, FleetState::Idle);
        assert!(fleet.seen(pane, 9).is_none());
    }

    #[test]
    fn a_report_that_lands_late_does_not_undo_a_later_one() {
        let mut fleet = Fleet::default();
        let pane = PaneId::new();
        let session = SessionId::new();
        let cwd = Path::new("/tmp");
        fleet.report(
            pane,
            session,
            report(ReportEvent::Finished { message: None }, 20),
            cwd,
            20,
        );
        assert!(
            fleet
                .report(
                    pane,
                    session,
                    report(ReportEvent::Working { activity: None }, 10),
                    cwd,
                    21,
                )
                .is_none()
        );
        assert_eq!(
            fleet.get(pane).map(|agent| agent.state),
            Some(FleetState::Done)
        );
    }

    #[test]
    fn an_agent_that_exits_is_kept_to_pick_up_only_with_a_session() {
        let mut fleet = Fleet::default();
        let session = SessionId::new();
        let cwd = Path::new("/tmp");
        let hooked = PaneId::new();
        fleet.report(
            hooked,
            session,
            report(ReportEvent::SessionStarted { title: None }, 1),
            cwd,
            1,
        );
        let ended = agent(fleet.process_finished(hooked, 2));
        assert_eq!(ended.state, FleetState::Ended);
        assert_eq!(
            ended.resume_command().as_deref(),
            Some("claude --resume 0f3c-session")
        );
        // The same agent started again in the pane is a new one.
        let again = agent(fleet.process_started(hooked, session, AgentKind::Claude, cwd, 3));
        assert_eq!(again.state, FleetState::Unknown);
        assert_eq!(again.agent_session, None);

        let bare = PaneId::new();
        fleet.process_started(bare, session, AgentKind::Gemini, cwd, 1);
        assert_eq!(
            fleet.process_finished(bare, 2),
            Some(FleetEvent::Removed { pane_id: bare })
        );
        assert!(fleet.process_finished(PaneId::new(), 2).is_none());
    }

    #[test]
    fn an_agent_with_no_hooks_is_read_from_its_notifications() {
        let mut fleet = Fleet::default();
        let pane = PaneId::new();
        let session = SessionId::new();
        fleet.process_started(pane, session, AgentKind::Gemini, Path::new("/tmp"), 1);
        let asked = agent(fleet.notice(pane, "Gemini needs your permission to run a command", 2));
        assert_eq!(asked.state, FleetState::Blocked);
        assert_eq!(asked.need, Some(Need::Permission));
        let answered = agent(fleet.submitted(pane, 3));
        assert_eq!(answered.state, FleetState::Unknown);
        assert_eq!(answered.message, None);
        let done = agent(fleet.notice(pane, "", 4));
        assert_eq!(done.state, FleetState::Done);

        // One that speaks for itself is not second-guessed by its bell.
        let hooked = PaneId::new();
        fleet.report(
            hooked,
            session,
            report(
                ReportEvent::Prompted {
                    prompt: "go".to_owned(),
                },
                1,
            ),
            Path::new("/tmp"),
            1,
        );
        assert!(fleet.notice(hooked, "anything", 2).is_none());
        assert!(fleet.submitted(hooked, 2).is_none());
        // And a program that is no agent has no place in the fleet.
        assert!(fleet.notice(PaneId::new(), "build finished", 2).is_none());
    }

    #[test]
    fn a_new_daemon_keeps_only_sessions_that_can_be_picked_up() {
        let mut old = Fleet::default();
        let session = SessionId::new();
        let cwd = Path::new("/tmp");
        let kept = PaneId::new();
        let closed = PaneId::new();
        let bare = PaneId::new();
        for pane in [kept, closed] {
            old.report(
                pane,
                session,
                report(
                    ReportEvent::Prompted {
                        prompt: "ship it".to_owned(),
                    },
                    1,
                ),
                cwd,
                1,
            );
        }
        old.process_started(bare, session, AgentKind::Gemini, cwd, 1);

        let mut new = Fleet::default();
        new.restore(old.list(), |pane| pane != closed);
        let agents = new.list();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].pane_id, kept);
        assert_eq!(agents[0].state, FleetState::Ended);
        assert_eq!(agents[0].task.as_deref(), Some("ship it"));
    }

    #[test]
    fn the_roster_puts_what_waits_on_the_user_first() {
        let mut fleet = Fleet::default();
        let session = SessionId::new();
        let cwd = Path::new("/tmp");
        let working = PaneId::new();
        let blocked = PaneId::new();
        let done = PaneId::new();
        fleet.report(
            working,
            session,
            report(ReportEvent::Working { activity: None }, 1),
            cwd,
            1,
        );
        fleet.report(
            done,
            session,
            report(ReportEvent::Finished { message: None }, 2),
            cwd,
            2,
        );
        fleet.report(
            blocked,
            session,
            report(
                ReportEvent::Blocked {
                    need: Need::Question,
                    message: None,
                },
                3,
            ),
            cwd,
            3,
        );
        let order: Vec<_> = fleet
            .list()
            .into_iter()
            .map(|agent| agent.pane_id)
            .collect();
        assert_eq!(order, vec![blocked, done, working]);
    }

    #[test]
    fn long_text_is_cut_to_one_line() {
        assert_eq!(one_line("  a\n\tb  c ", 20).as_deref(), Some("a b c"));
        assert_eq!(one_line("abcdefgh", 5).as_deref(), Some("abcd…"));
        assert_eq!(one_line(" \n ", 5), None);
        assert_eq!(one_line("a\x1b[31mb", 9).as_deref(), Some("a[31mb"));
    }
}
