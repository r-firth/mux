//! The fleet.
//!
//! Every coding agent running in a terminal pane, in one place. ⌘⇧A lays
//! it over the panes like a tab of its own: a roster on the left, the
//! agents under the projects they work in with what each is doing, and on
//! the right the chosen agent's own pane, live. The roster is the pane on
//! the left: the arrows choose, Return or ⌥→ steps into the agent's pane
//! to type to it, ⌥← steps back, and escape or ⌘⇧A goes back to the tab
//! it was opened over. While it is closed, a chip at the head of the strip
//! says agents are there, and how many are waiting.
//!
//! `v` turns the view into a wall: every agent's pane at once, each drawn
//! small in a tile under its name and state, the ones that wait first. The
//! panes keep their own sizes; a tile draws its pane smaller, never too
//! small to read, and crops to the foot of what it holds. A waiting
//! agent's tile calls as a pane that calls does, in peach on the ground
//! around it.
//!
//! The daemon keeps the fleet (see `mux-fleet`); this window keeps a copy,
//! told of each change, and says when a finished agent has been looked at.

use super::*;
use mux_fleet::install::{self, Homes, HookStatus};
use mux_fleet::{AgentKind, FleetAgent, FleetEvent, FleetState};

/// The roster's share of the window, and the widths it keeps between.
const ROSTER_SHARE: f32 = 0.34;
const ROSTER_MIN: f32 = 340.0;
const ROSTER_MAX: f32 = 460.0;
/// The least the agent's pane keeps beside the roster.
const PANE_MIN: f32 = 240.0;
const ROW_HEIGHT: f32 = 46.0;
/// A row with one line: an agent that was asked nothing and says nothing.
const ROW_HEIGHT_BRIEF: f32 = 28.0;
/// How tall the slab over an ended agent's pane is.
const ENDED_CARD_HEIGHT: f32 = 92.0;
/// How long an agent is chosen before its pane is sized to the view, so
/// passing over a row leaves its program alone.
const SETTLE: Duration = Duration::from_millis(140);
/// How long a finished agent is chosen before it counts as looked at.
const SEEN_AFTER: Duration = Duration::from_millis(1200);
/// How much of another session's pane is asked for.
const REMOTE_LINES: u32 = 200;
/// The least a tile of the wall is, the gap between tiles, and the heights
/// of a tile's head and the wall's foot.
/// A roster shorter than this says less at its foot.
const SHORT_ROSTER: f32 = 420.0;
const TILE_MIN_WIDTH: f32 = 260.0;
const TILE_MIN_HEIGHT: f32 = 180.0;
const TILE_HEAD: f32 = 46.0;
const TILE_PAD: f32 = 5.0;
const WALL_FOOT: f32 = 36.0;
/// The smallest a tile's text is drawn, which still reads: a pane too wide
/// or too tall for its tile at that size shows its left side and the foot
/// of what it holds, rather than the whole made smaller.
const TILE_MIN_FONT: f32 = 8.0;
/// How long a new tab's shell is waited on before the agent's command is
/// typed regardless.
const SHELL_WAIT: Duration = Duration::from_secs(4);

pub(super) struct FleetView {
    focus: FocusHandle,
    /// The agent the keys act on, by its pane, so the roster can reorder
    /// under it. `None` is the first in the roster.
    selected: Option<PaneId>,
    /// When that agent was chosen.
    selected_at: Instant,
    /// Whether the keys are in the agent's pane rather than the roster.
    in_pane: bool,
    /// The tab the view was opened over, which closing it goes back to.
    came_from: Option<TabId>,
    /// The tab the workspace is on beneath the view, and one it has been
    /// asked to move to. Any other change of tab is the user's own, and
    /// puts the view away.
    under_tab: Option<TabId>,
    expected_tab: Option<TabId>,
    expects_new_tab: bool,
    card: Option<Card>,
    scroll: ScrollHandle,
    /// What a pane of another session shows, as last asked.
    remote_text: HashMap<PaneId, String>,
    /// The pane a new agent was started in, chosen once the agent shows.
    awaiting: Option<PaneId>,
    /// The pane the view last drew.
    drawn_pane: Option<PaneId>,
    /// Whether the view is the wall, every agent's pane in a tile, rather
    /// than the roster beside one pane.
    wall: bool,
    /// The panes on the wall as last laid out, and how many to a row.
    wall_shown: Vec<PaneId>,
    wall_columns: usize,
}

/// A small sheet over the foot of the roster.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Card {
    /// Which agent to start, and whether where the keys were rather than
    /// in the chosen agent's project.
    Start { choice: usize, here: bool },
    /// Each agent Mux has hooks for.
    Hooks { choice: usize },
}

/// An agent on its way: its command is typed into the new tab's shell once
/// that has come up.
pub(super) struct FleetStart {
    command: String,
    /// The panes there were before the tab was asked for.
    known: HashSet<PaneId>,
}

/// One agent's tile on the wall.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct WallTile {
    pane_id: PaneId,
    frame: layout::Rect,
}

impl WallTile {
    /// Where the tile draws its pane, inside its edge and under its head.
    fn body(&self) -> layout::Rect {
        layout::Rect {
            x: self.frame.x + 1.0 + TILE_PAD,
            y: self.frame.y + TILE_HEAD,
            width: (self.frame.width - 2.0 - TILE_PAD * 2.0).max(1.0),
            height: (self.frame.height - TILE_HEAD - 1.0 - TILE_PAD).max(1.0),
        }
    }
}

/// The wall: the tiles of the page on screen, and the foot under them.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct WallLayout {
    tiles: Vec<WallTile>,
    columns: usize,
    foot: layout::Rect,
    /// Which page of tiles this is, from 0, and of how many.
    page: usize,
    pages: usize,
}

/// A tile's pane, drawn small: its view, the size it is drawn at, and how
/// far down it the last of what it shows ends.
pub(super) struct TilePane {
    view: Entity<gpui_terminal::TerminalView>,
    width: f32,
    height: f32,
    foot: f32,
}

/// How many of a frame's rows, from its top, hold anything: text, a cell
/// with a ground of its own, or the cursor. A program's latest output ends
/// there, with only blank rows under it.
fn rows_in_use(frame: &mux_terminal::RenderFrame) -> u16 {
    let cols = usize::from(frame.cols.max(1));
    let shows = |cell: &mux_terminal::RenderCell| {
        !cell.grapheme.trim().is_empty() || cell.background != frame.background
    };
    let last_text = frame
        .cells
        .chunks(cols)
        .rposition(|row| row.iter().any(shows))
        .map_or(0, |row| row + 1);
    let cursor = frame
        .cursor
        .filter(|cursor| cursor.visible)
        .map_or(0, |cursor| usize::from(cursor.y) + 1);
    u16::try_from(last_text.max(cursor))
        .unwrap_or(u16::MAX)
        .min(frame.rows)
}

/// Where the view's parts go.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct FleetLayout {
    pub(super) roster: layout::Rect,
    /// Everything right of the roster.
    detail: layout::Rect,
    /// The slab over an ended agent's pane.
    card: Option<layout::Rect>,
    /// The chosen agent's pane, when this window has it.
    pub(super) pane: Option<layout::PaneGeometry>,
    /// The wall, when the view is one: nothing else is drawn then.
    wall: Option<WallLayout>,
}

/// What a pane's head says of the agent in it: its name, what it was
/// asked, and its state with the colour that state is said in.
pub(super) type FleetHead = (String, Option<String>, Option<(&'static str, Hsla)>);

/// One project's agents in the roster.
#[derive(Debug, PartialEq)]
struct Group<'a> {
    name: &'a str,
    agents: Vec<&'a FleetAgent>,
}

/// The roster: agents under their projects, each project's most pressing
/// first, and the projects by their most pressing agent, then by name.
fn roster(agents: &[FleetAgent]) -> Vec<Group<'_>> {
    let mut groups: Vec<(&std::path::Path, Group<'_>)> = Vec::new();
    for agent in agents {
        let root = agent.project.root.as_path();
        if let Some((_, group)) = groups.iter_mut().find(|(known, _)| *known == root) {
            group.agents.push(agent);
        } else {
            groups.push((
                root,
                Group {
                    name: &agent.project.name,
                    agents: vec![agent],
                },
            ));
        }
    }
    for (_, group) in &mut groups {
        group
            .agents
            .sort_by_key(|agent| (agent.state.rank(), agent.since_ms, agent.pane_id));
    }
    groups.sort_by(|(a_root, a), (b_root, b)| {
        let pressing = |group: &Group<'_>| group.agents.first().map(|agent| agent.state.rank());
        pressing(a)
            .cmp(&pressing(b))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a_root.cmp(b_root))
    });
    groups.into_iter().map(|(_, group)| group).collect()
}

/// The roster's agents in the order it lists them.
fn roster_order(agents: &[FleetAgent]) -> Vec<PaneId> {
    roster(agents)
        .iter()
        .flat_map(|group| group.agents.iter().map(|agent| agent.pane_id))
        .collect()
}

/// Which of the roster's list an agent's row is, counting the head each
/// project has over its rows: what scrolling to the row goes by.
fn roster_list_index(agents: &[FleetAgent], pane_id: PaneId) -> Option<usize> {
    let mut index = 0;
    for group in roster(agents) {
        index += 1;
        for agent in group.agents {
            if agent.pane_id == pane_id {
                return Some(index);
            }
            index += 1;
        }
    }
    None
}

/// The wall's agents in the order it tiles them: the ones that wait on the
/// user first, then those with news, at work, idle and ended, each kind by
/// how long it has been so.
fn wall_order(agents: &[FleetAgent]) -> Vec<PaneId> {
    let mut order: Vec<&FleetAgent> = agents.iter().collect();
    order.sort_by_key(|agent| (agent.state.rank(), agent.since_ms, agent.pane_id));
    order.into_iter().map(|agent| agent.pane_id).collect()
}

/// How `count` tiles share a wall this size: how many to a row and how many
/// rows a page has. Every tile keeps its least size, so a wall with more
/// agents than fit shows them a page at a time; among the ways that fit,
/// the one whose tiles hold the largest pane wins.
fn wall_grid(count: usize, width: f32, height: f32) -> (usize, usize) {
    let gap = layout::PANE_GAP;
    let most = |room: f32, least: f32| (((room + gap) / (least + gap)).floor() as usize).max(1);
    let most_columns = most(width, TILE_MIN_WIDTH);
    let most_rows = most(height, TILE_MIN_HEIGHT);
    let count = count.max(1);
    (1..=most_columns.min(count))
        .map(|columns| (columns, count.div_ceil(columns).min(most_rows)))
        .max_by(|a, b| {
            let score = |(columns, rows): (usize, usize)| {
                let tile_width = (width - gap * (columns - 1) as f32) / columns as f32;
                let tile_height = (height - gap * (rows - 1) as f32) / rows as f32;
                // A pane is about eight wide to five high; a page that
                // holds every agent beats one that doesn't.
                let fits = columns * rows >= count;
                (fits, (tile_width / 1.6).min(tile_height))
            };
            let (a_fits, a_size) = score(*a);
            let (b_fits, b_size) = score(*b);
            a_fits
                .cmp(&b_fits)
                .then(a_size.total_cmp(&b_size))
                // Of two as good, the wider wall.
                .then(a.0.cmp(&b.0))
        })
        .unwrap_or((1, 1))
}

/// Where a step that way from tile `at` lands, of `count` tiles in rows of
/// `columns`: along the row and on to the next, or up and down a column,
/// stopping at the wall's edges.
fn wall_step(at: usize, count: usize, columns: usize, key: &str) -> usize {
    let columns = columns.max(1);
    match key {
        "left" | "h" => at.saturating_sub(1),
        "right" | "l" => (at + 1).min(count.saturating_sub(1)),
        "up" | "k" if at >= columns => at - columns,
        "down" | "j" if at + columns < count => at + columns,
        // Down from the row above a short last row goes to its end.
        "down" | "j" if at / columns < count.saturating_sub(1) / columns => count - 1,
        _ => at,
    }
}

/// An agent's name as the panes say it: the word that runs it, which is
/// short enough to leave a row's room to what the agent was asked.
pub(super) fn agent_name(kind: AgentKind) -> String {
    kind.command().to_owned()
}

/// What an agent says, with the paths in it said from where it works: a
/// file in its directory or its project by the rest of its path, and one
/// under the home directory from `~`. A row has little room, and the end
/// of a path says more than its start.
fn said_from_project(agent: &FleetAgent, said: &str, home: Option<&std::path::Path>) -> String {
    let mut said = said.to_owned();
    let mut places = vec![agent.cwd.as_path(), agent.project.root.as_path()];
    places.sort_by_key(|place| std::cmp::Reverse(place.as_os_str().len()));
    for place in places {
        let place = place.to_string_lossy();
        if place.len() > 1 {
            said = said.replace(&format!("{place}/"), "");
        }
    }
    if let Some(home) = home.map(|home| home.to_string_lossy())
        && home.len() > 1
    {
        said = said.replace(&format!("{home}/"), "~/");
    }
    said
}

/// An agent's line under its name: what it says, and for one that waits
/// having said nothing, what kind of thing it waits for.
fn fleet_headline(agent: &FleetAgent) -> Option<String> {
    fleet_said(agent, agent.headline()).or_else(|| {
        (agent.state == FleetState::Blocked).then(|| match agent.need {
            Some(mux_fleet::Need::Permission) => "asks to be allowed something".to_owned(),
            Some(mux_fleet::Need::Question) => "has a question".to_owned(),
            Some(mux_fleet::Need::Auth) => "needs you to sign in".to_owned(),
            _ => "wants you".to_owned(),
        })
    })
}

/// A line of an agent's, as a row or a tile's head says it.
fn fleet_said(agent: &FleetAgent, said: Option<&str>) -> Option<String> {
    static HOME: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    let home = HOME.get_or_init(|| {
        directories::BaseDirs::new().map(|directories| directories.home_dir().to_path_buf())
    });
    said.map(|said| said_from_project(agent, said, home.as_deref()))
}

/// The state as a row has room for it.
const fn state_word(state: FleetState) -> &'static str {
    match state {
        FleetState::Blocked => "waiting",
        other => other.word(),
    }
}

/// A row's last words: its state and for how long, `waiting 2m`. An idle
/// agent has nothing to count.
fn state_and_age(agent: &FleetAgent, now_ms: u64) -> String {
    let word = state_word(agent.state);
    if agent.state == FleetState::Idle {
        return word.to_owned();
    }
    let seconds = now_ms.saturating_sub(agent.since_ms) / 1_000;
    format!("{word} {}", pane_calls::duration_words(seconds))
}

/// How the fleet reads in a line: `2 waiting · 3 working · 1 done`.
fn summary(agents: &[FleetAgent]) -> String {
    let count = |wanted: &[FleetState]| {
        agents
            .iter()
            .filter(|agent| wanted.contains(&agent.state))
            .count()
    };
    let parts = [
        (count(&[FleetState::Blocked]), "waiting"),
        (count(&[FleetState::Error]), "failed"),
        (count(&[FleetState::Working]), "working"),
        (count(&[FleetState::Done]), "done"),
        (count(&[FleetState::Idle, FleetState::Unknown]), "idle"),
        (count(&[FleetState::Ended]), "ended"),
    ];
    parts
        .iter()
        .filter(|(count, _)| *count > 0)
        .map(|(count, word)| format!("{count} {word}"))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The summary in no more than `room` characters: its leading parts, whole.
/// The states that want the user come first, so what gives way is the
/// quiet end of it.
fn summary_within(agents: &[FleetAgent], room: usize) -> String {
    let said = summary(agents);
    let mut kept = 0;
    for (at, _) in said.match_indices(" · ") {
        if said[..at].chars().count() > room {
            break;
        }
        kept = at;
    }
    if said.chars().count() <= room {
        said
    } else if kept > 0 {
        said[..kept].to_owned()
    } else {
        said
    }
}

/// The next agent waiting on the user after `from` in `order`, coming
/// round at the ends, or the one before with `forward` false.
fn next_waiting(
    order: &[PaneId],
    waits: impl Fn(PaneId) -> bool,
    from: Option<PaneId>,
    forward: bool,
) -> Option<PaneId> {
    let count = order.len();
    let at = from.and_then(|from| order.iter().position(|pane_id| *pane_id == from));
    (1..=count)
        .map(|step| {
            let start = at.unwrap_or(if forward { count - 1 } else { 0 });
            if forward {
                (start + step) % count
            } else {
                (start + count - step % count) % count
            }
        })
        .map(|index| order[index])
        .find(|pane_id| waits(*pane_id))
}

/// A path as a shell reads it whole: in single quotes.
fn shell_quoted(path: &std::path::Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|since| u64::try_from(since.as_millis()).ok())
        .unwrap_or(0)
}

/// How long until an age that began at `since_ms` reads differently.
fn until_age_changes(since_ms: u64, now_ms: u64) -> Duration {
    let age = now_ms.saturating_sub(since_ms);
    let step = if age < 60_000 { 1_000 } else { 60_000 };
    Duration::from_millis(step - age % step)
}

/// The agents found on this machine, of those Mux knows by name. Finding
/// them can mean asking a login shell for its PATH, so not on the UI thread.
fn find_launchers() -> Vec<AgentKind> {
    AgentKind::ALL
        .into_iter()
        .filter(|kind| {
            mux_acp::AgentSpec {
                name: kind.name().to_owned(),
                command: PathBuf::from(kind.command()),
                args: Vec::new(),
                environment: Vec::new(),
            }
            .command_available()
        })
        .collect()
}

fn hook_statuses() -> Vec<(AgentKind, HookStatus)> {
    let Some(homes) = Homes::from_environment() else {
        return Vec::new();
    };
    AgentKind::HOOKED
        .into_iter()
        .map(|kind| (kind, install::status(kind, &homes)))
        .collect()
}

impl MuxApp {
    /// Take in the whole fleet, as the daemon has it now.
    pub(super) fn set_fleet(&mut self, agents: Vec<FleetAgent>) {
        self.fleet = agents;
        self.fleet_seen_sent.clear();
        self.fleet_changed();
    }

    /// Take in one change to the fleet.
    pub(super) fn apply_fleet_event(&mut self, event: FleetEvent) {
        match event {
            FleetEvent::Changed(agent) => {
                let pane_id = agent.pane_id;
                // Whatever it says now, the last word sent about it has
                // been heard.
                self.fleet_seen_sent.remove(&pane_id);
                if let Some(known) = self.fleet.iter_mut().find(|known| known.pane_id == pane_id) {
                    *known = *agent;
                } else {
                    self.fleet.push(*agent);
                }
                if let Some(view) = self.fleet_view.as_mut()
                    && view.awaiting == Some(pane_id)
                {
                    view.awaiting = None;
                    view.selected = Some(pane_id);
                    view.selected_at = Instant::now();
                }
                if self.fleet_remote_panes().contains(&pane_id) {
                    self.backend.send(CommandMessage::PaneText {
                        pane_id,
                        lines: REMOTE_LINES,
                    });
                }
            }
            FleetEvent::Removed { pane_id } => {
                self.fleet.retain(|agent| agent.pane_id != pane_id);
                self.fleet_seen_sent.remove(&pane_id);
            }
        }
        self.fleet_changed();
    }

    fn fleet_changed(&mut self) {
        let Some(view) = self.fleet_view.as_mut() else {
            return;
        };
        if view
            .selected
            .is_some_and(|pane_id| !self.fleet.iter().any(|agent| agent.pane_id == pane_id))
        {
            view.selected = None;
            view.selected_at = Instant::now();
        }
        // A wall with no agents has nothing to show.
        if self.fleet.is_empty() {
            view.wall = false;
        }
    }

    /// The live agent in a pane, if one is there.
    pub(super) fn fleet_agent_in(&self, pane_id: PaneId) -> Option<&FleetAgent> {
        self.fleet
            .iter()
            .find(|agent| agent.pane_id == pane_id && agent.state.is_live())
    }

    /// The agent the fleet view's keys act on.
    fn fleet_selected(&self) -> Option<&FleetAgent> {
        let view = self.fleet_view.as_ref()?;
        let chosen = view
            .selected
            .and_then(|pane_id| self.fleet.iter().find(|agent| agent.pane_id == pane_id));
        chosen.or_else(|| {
            let first = self.fleet_order().into_iter().next()?;
            self.fleet.iter().find(|agent| agent.pane_id == first)
        })
    }

    /// The agents in the order the view lists them: the wall's while it is
    /// one, else the roster's.
    fn fleet_order(&self) -> Vec<PaneId> {
        if self.fleet_wall_open() {
            wall_order(&self.fleet)
        } else {
            roster_order(&self.fleet)
        }
    }

    /// Whether the view is the wall.
    pub(super) fn fleet_wall_open(&self) -> bool {
        self.fleet_view.as_ref().is_some_and(|view| view.wall)
    }

    /// Whether a pane is on the wall, on the page on screen.
    pub(super) fn fleet_wall_shows(&self, pane_id: PaneId) -> bool {
        self.fleet_view
            .as_ref()
            .is_some_and(|view| view.wall && view.wall_shown.contains(&pane_id))
    }

    /// The chosen agent's pane, while the view shows it and this window has
    /// it: the one pane on screen then.
    pub(super) fn fleet_shown_pane(&self) -> Option<PaneId> {
        self.fleet_selected()
            .filter(|_| !self.fleet_wall_open())
            .map(|agent| agent.pane_id)
            .filter(|pane_id| self.panes.contains_key(pane_id))
    }

    /// The chosen agent's pane when it belongs to another session.
    fn fleet_remote_pane(&self) -> Option<PaneId> {
        self.fleet_selected()
            .filter(|_| !self.fleet_wall_open())
            .map(|agent| agent.pane_id)
            .filter(|pane_id| !self.panes.contains_key(pane_id))
    }

    /// The tab the chosen agent's pane is in, when it is in this session.
    pub(super) fn fleet_chosen_tab(&self) -> Option<TabId> {
        self.tab_of_pane(self.fleet_selected()?.pane_id)
    }

    /// Whether an agent in one of the tab's panes waits on the user.
    pub(super) fn fleet_waits_in_tab(&self, tab_id: TabId) -> bool {
        self.fleet.iter().any(|agent| {
            agent.state == FleetState::Blocked && self.tab_of_pane(agent.pane_id) == Some(tab_id)
        })
    }

    /// What a pane's program said, with its paths said from the project
    /// of the agent in the pane, when one is.
    pub(super) fn fleet_said_in(&self, pane_id: PaneId, said: &str) -> String {
        self.fleet_agent_in(pane_id)
            .and_then(|agent| fleet_said(agent, Some(said)))
            .unwrap_or_else(|| said.to_owned())
    }

    /// The panes on show that belong to other sessions, whose text is
    /// asked for: every such agent's on the grid, the chosen one's beside
    /// the roster.
    fn fleet_remote_panes(&self) -> Vec<PaneId> {
        if self.fleet_wall_open() {
            self.fleet
                .iter()
                .map(|agent| agent.pane_id)
                .filter(|pane_id| !self.panes.contains_key(pane_id))
                .collect()
        } else {
            self.fleet_remote_pane().into_iter().collect()
        }
    }

    pub(super) fn fleet_open(&self) -> bool {
        self.fleet_view.is_some()
    }

    /// Whether the roster, rather than a pane, has the keys.
    pub(super) fn fleet_roster_has_keys(&self) -> bool {
        self.fleet_view.as_ref().is_some_and(|view| !view.in_pane)
    }

    /// The tab a pane of this session is in.
    fn tab_of_pane(&self, pane_id: PaneId) -> Option<TabId> {
        self.session
            .as_ref()?
            .tabs
            .iter()
            .find(|tab| tab.layout.contains(pane_id))
            .map(|tab| tab.id)
    }

    /// The tab whose ink the view wears: the chosen agent's.
    pub(super) fn fleet_ink_tab(&self) -> Option<TabId> {
        self.fleet_view.as_ref()?;
        self.tab_of_pane(self.fleet_selected()?.pane_id)
    }

    /// How many agents are waiting on the user.
    fn fleet_waiting(&self) -> usize {
        self.fleet
            .iter()
            .filter(|agent| agent.state.wants_user())
            .count()
    }

    /// ⌘⇧A: lay the fleet over the panes, or put it away.
    pub(super) fn toggle_fleet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.fleet_view.is_some() {
            self.close_fleet(true, window, cx);
        } else {
            self.open_fleet(window, cx);
        }
    }

    pub(super) fn open_fleet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.fleet_view.is_some() {
            return;
        }
        self.finish_tab_rename(true, window, cx);
        self.palette = None;
        self.session_sheet = None;
        self.settings_sheet = None;
        self.quick_select = None;
        self.pane_menu = None;
        self.mode = InputMode::Normal;
        let focus = cx.focus_handle();
        let deferred = focus.clone();
        window.on_next_frame(move |window, cx| deferred.focus(window, cx));
        // It opens on the agent in the pane with the keys, else on the
        // first that waits, else at the top.
        let here = self
            .terminal_input_pane_id()
            .filter(|pane_id| self.fleet.iter().any(|agent| agent.pane_id == *pane_id));
        let order = roster_order(&self.fleet);
        let waiting = order.iter().copied().find(|pane_id| {
            self.fleet
                .iter()
                .any(|agent| agent.pane_id == *pane_id && agent.state.wants_user())
        });
        let active = self.active_tab_id();
        self.fleet_view = Some(FleetView {
            focus,
            selected: here.or(waiting),
            selected_at: Instant::now(),
            in_pane: false,
            came_from: active,
            under_tab: active,
            expected_tab: None,
            expects_new_tab: false,
            card: None,
            scroll: ScrollHandle::new(),
            remote_text: HashMap::new(),
            awaiting: None,
            drawn_pane: None,
            wall: false,
            wall_shown: Vec::new(),
            wall_columns: 1,
        });
        // The slabs are simply there, not gliding in from the tab's.
        self.layout_motion = motion::LayoutMotion::default();
        // Events may have been missed while it was shut.
        self.backend.send(CommandMessage::ListFleet);
        Self::refresh_fleet_hooks(cx);
        Self::find_fleet_launchers(cx);
        self.ask_remote_text();
        cx.notify();
    }

    /// Put the view away, going back to the tab it was opened over when
    /// `back` says so.
    pub(super) fn close_fleet(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        let was_in_view = self.panes_in_view();
        let Some(view) = self.fleet_view.take() else {
            return;
        };
        // The tab's panes took in their output unseen meanwhile.
        self.publish_panes_come_into_view(&was_in_view);
        self.fleet_start = None;
        let active = self.active_tab_id();
        if back
            && let Some(tab_id) = view.came_from
            && Some(tab_id) != active
            && self
                .session
                .as_ref()
                .is_some_and(|session| session.tabs.iter().any(|tab| tab.id == tab_id))
        {
            self.pending_focused_pane = None;
            self.send_workspace(WorkspaceCommand::SelectTab(tab_id));
        }
        self.layout_motion = motion::LayoutMotion::default();
        self.restore_keyboard(window, cx);
        cx.notify();
    }

    fn select_fleet_agent(&mut self, pane_id: Option<PaneId>, cx: &mut Context<Self>) {
        let row = pane_id.and_then(|pane_id| roster_list_index(&self.fleet, pane_id));
        let was_in_view = self.panes_in_view();
        let Some(view) = self.fleet_view.as_mut() else {
            return;
        };
        if view.selected == pane_id {
            return;
        }
        view.selected = pane_id;
        view.selected_at = Instant::now();
        if let Some(row) = row {
            view.scroll.scroll_to_item(row);
        }
        self.ask_remote_text();
        // The roster's one pane changes with the choice.
        self.publish_panes_come_into_view(&was_in_view);
        // Its pane is sized, and its news read, once it has been chosen a
        // moment.
        for wait in [SETTLE, SEEN_AFTER] {
            cx.spawn(async move |entity, cx| {
                cx.background_executor().timer(wait).await;
                let _ = entity.update(cx, |_, cx| cx.notify());
            })
            .detach();
        }
        cx.notify();
    }

    fn move_fleet_choice(&mut self, delta: isize, cx: &mut Context<Self>) {
        let order = roster_order(&self.fleet);
        if order.is_empty() {
            return;
        }
        let at = self
            .fleet_selected()
            .and_then(|agent| order.iter().position(|pane_id| *pane_id == agent.pane_id))
            .unwrap_or(0);
        let next = wrapping_step(at, delta, order.len());
        self.select_fleet_agent(Some(order[next]), cx);
    }

    /// Tab and Shift+Tab: the next agent that waits on the user.
    fn choose_next_waiting(&mut self, forward: bool, cx: &mut Context<Self>) {
        let order = self.fleet_order();
        let from = self.fleet_selected().map(|agent| agent.pane_id);
        let next = next_waiting(
            &order,
            |pane_id| {
                self.fleet
                    .iter()
                    .any(|agent| agent.pane_id == pane_id && agent.state.wants_user())
            },
            from,
            forward,
        );
        match next {
            Some(pane_id) => self.select_fleet_agent(Some(pane_id), cx),
            None => self.say_in_strip("no agent is waiting on you", false, cx),
        }
    }

    /// `v`: turn the view into the wall, or back into the roster. The agent
    /// chosen in one is the one chosen in the other.
    pub(super) fn set_fleet_wall(&mut self, wall: bool, cx: &mut Context<Self>) {
        if wall && self.fleet.is_empty() {
            self.say_in_strip("no agents to show yet", false, cx);
            return;
        }
        let chosen = self.fleet_selected().map(|agent| agent.pane_id);
        let was_in_view = self.panes_in_view();
        let Some(view) = self.fleet_view.as_mut() else {
            return;
        };
        if view.wall == wall {
            return;
        }
        view.wall = wall;
        view.card = None;
        view.in_pane = false;
        view.selected = chosen;
        view.selected_at = Instant::now();
        if !wall {
            view.wall_shown.clear();
        }
        self.publish_panes_come_into_view(&was_in_view);
        self.ask_remote_text();
        cx.notify();
    }

    /// Open the view as the wall.
    pub(super) fn open_fleet_wall(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_fleet(window, cx);
        if self.fleet_view.as_ref().is_some_and(|view| view.in_pane) {
            self.leave_fleet_pane(window, cx);
        }
        self.set_fleet_wall(true, cx);
    }

    /// An arrow on the wall: the tile that way.
    fn move_wall_choice(&mut self, key: &str, cx: &mut Context<Self>) {
        let order = wall_order(&self.fleet);
        let Some(view) = self.fleet_view.as_ref() else {
            return;
        };
        let at = self
            .fleet_selected()
            .and_then(|agent| order.iter().position(|pane_id| *pane_id == agent.pane_id))
            .unwrap_or(0);
        let next = wall_step(at, order.len(), view.wall_columns, key);
        if let Some(pane_id) = order.get(next) {
            self.select_fleet_agent(Some(*pane_id), cx);
        }
    }

    /// Return on the wall: back to the roster and into the chosen agent's
    /// pane, to type to it.
    fn enter_wall_tile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_fleet_wall(false, cx);
        self.enter_fleet_pane(window, cx);
    }

    /// Ask for what the chosen agent's pane shows, when it belongs to
    /// another session and so has no terminal here.
    fn ask_remote_text(&self) {
        for pane_id in self.fleet_remote_panes() {
            self.backend.send(CommandMessage::PaneText {
                pane_id,
                lines: REMOTE_LINES,
            });
        }
    }

    pub(super) fn set_fleet_pane_text(&mut self, pane_id: PaneId, text: String) {
        let shown = self.fleet_remote_panes();
        if let Some(view) = self.fleet_view.as_mut() {
            view.remote_text
                .retain(|pane_id, _| shown.contains(pane_id));
            if shown.contains(&pane_id) {
                view.remote_text.insert(pane_id, text);
            }
        }
    }

    /// Step from the roster into the chosen agent's pane: its tab becomes
    /// the one beneath the view and the pane takes the keys, so every key
    /// reaches the agent as it would in its own tab.
    fn enter_fleet_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(agent) = self.fleet_selected().cloned() else {
            return;
        };
        let pane_id = agent.pane_id;
        if !self.panes.contains_key(&pane_id) {
            // A pane of another session: go to it there.
            self.go_to_chosen_agent(window, cx);
            return;
        }
        let Some(tab_id) = self.tab_of_pane(pane_id) else {
            return;
        };
        // An agent surface laid over the pane steps aside for its terminal.
        if self.agent_panes.get(&tab_id) == Some(&pane_id) {
            self.agent_panes.remove(&tab_id);
        }
        if Some(tab_id) != self.active_tab_id() {
            self.send_workspace(WorkspaceCommand::SelectTab(tab_id));
            if let Some(view) = self.fleet_view.as_mut() {
                view.expected_tab = Some(tab_id);
            }
        }
        self.request_pane_focus(pane_id);
        if let Some(view) = self.fleet_view.as_mut() {
            view.selected = Some(pane_id);
            view.in_pane = true;
            view.card = None;
        }
        self.focus_handle.focus(window, cx);
        self.restart_cursor_blink();
        cx.notify();
    }

    /// A click in the view's pane steps into it, as Return does.
    pub(super) fn enter_fleet_pane_by_pointer(
        &mut self,
        pane_id: PaneId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.fleet_roster_has_keys() && self.fleet_shown_pane() == Some(pane_id) {
            self.enter_fleet_pane(window, cx);
        }
    }

    pub(super) fn focus_fleet_roster(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = &self.fleet_view {
            view.focus.focus(window, cx);
        }
    }

    /// Whether the roster's own element has the focus yet: it takes it a
    /// frame after the view opens.
    pub(super) fn fleet_roster_focused(&self, window: &Window) -> bool {
        self.fleet_view
            .as_ref()
            .is_some_and(|view| view.focus.is_focused(window))
    }

    /// Step back from the agent's pane to the roster.
    fn leave_fleet_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.fleet_view.as_mut() else {
            return;
        };
        view.in_pane = false;
        view.focus.focus(window, cx);
        self.mode = InputMode::Normal;
        cx.notify();
    }

    /// A key while the agent's pane has the keys. ⌥← steps back to the
    /// roster, and the other ways between panes stay put, since the view
    /// has only the one; everything else is the agent's.
    pub(super) fn fleet_pane_key(
        &mut self,
        keystroke: &gpui::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.fleet_view.as_ref().is_some_and(|view| view.in_pane)
            || self.mode != InputMode::Normal
        {
            return false;
        }
        let Some(chord) = key_chord(keystroke) else {
            return false;
        };
        match self.keymap.resolve(InputMode::Normal, chord) {
            Some(Action::FocusPaneOrTab(Direction::Left) | Action::FocusPane(Direction::Left)) => {
                self.leave_fleet_pane(window, cx);
                true
            }
            Some(Action::FocusPaneOrTab(_) | Action::FocusPane(_)) => true,
            _ => false,
        }
    }

    /// `g`: go to the chosen agent's own tab and pane, in whatever session,
    /// and put the view away.
    fn go_to_chosen_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(agent) = self.fleet_selected().cloned() else {
            return;
        };
        self.close_fleet(false, window, cx);
        self.go_to_fleet_agent(agent.pane_id, agent.session_id, cx);
    }

    /// Go to an agent's pane: its tab is shown and the pane takes the keys.
    /// One in another session is gone to once that session is attached.
    pub(super) fn go_to_fleet_agent(
        &mut self,
        pane_id: PaneId,
        session_id: SessionId,
        cx: &mut Context<Self>,
    ) {
        if self.session.as_ref().map(|session| session.id) != Some(session_id) {
            self.fleet_goto = Some(pane_id);
            self.backend.send(CommandMessage::AttachSession(session_id));
            return;
        }
        let Some(tab_id) = self.tab_of_pane(pane_id) else {
            return;
        };
        if self.agent_panes.get(&tab_id) == Some(&pane_id) {
            self.agent_panes.remove(&tab_id);
        }
        if Some(tab_id) != self.active_tab_id() {
            self.send_workspace(WorkspaceCommand::SelectTab(tab_id));
        }
        self.request_pane_focus(pane_id);
        cx.notify();
    }

    /// The agent "go to the pane that called" goes to: the one that has
    /// waited on the user the shortest while, other than where the keys are.
    pub(super) fn fleet_agent_calling(&self) -> Option<(PaneId, SessionId)> {
        let here = self.terminal_input_pane_id().filter(|_| !self.fleet_open());
        self.fleet
            .iter()
            .filter(|agent| agent.state.wants_user() && Some(agent.pane_id) != here)
            .max_by_key(|agent| agent.since_ms)
            .map(|agent| (agent.pane_id, agent.session_id))
    }

    /// `r`: type what picks an ended agent's session up again into its
    /// pane's shell, which has to be at its prompt.
    fn resume_chosen_agent(&mut self, cx: &mut Context<Self>) {
        let Some(agent) = self.fleet_selected().cloned() else {
            return;
        };
        let name = agent_name(agent.kind);
        if agent.state != FleetState::Ended {
            self.say_in_strip(format!("{name} is still running"), false, cx);
            return;
        }
        let Some(command) = agent.resume_command() else {
            self.say_in_strip(format!("{name} left no session to resume"), true, cx);
            return;
        };
        let Some(pane) = self.panes.get(&agent.pane_id) else {
            self.say_in_strip("its pane is in another session · g goes there", false, cx);
            return;
        };
        if self.pane_exits.contains_key(&agent.pane_id) {
            self.say_in_strip("its pane's shell has exited", true, cx);
            return;
        }
        if let Some(program) = &pane.program {
            self.say_in_strip(format!("{program} is running in its pane"), true, cx);
            return;
        }
        self.backend.send(CommandMessage::Write {
            pane_id: agent.pane_id,
            bytes: format!("{command}\r").into_bytes(),
        });
    }

    /// `x`: let an ended agent's session go.
    fn forget_chosen_agent(&mut self, cx: &mut Context<Self>) {
        let Some(agent) = self.fleet_selected() else {
            return;
        };
        if agent.state != FleetState::Ended {
            let name = agent_name(agent.kind);
            self.say_in_strip(format!("{name} is still running"), false, cx);
            return;
        }
        self.backend.send(CommandMessage::ForgetFleetAgent {
            pane_id: agent.pane_id,
        });
    }

    /// Look for the agents on this machine, for `n` to offer.
    fn find_fleet_launchers(cx: &mut Context<Self>) {
        cx.spawn(async move |entity, cx| {
            let found = cx
                .background_executor()
                .spawn(async { find_launchers() })
                .await;
            let _ = entity.update(cx, |this, cx| {
                this.fleet_launchers = Some(found);
                cx.notify();
            });
        })
        .detach();
    }

    /// Read whether Mux's hooks are in each agent it has them for.
    pub(super) fn refresh_fleet_hooks(cx: &mut Context<Self>) {
        cx.spawn(async move |entity, cx| {
            let statuses = cx
                .background_executor()
                .spawn(async { hook_statuses() })
                .await;
            let _ = entity.update(cx, |this, cx| {
                if this.fleet_hooks != statuses {
                    this.fleet_hooks = statuses;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The agents that are set up here without Mux's hooks, or with old ones.
    fn unhooked_agents(&self) -> Vec<AgentKind> {
        self.fleet_hooks
            .iter()
            .filter(|(_, status)| matches!(status, HookStatus::Missing | HookStatus::Outdated))
            .map(|(kind, _)| *kind)
            .collect()
    }

    /// Open the start card: over the roster, with the view opened first
    /// when it is shut. `here` starts the agent where the keys were.
    pub(super) fn open_fleet_start(
        &mut self,
        here: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_fleet(window, cx);
        if self.fleet_view.as_ref().is_some_and(|view| view.in_pane) {
            self.leave_fleet_pane(window, cx);
        }
        if let Some(view) = self.fleet_view.as_mut() {
            view.card = Some(Card::Start { choice: 0, here });
        }
        if self.fleet_launchers.as_ref().is_none_or(Vec::is_empty) {
            Self::find_fleet_launchers(cx);
        }
        cx.notify();
    }

    /// Open the hooks card, the view first when it is shut.
    pub(super) fn open_fleet_hooks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_fleet(window, cx);
        if self.fleet_view.as_ref().is_some_and(|view| view.in_pane) {
            self.leave_fleet_pane(window, cx);
        }
        // It opens on the first agent there is something to do for.
        let choice = self
            .fleet_hooks
            .iter()
            .position(|(_, status)| matches!(status, HookStatus::Missing | HookStatus::Outdated))
            .unwrap_or(0);
        if let Some(view) = self.fleet_view.as_mut() {
            view.card = Some(Card::Hooks { choice });
        }
        Self::refresh_fleet_hooks(cx);
        cx.notify();
    }

    /// Start `kind` in a tab of its own: the tab is asked for, and the
    /// command typed once its shell is up. It starts in the chosen agent's
    /// project unless `here`, when it starts where the keys were.
    fn start_fleet_agent(&mut self, kind: AgentKind, here: bool, cx: &mut Context<Self>) {
        let root = self
            .fleet_selected()
            .filter(|_| !here)
            .map(|agent| agent.project.root.clone())
            .filter(|root| root.is_dir());
        let command = match root {
            Some(root) => format!("cd {} && {}", shell_quoted(&root), kind.command()),
            None => kind.command().to_owned(),
        };
        self.fleet_start = Some(FleetStart {
            command,
            known: self.panes.keys().copied().collect(),
        });
        if let Some(view) = self.fleet_view.as_mut() {
            view.card = None;
            view.expects_new_tab = true;
        }
        self.send_workspace(WorkspaceCommand::NewTab);
        cx.notify();
    }

    /// The tab asked for has come: type the agent's command into its shell
    /// once the shell has shown its prompt, or after a while regardless.
    fn type_fleet_start(&mut self, cx: &mut Context<Self>) {
        let Some(pane_id) = self.focused_pane_id() else {
            return;
        };
        if self
            .fleet_start
            .as_ref()
            .is_none_or(|start| start.known.contains(&pane_id))
        {
            return;
        }
        let Some(start) = self.fleet_start.take() else {
            return;
        };
        if let Some(view) = self.fleet_view.as_mut() {
            view.awaiting = Some(pane_id);
        }
        let began = Instant::now();
        cx.spawn(async move |entity, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(80))
                    .await;
                let ready = entity
                    .update(cx, |this, _| {
                        this.panes
                            .get(&pane_id)
                            .is_none_or(|pane| pane.title.title().is_some())
                    })
                    .unwrap_or(true);
                if ready || began.elapsed() >= SHELL_WAIT {
                    break;
                }
            }
            let _ = entity.update(cx, |this, _| {
                if this.panes.contains_key(&pane_id) {
                    this.backend.send(CommandMessage::Write {
                        pane_id,
                        bytes: format!("{}\r", start.command).into_bytes(),
                    });
                }
            });
        })
        .detach();
    }

    /// Put Mux's hooks into an agent, or take them out, and say what came
    /// of it and what the agent needs next.
    fn set_fleet_hooks(kind: AgentKind, hooked: bool, cx: &mut Context<Self>) {
        cx.spawn(async move |entity, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let homes = Homes::from_environment()
                        .ok_or_else(|| "no home directory to find its settings in".to_owned())?;
                    if hooked {
                        install::install(kind, &homes)
                    } else {
                        install::uninstall(kind, &homes)
                    }
                    .map_err(|error| error.to_string())
                })
                .await;
            let _ = entity.update(cx, |this, cx| {
                let name = agent_name(kind);
                match result {
                    Ok(changed) => {
                        let said = match (hooked, changed, kind) {
                            (true, _, AgentKind::Codex) => {
                                "hooked codex · it asks to trust them when next started".to_owned()
                            }
                            (true, _, AgentKind::Copilot) => {
                                "hooked copilot · restart copilot to load it".to_owned()
                            }
                            (true, true, _) => format!("hooked {name}"),
                            (true, false, _) => format!("{name} is already hooked"),
                            (false, true, _) => format!("took mux's hooks out of {name}"),
                            (false, false, _) => format!("{name} had no hooks of mux's"),
                        };
                        this.say_in_strip(said, false, cx);
                    }
                    Err(error) => {
                        this.say_in_strip(format!("couldn't hook {name}: {error}"), true, cx);
                    }
                }
                Self::refresh_fleet_hooks(cx);
            });
        })
        .detach();
    }

    /// The workspace changed beneath the view. A tab it asked for is taken
    /// in its stride; any other change of tab, or the keys going to a pane
    /// the view does not show, is the user at work in the workspace, and
    /// the view gets out of the way.
    pub(super) fn fleet_follow_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(pane_id) = self.fleet_goto
            && self.panes.contains_key(&pane_id)
            && let Some(session_id) = self.session.as_ref().map(|session| session.id)
        {
            self.fleet_goto = None;
            self.go_to_fleet_agent(pane_id, session_id, cx);
        }
        let active = self.active_tab_id();
        let Some(view) = self.fleet_view.as_mut() else {
            return;
        };
        if view.expects_new_tab && active != view.under_tab {
            view.expects_new_tab = false;
            view.expected_tab = None;
            view.under_tab = active;
            self.type_fleet_start(cx);
        } else if view.expected_tab.is_some() && active == view.expected_tab {
            view.expected_tab = None;
            view.under_tab = active;
        } else if active != view.under_tab && view.expected_tab.is_none() {
            self.close_fleet(false, window, cx);
            return;
        }
        let Some(view) = self.fleet_view.as_ref() else {
            return;
        };
        if !view.in_pane {
            return;
        }
        let chosen = view.selected;
        if chosen.is_none_or(|pane_id| !self.panes.contains_key(&pane_id)) {
            // Its pane has closed: back to the roster.
            self.leave_fleet_pane(window, cx);
        } else if self.terminal_input_pane_id() != chosen {
            self.close_fleet(false, window, cx);
        }
    }

    /// A real tab's chip was clicked while the view was open: the view
    /// goes, and the tab is shown.
    pub(super) fn leave_fleet_for_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.fleet_view.is_some() {
            self.pending_focused_pane = None;
            self.close_fleet(false, window, cx);
        }
    }

    /// The pane being looked at: the view's while it is open and has been
    /// on it a moment, else the one with the keys.
    pub(super) fn looked_at_pane(&self) -> Option<PaneId> {
        match &self.fleet_view {
            Some(view) if !view.in_pane => self
                .fleet_shown_pane()
                .filter(|_| view.selected_at.elapsed() >= SEEN_AFTER),
            _ => self.terminal_input_pane_id(),
        }
    }

    /// An agent that finished in the pane being looked at is no longer
    /// news: tell the daemon, once.
    pub(super) fn mark_fleet_seen(&mut self, window: &Window) {
        if !window.is_window_active() {
            return;
        }
        let Some(pane_id) = self.looked_at_pane() else {
            return;
        };
        let done = self
            .fleet
            .iter()
            .any(|agent| agent.pane_id == pane_id && agent.state == FleetState::Done);
        if done && self.fleet_seen_sent.insert(pane_id) {
            self.backend.send(CommandMessage::FleetSeen { pane_id });
        }
    }

    /// Whether a working agent's dot is on screen, breathing.
    pub(super) fn fleet_pulse_wanted(&self) -> bool {
        self.fleet_view.is_some()
            && self.motion == MotionPreference::Full
            && self
                .fleet
                .iter()
                .any(|agent| agent.state == FleetState::Working)
    }

    /// Keep the roster's ages, and another session's pane, up to date while
    /// the view is open: a timer to the next moment something reads
    /// differently, and none while it is shut.
    pub(super) fn ensure_fleet_clock(&mut self, cx: &mut Context<Self>) {
        if self.fleet_clock_running || self.fleet_view.is_none() {
            return;
        }
        self.fleet_clock_running = true;
        cx.spawn(async move |entity, cx| {
            loop {
                let Ok(Some(wait)) = entity.update(cx, |this, _| this.next_fleet_reading()) else {
                    break;
                };
                cx.background_executor().timer(wait).await;
                let open = entity
                    .update(cx, |this, cx| {
                        if this.fleet_view.is_none() {
                            return false;
                        }
                        this.ask_remote_text();
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !open {
                    break;
                }
            }
            let _ = entity.update(cx, |this, _| this.fleet_clock_running = false);
        })
        .detach();
    }

    /// How long until the view next reads differently by itself.
    fn next_fleet_reading(&self) -> Option<Duration> {
        self.fleet_view.as_ref()?;
        if !self.fleet_remote_panes().is_empty() {
            return Some(Duration::from_secs(1));
        }
        let now = now_ms();
        Some(
            self.fleet
                .iter()
                .filter(|agent| agent.state != FleetState::Idle)
                .map(|agent| until_age_changes(agent.since_ms, now))
                .min()
                .unwrap_or(Duration::from_mins(1))
                .max(Duration::from_millis(50)),
        )
    }

    /// What a pane's head says of the agent in it: its name, what it was
    /// asked, and its state in the head's words, when it has said one.
    pub(super) fn fleet_head(&self, pane_id: PaneId) -> Option<FleetHead> {
        let agent = self.fleet_agent_in(pane_id)?;
        let state = match agent.state {
            FleetState::Blocked | FleetState::Error => Some((agent.state.word(), color(SIGNAL))),
            FleetState::Working => Some((agent.state.word(), color(MUTED_TEXT))),
            FleetState::Done => Some((agent.state.word(), self.active_ink().color())),
            FleetState::Idle => Some((agent.state.word(), color(FAINT_TEXT))),
            FleetState::Unknown | FleetState::Ended => None,
        };
        Some((agent_name(agent.kind), agent.task.clone(), state))
    }

    /// Where the view's parts go in a window this size.
    pub(super) fn fleet_layout(&self, width: f32, height: f32) -> Option<FleetLayout> {
        let view = self.fleet_view.as_ref()?;
        let top = layout::TAB_BAR_HEIGHT + layout::WORKSPACE_TOP_GAP;
        let bounds = layout::Rect {
            x: layout::WORKSPACE_INSET,
            y: top,
            width: (width - layout::WORKSPACE_INSET * 2.0).max(1.0),
            height: (height - top - layout::WORKSPACE_INSET).max(1.0),
        };
        let roster_width = (bounds.width * ROSTER_SHARE)
            .clamp(ROSTER_MIN, ROSTER_MAX)
            .min((bounds.width - PANE_MIN - layout::PANE_GAP).max(ROSTER_MIN.min(bounds.width)))
            .round();
        let roster = layout::Rect {
            width: roster_width,
            ..bounds
        };
        let detail = layout::Rect {
            x: bounds.x + roster_width + layout::PANE_GAP,
            width: (bounds.width - roster_width - layout::PANE_GAP).max(1.0),
            ..bounds
        };
        let chosen = self.fleet_selected();
        let wall = view.wall.then(|| self.wall_layout(bounds));
        let ended = wall.is_none() && chosen.is_some_and(|agent| agent.state == FleetState::Ended);
        let card = ended.then_some(layout::Rect {
            height: ENDED_CARD_HEIGHT.min(detail.height),
            ..detail
        });
        let pane_frame = card.map_or(detail, |card| layout::Rect {
            y: detail.y + card.height + layout::PANE_GAP,
            height: (detail.height - card.height - layout::PANE_GAP).max(1.0),
            ..detail
        });
        let pane = self.fleet_shown_pane().map(|pane_id| {
            let head = if self
                .find
                .as_ref()
                .is_some_and(|find| find.pane_id == pane_id)
            {
                layout::PANE_HEAD_HEIGHT
            } else {
                layout::PANE_LEGEND_TOP
            };
            layout::PaneGeometry {
                pane_id,
                frame: pane_frame,
                rect: layout::surface_of(pane_frame, head),
                head,
                focused: view.in_pane,
            }
        });
        Some(FleetLayout {
            roster,
            detail,
            card,
            pane,
            wall,
        })
    }

    /// Where the wall's tiles go in `bounds`: the page with the chosen
    /// agent on it, in rows, over a foot.
    fn wall_layout(&self, bounds: layout::Rect) -> WallLayout {
        let gap = layout::PANE_GAP;
        let foot = layout::Rect {
            y: bounds.y + bounds.height - WALL_FOOT,
            height: WALL_FOOT,
            ..bounds
        };
        let room = layout::Rect {
            height: (bounds.height - WALL_FOOT - gap).max(1.0),
            ..bounds
        };
        let order = wall_order(&self.fleet);
        let (columns, rows) = wall_grid(order.len(), room.width, room.height);
        let per_page = columns * rows;
        let pages = order.len().div_ceil(per_page).max(1);
        let chosen = self
            .fleet_selected()
            .and_then(|agent| order.iter().position(|pane_id| *pane_id == agent.pane_id))
            .unwrap_or(0);
        let page = chosen / per_page;
        let tile_width = (room.width - gap * (columns - 1) as f32) / columns as f32;
        let tile_height = (room.height - gap * (rows - 1) as f32) / rows as f32;
        let tiles = order
            .iter()
            .skip(page * per_page)
            .take(per_page)
            .enumerate()
            .map(|(index, pane_id)| WallTile {
                pane_id: *pane_id,
                frame: layout::Rect {
                    x: (room.x + (tile_width + gap) * (index % columns) as f32).round(),
                    y: (room.y + (tile_height + gap) * (index / columns) as f32).round(),
                    width: tile_width.floor(),
                    height: tile_height.floor(),
                },
            })
            .collect();
        WallLayout {
            tiles,
            columns,
            foot,
            page,
            pages,
        }
    }

    /// Take in the wall as laid out this frame: its panes are the ones in
    /// view, and one that has just come onto it is drawn afresh.
    pub(super) fn publish_fleet_wall(&mut self, layout: &FleetLayout) {
        let Some(view) = self.fleet_view.as_mut() else {
            return;
        };
        let Some(wall) = &layout.wall else {
            view.wall_shown.clear();
            return;
        };
        let shown: Vec<PaneId> = wall.tiles.iter().map(|tile| tile.pane_id).collect();
        view.wall_columns = wall.columns;
        let was = std::mem::replace(&mut view.wall_shown, shown.clone());
        for pane_id in shown {
            if !was.contains(&pane_id)
                && let Some(pane) = self.panes.get_mut(&pane_id)
            {
                let _ = pane.publish_frame();
            }
        }
    }

    /// The metrics a pane of `cols` is drawn with in a tile's body: the
    /// terminal's own face, as large as shows the pane's whole width but
    /// never too small to read, in quarter-point steps so tiles of a size
    /// share what they shape. What does not fit then is cropped, not shrunk.
    fn tile_metrics(
        &mut self,
        cols: u16,
        body: layout::Rect,
        cx: &mut Context<Self>,
    ) -> gpui_terminal::GridMetrics {
        let full = self.metrics;
        let pad = full.padding_x * 2.0;
        let scale = ((body.width - pad) / (full.cell_width * f32::from(cols.max(1)))).min(1.0);
        let size = ((full.font_size * scale * 4.0).floor() / 4.0)
            .max(TILE_MIN_FONT)
            .min(full.font_size);
        *self
            .fleet_tile_metrics
            .entry((self.terminal_font.clone(), size.to_bits()))
            .or_insert_with(|| {
                gpui_terminal::GridMetrics::from_font(&self.terminal_font, size, cx.text_system())
            })
    }

    /// The terminal views of the wall's tiles, each pane drawn small. A
    /// view is kept while nothing it draws has changed, as a pane's is.
    pub(super) fn fleet_tile_panes(
        &mut self,
        layout: &FleetLayout,
        cx: &mut Context<Self>,
    ) -> HashMap<PaneId, TilePane> {
        let mut tiles = HashMap::new();
        let Some(wall) = &layout.wall else {
            return tiles;
        };
        for tile in &wall.tiles {
            let pane_id = tile.pane_id;
            let Some((cols, rows)) = self
                .panes
                .get(&pane_id)
                .map(|pane| (pane.frame.cols, pane.frame.rows))
            else {
                continue;
            };
            let metrics = self.tile_metrics(cols, tile.body(), cx);
            let Some(pane) = self.panes.get(&pane_id) else {
                continue;
            };
            let props = gpui_terminal::TerminalProps {
                frame_serial: pane.frame_serial,
                font_family: self.terminal_font.clone(),
                metrics,
                // Nothing is typed into a tile, so it shows no cursor.
                focused: false,
                chrome: gpui_terminal::TerminalChrome {
                    surface: color(SURFACE),
                    cursor: color(MUTED_TEXT),
                    hollow_cursor: false,
                },
                marks: Rc::default(),
                opacity: 1.0,
            };
            let view = match self.terminal_views.get(&pane_id) {
                Some(view) if view.read(cx).props() == &props => view.clone(),
                _ => {
                    let frame = Rc::clone(&pane.frame);
                    let cache = Rc::clone(&pane.render_cache);
                    let view = cx.new(|_| gpui_terminal::TerminalView::new(props, &frame, cache));
                    self.terminal_views.insert(pane_id, view.clone());
                    view
                }
            };
            tiles.insert(
                pane_id,
                TilePane {
                    view,
                    width: metrics.cell_width * f32::from(cols) + metrics.padding_x * 2.0,
                    height: metrics.cell_height * f32::from(rows) + metrics.padding_y * 2.0,
                    foot: metrics.cell_height * f32::from(rows_in_use(&pane.frame))
                        + metrics.padding_y * 2.0,
                },
            );
        }
        tiles
    }

    /// What the wall adds to the ground's light: a waiting agent's tile
    /// calls in peach, as a pane that calls does, and a working one is busy.
    pub(super) fn fleet_wall_lights(
        &self,
        viewport: gpui::Size<gpui::Pixels>,
        calls: &mut Vec<ground::Call>,
        busy: &mut Vec<(layout::Rect, f32)>,
    ) {
        let Some(wall) = self
            .fleet_layout(f32::from(viewport.width), f32::from(viewport.height))
            .and_then(|layout| layout.wall)
        else {
            return;
        };
        for tile in &wall.tiles {
            let Some(agent) = self
                .fleet
                .iter()
                .find(|agent| agent.pane_id == tile.pane_id)
            else {
                continue;
            };
            if agent.state.wants_user() {
                calls.push(ground::Call {
                    caller: ground::Caller::Pane(tile.pane_id),
                    from: tile.frame,
                    reach: ground::PANE_REACH,
                    ink: None,
                });
            } else if agent.state == FleetState::Working {
                busy.push((tile.frame, 1.0));
            }
        }
    }

    /// A pane the view comes to show took in its output unseen until now:
    /// draw it afresh.
    pub(super) fn publish_fleet_pane(&mut self) {
        let shown = self.fleet_shown_pane();
        let Some(view) = self.fleet_view.as_mut() else {
            return;
        };
        if std::mem::replace(&mut view.drawn_pane, shown) != shown
            && let Some(pane) = shown.and_then(|pane_id| self.panes.get_mut(&pane_id))
        {
            let _ = pane.publish_frame();
        }
    }

    /// Whether the chosen agent's pane takes the view's size yet: once it
    /// has been chosen a moment, or at once when it is stepped into.
    pub(super) fn fleet_pane_settled(&self) -> bool {
        self.fleet_view
            .as_ref()
            .is_some_and(|view| view.in_pane || view.selected_at.elapsed() >= SETTLE)
    }

    /// The slabs the view lays on the ground, and the one of its own that
    /// has the keys.
    pub(super) fn fleet_ground(
        &self,
        viewport: gpui::Size<gpui::Pixels>,
    ) -> Option<(Vec<layout::Rect>, Option<layout::Rect>)> {
        let layout = self.fleet_layout(f32::from(viewport.width), f32::from(viewport.height))?;
        if let Some(wall) = &layout.wall {
            let chosen = self.fleet_selected().map(|agent| agent.pane_id);
            let mut slabs: Vec<_> = wall.tiles.iter().map(|tile| tile.frame).collect();
            slabs.push(wall.foot);
            let focus = wall
                .tiles
                .iter()
                .find(|tile| Some(tile.pane_id) == chosen)
                .map(|tile| tile.frame);
            return Some((slabs, focus));
        }
        let mut slabs = vec![layout.roster];
        slabs.extend(layout.card);
        if layout.pane.is_none() && self.fleet_selected().is_some() {
            slabs.push(layout.card.map_or(layout.detail, |card| layout::Rect {
                y: layout.detail.y + card.height + layout::PANE_GAP,
                height: (layout.detail.height - card.height - layout::PANE_GAP).max(1.0),
                ..layout.detail
            }));
        }
        let focus = self.fleet_roster_has_keys().then_some(layout.roster);
        Some((slabs, focus))
    }

    /// Where the fleet's chip sits in the strip, near enough for the
    /// ground's light to start from.
    pub(super) fn fleet_chip_rect() -> layout::Rect {
        let lead = if cfg!(target_os = "macos") {
            82.0
        } else {
            10.0
        };
        layout::Rect {
            x: lead,
            y: (layout::TAB_BAR_HEIGHT - 26.0) / 2.0,
            width: 20.0 + 6.0 + 8.0 + 5.0 * STRIP_CHAR_WIDTH,
            height: 26.0,
        }
    }

    /// The fleet's chip at the head of the strip: the tab on screen while
    /// the view is open; otherwise quiet, there only while there are
    /// agents, with a peach dot and a count when any waits on the user.
    pub(super) fn render_fleet_chip(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let open = self.fleet_view.is_some();
        if !open && self.fleet.is_empty() {
            return None;
        }
        let waiting = self.fleet_waiting();
        let said = summary(&self.fleet);
        let tooltip: SharedString = if said.is_empty() {
            "fleet".into()
        } else {
            format!("fleet · {said}").into()
        };
        let ink = self.active_ink();
        let dot = div().flex_none().size(px(6.0)).rounded_full().map(|dot| {
            if waiting > 0 {
                dot.bg(color(SIGNAL))
            } else if open {
                dot.bg(ink.color())
            } else {
                dot.border_1().border_color(color(FAINT_TEXT))
            }
        });
        let chip = h_flex()
            .id("fleet-chip")
            .flex_none()
            .h(px(26.0))
            .px(px(10.0))
            .gap(px(8.0))
            .items_center()
            .rounded(px(6.0))
            .cursor_pointer()
            .tooltip(move |_, cx| strip_tooltip(tooltip.clone(), Some("⌘⇧a"), cx))
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _, window, cx| this.toggle_fleet(window, cx)))
            .child(dot)
            .child(
                div()
                    .font_weight(if open {
                        FontWeight::BOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .text_color(color(if open { TEXT } else { FAINT_TEXT }))
                    .child("fleet"),
            )
            .when(waiting > 0, |chip| {
                chip.child(
                    div()
                        .flex_none()
                        .text_color(color(SIGNAL))
                        .child(waiting.to_string()),
                )
            });
        Some(if open {
            chip.bg(wash(0.10)).into_any_element()
        } else {
            chip.hover(|style| style.bg(wash(0.05)))
                .active(|style| style.bg(wash(0.10)))
                .into_any_element()
        })
    }

    /// An agent's mark: solid peach while it waits on the user, a peach
    /// square when it failed, its tab's ink breathing while it works and
    /// solid when it is done, a ring while it idles, and a dash once ended.
    fn fleet_dot(&self, agent: &FleetAgent) -> gpui::AnyElement {
        let ink = self
            .tab_of_pane(agent.pane_id)
            .map_or(color(MUTED_TEXT), |tab_id| self.tab_ink(tab_id).color());
        let dot = div().flex_none().size(px(6.0));
        match agent.state {
            FleetState::Blocked => dot.rounded_full().bg(color(SIGNAL)),
            FleetState::Error => dot.rounded(px(1.0)).bg(color(SIGNAL)),
            FleetState::Working => dot.rounded_full().bg(ink.opacity(self.agent_pulse_level())),
            FleetState::Done => dot.rounded_full().bg(ink),
            FleetState::Idle => dot.rounded_full().border_1().border_color(ink.opacity(0.8)),
            FleetState::Unknown => dot
                .rounded_full()
                .border_1()
                .border_color(color(FAINT_TEXT)),
            FleetState::Ended => div()
                .flex_none()
                .w(px(6.0))
                .h(px(1.0))
                .bg(color(FAINT_TEXT)),
        }
        .into_any_element()
    }

    /// The colour an agent's state is said in, the one its pane's head
    /// says it in: peach while it wants the user, its tab's ink once done.
    fn fleet_state_tone(&self, agent: &FleetAgent) -> Hsla {
        match agent.state {
            FleetState::Blocked | FleetState::Error => color(SIGNAL),
            FleetState::Working => color(MUTED_TEXT),
            FleetState::Done => self
                .tab_of_pane(agent.pane_id)
                .map_or(color(MUTED_TEXT), |tab_id| self.tab_ink(tab_id).color()),
            FleetState::Idle | FleetState::Unknown | FleetState::Ended => color(FAINT_TEXT),
        }
    }

    /// One agent in the roster, on two lines: its mark, name, what it was
    /// asked, and its state and for how long; then what it is at, and the
    /// branch it is on. One with neither a task nor a word is a single line.
    #[allow(clippy::too_many_lines)]
    fn fleet_row(
        &self,
        agent: &FleetAgent,
        chosen: bool,
        now: u64,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let pane_id = agent.pane_id;
        let ended = agent.state == FleetState::Ended;
        let headline = fleet_headline(agent);
        let branch = agent.project.branch.clone().map(|branch| {
            h_flex()
                .flex_none()
                .max_w(px(150.0))
                .gap(px(6.0))
                .text_size(px(12.0))
                .text_color(color(FAINT_TEXT))
                .child(div().min_w_0().truncate().child(branch))
                .when(agent.project.worktree, |branch| {
                    branch.child(div().flex_none().child("wt"))
                })
        });
        // An agent with nothing asked of it and nothing to say is one line,
        // its branch beside its state.
        let brief = agent.task.is_none() && headline.is_none();
        let (branch_beside, branch_below) = if brief {
            (branch, None)
        } else {
            (None, branch)
        };
        let first = h_flex()
            .h(px(20.0))
            .gap(px(8.0))
            .items_center()
            .child(
                h_flex()
                    .flex_none()
                    .w(px(6.0))
                    .h_full()
                    .items_center()
                    .child(self.fleet_dot(agent)),
            )
            .child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::BOLD)
                    .text_color(color(if ended { MUTED_TEXT } else { TEXT }))
                    .child(agent_name(agent.kind)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(color(if ended { FAINT_TEXT } else { TEXT }))
                    .children(agent.task.clone()),
            )
            .children(branch_beside)
            .child(
                div()
                    .flex_none()
                    .text_color(self.fleet_state_tone(agent))
                    .child(state_and_age(agent, now)),
            );
        let second = h_flex()
            .h(px(18.0))
            .pl(px(14.0))
            .gap(px(12.0))
            .items_center()
            .text_size(px(12.0))
            .text_color(color(FAINT_TEXT))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .when(agent.state == FleetState::Blocked, |line| {
                        line.text_color(color(MUTED_TEXT))
                    })
                    .children(headline),
            )
            .children(branch_below);
        h_flex()
            .id(SharedString::from(format!("fleet-row-{pane_id}")))
            .relative()
            .flex_none()
            .h(px(if brief { ROW_HEIGHT_BRIEF } else { ROW_HEIGHT }))
            .pl(px(24.0))
            .pr(px(10.0))
            .rounded(px(6.0))
            .cursor_pointer()
            .when(chosen, |row| row.bg(wash(0.08)))
            .when(!chosen, |row| row.hover(|row| row.bg(wash(0.04))))
            .on_click(
                cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                    this.select_fleet_agent(Some(pane_id), cx);
                    if let Some(view) = this.fleet_view.as_mut() {
                        view.card = None;
                        if view.in_pane {
                            view.in_pane = false;
                            view.focus.focus(window, cx);
                        }
                    }
                    if event.click_count() == 2 {
                        this.go_to_chosen_agent(window, cx);
                    }
                    cx.notify();
                }),
            )
            .child(
                div()
                    .absolute()
                    .left(px(8.0))
                    .top(px(if brief { 4.0 } else { 5.0 }))
                    .text_color(color(SIGNAL))
                    .when(!chosen, gpui::Styled::invisible)
                    .child("›"),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .when(ended, |row| row.opacity(0.75))
                    .child(first)
                    .when(!brief, |row| row.child(second)),
            )
            .into_any_element()
    }

    /// The roster's foot: the keys that work now, in brackets.
    fn fleet_hints(&self, short: bool) -> gpui::AnyElement {
        let view = self.fleet_view.as_ref();
        let chosen = self.fleet_selected();
        let remote = self.fleet_remote_pane().is_some();
        let hints: Vec<(&str, &str)> = match (view.and_then(|view| view.card), chosen) {
            (Some(Card::Start { .. }), _) => {
                vec![("↑↓", "choose"), ("↵", "start it"), ("esc", "back")]
            }
            (Some(Card::Hooks { .. }), _) => vec![
                ("↑↓", "choose"),
                ("↵", "hook"),
                ("u", "unhook"),
                ("esc", "back"),
            ],
            (None, _) if view.is_some_and(|view| view.in_pane) => {
                vec![("⌥←", "roster"), ("⌘⇧a", "close")]
            }
            (None, None) => vec![("n", "new"), ("h", "hooks"), ("esc", "close")],
            (None, Some(agent)) if agent.state == FleetState::Ended => vec![
                ("r", "resume"),
                ("x", "forget"),
                ("g", "its tab"),
                ("n", "new"),
                ("h", "hooks"),
                ("esc", "close"),
            ],
            (None, Some(_)) => vec![
                ("↵", if remote { "go there" } else { "type to it" }),
                ("g", "its tab"),
                ("⇥", "next waiting"),
                ("v", "grid"),
                ("n", "new"),
                ("h", "hooks"),
                ("esc", "close"),
            ],
        };
        h_flex()
            .flex_none()
            .flex_wrap()
            .gap_x(px(12.0))
            .gap_y(px(4.0))
            .px(px(18.0))
            .pt(px(8.0))
            .pb(px(12.0))
            .border_t_1()
            .border_color(hairline(0.07))
            .text_size(px(12.0))
            .text_color(color(FAINT_TEXT))
            .children(hints.into_iter().map(|(key, label)| {
                h_flex()
                    .flex_none()
                    .h(px(20.0))
                    .gap(px(6.0))
                    .items_center()
                    .child(kbd(key, color(MUTED_TEXT)))
                    .child(label)
            }))
            // A short roster keeps its room for agents: one line of keys,
            // the first of them, and the rest still work.
            .when(short, |hints| {
                hints.h(px(41.0)).gap_y(px(16.0)).overflow_hidden()
            })
            .into_any_element()
    }

    /// The start card: the agents found here, to pick one to run.
    fn fleet_start_card(&self, choice: usize, here: bool) -> gpui::AnyElement {
        let place = self.fleet_selected().filter(|_| !here).map_or_else(
            || "where you were".to_owned(),
            |agent| format!("in {}", agent.project.name),
        );
        let mut card = v_flex().gap(px(2.0)).child(
            h_flex()
                .h(px(28.0))
                .pl(px(24.0))
                .pr(px(10.0))
                .justify_between()
                .child(div().font_weight(FontWeight::BOLD).child("start an agent"))
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(color(FAINT_TEXT))
                        .child(format!("in a new tab, {place}")),
                ),
        );
        match self.fleet_launchers.as_deref() {
            None => {
                card = card.child(Self::fleet_card_note("looking for agents…"));
            }
            Some([]) => {
                card = card.child(Self::fleet_card_note(
                    "none of claude, codex, copilot or gemini is installed",
                ));
            }
            Some(found) => {
                for (index, kind) in found.iter().enumerate() {
                    card = card.child(Self::fleet_card_line(
                        index == choice.min(found.len() - 1),
                        agent_name(*kind),
                        String::new(),
                        FAINT_TEXT,
                    ));
                }
            }
        }
        card.into_any_element()
    }

    /// The hooks card: each agent Mux has hooks for, and whether they are in.
    fn fleet_hooks_card(&self, choice: usize) -> gpui::AnyElement {
        let mut card = v_flex().gap(px(2.0)).child(
            h_flex()
                .h(px(28.0))
                .pl(px(24.0))
                .pr(px(10.0))
                .justify_between()
                .child(div().font_weight(FontWeight::BOLD).child("hooks"))
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(color(FAINT_TEXT))
                        .child("agents tell mux what they are doing"),
                ),
        );
        if self.fleet_hooks.is_empty() {
            card = card.child(Self::fleet_card_note("looking…"));
        }
        for (index, (kind, status)) in self.fleet_hooks.iter().enumerate() {
            let tone = match status {
                HookStatus::Installed => SAGE,
                HookStatus::Outdated => SIGNAL,
                HookStatus::Missing => MUTED_TEXT,
                HookStatus::Absent => FAINT_TEXT,
            };
            card = card.child(Self::fleet_card_line(
                index == choice.min(self.fleet_hooks.len() - 1),
                agent_name(*kind),
                status.word().to_owned(),
                tone,
            ));
        }
        card.into_any_element()
    }

    fn fleet_card_note(said: &'static str) -> gpui::AnyElement {
        h_flex()
            .h(px(28.0))
            .pl(px(24.0))
            .text_color(color(FAINT_TEXT))
            .child(said)
            .into_any_element()
    }

    fn fleet_card_line(chosen: bool, name: String, said: String, tone: u32) -> gpui::AnyElement {
        h_flex()
            .relative()
            .h(px(28.0))
            .pl(px(24.0))
            .pr(px(10.0))
            .gap(px(12.0))
            .rounded(px(6.0))
            .when(chosen, |line| line.bg(wash(0.08)))
            .child(
                div()
                    .absolute()
                    .left(px(8.0))
                    .text_color(color(SIGNAL))
                    .when(!chosen, gpui::Styled::invisible)
                    .child("›"),
            )
            .child(
                div()
                    .when(chosen, |name| name.font_weight(FontWeight::BOLD))
                    .child(name),
            )
            .child(div().flex_1())
            .child(div().flex_none().text_color(color(tone)).child(said))
            .into_any_element()
    }

    /// The roster slab: a head that sums the fleet up, the agents under
    /// their projects, a word about hooks while any are missing, a card
    /// when one is open, and the keys.
    #[allow(clippy::too_many_lines)]
    fn render_fleet_roster(
        &self,
        view: &FleetView,
        frame: layout::Rect,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let now = now_ms();
        let chosen = self.fleet_selected().map(|agent| agent.pane_id);
        let groups = roster(&self.fleet);
        let focused = !view.in_pane;
        let ink = self.active_ink();
        // The head's room after its title, in characters of its small type.
        let room = ((frame.width - 30.0 - 16.0 - 12.0 - 52.0) / 7.3).floor();
        let said = summary_within(&self.fleet, room.max(0.0) as usize);

        let mut list = v_flex()
            .id("fleet-roster-rows")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&view.scroll)
            .px(px(6.0))
            .pb(px(6.0));
        if groups.is_empty() {
            list = list.child(
                v_flex()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .gap(px(6.0))
                    .text_color(color(FAINT_TEXT))
                    .child(div().text_color(color(MUTED_TEXT)).child("no agents yet"))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .child("run claude, codex or copilot in any pane"),
                    )
                    .child(
                        h_flex()
                            .mt(px(6.0))
                            .gap(px(6.0))
                            .text_size(px(12.0))
                            .child(kbd("n", color(MUTED_TEXT)))
                            .child("start one here"),
                    ),
            );
        }
        for (index, group) in groups.iter().enumerate() {
            list = list.child(
                h_flex()
                    .flex_none()
                    .h(px(24.0))
                    .when(index > 0, |head| head.mt(px(8.0)))
                    .pl(px(24.0))
                    .pr(px(10.0))
                    .gap(px(8.0))
                    .items_center()
                    .text_size(px(12.0))
                    .text_color(color(MUTED_TEXT))
                    .child(div().min_w_0().truncate().child(group.name.to_owned()))
                    .child(
                        div()
                            .flex_none()
                            .text_color(color(FAINT_TEXT))
                            .child(group.agents.len().to_string()),
                    )
                    .child(div().flex_1().h(px(1.0)).bg(hairline(0.05))),
            );
            for agent in &group.agents {
                list = list.child(self.fleet_row(agent, chosen == Some(agent.pane_id), now, cx));
            }
        }

        let unhooked = self.unhooked_agents();
        let short = frame.height < SHORT_ROSTER && view.card.is_none();
        let hooks_line = (!unhooked.is_empty() && view.card.is_none() && !short).then(|| {
            let names = unhooked
                .iter()
                .map(|kind| kind.command().to_owned())
                .collect::<Vec<_>>();
            h_flex()
                .id("fleet-hooks-line")
                .flex_none()
                .h(px(28.0))
                .px(px(18.0))
                // A band of its own under a full roster, not one more row.
                .border_t_1()
                .border_color(hairline(0.07))
                .gap(px(12.0))
                .items_center()
                .text_size(px(12.0))
                .text_color(color(FAINT_TEXT))
                .cursor_pointer()
                .hover(|line| line.text_color(color(MUTED_TEXT)))
                .on_click(cx.listener(|this, _, window, cx| this.open_fleet_hooks(window, cx)))
                .child(div().min_w_0().truncate().child(if names.len() > 1 {
                    format!("{} agents can tell mux more", names.len())
                } else {
                    format!("{} can tell mux more", chrome::names_list(&names))
                }))
                .child(div().flex_1())
                .child(
                    h_flex()
                        .flex_none()
                        .gap(px(6.0))
                        .child(kbd("h", color(MUTED_TEXT)))
                        .child(if unhooked.len() == 1 {
                            "hook it"
                        } else {
                            "hook them"
                        }),
                )
        });
        let card = view.card.map(|card| {
            div()
                .flex_none()
                .px(px(6.0))
                .pt(px(8.0))
                .pb(px(6.0))
                .border_t_1()
                .border_color(hairline(0.07))
                .child(match card {
                    Card::Start { choice, here } => self.fleet_start_card(choice, here),
                    Card::Hooks { choice } => self.fleet_hooks_card(choice),
                })
        });

        let slab = v_flex()
            .id("fleet-roster")
            .key_context("MuxFleet")
            .track_focus(&view.focus)
            .on_key_down(cx.listener(Self::fleet_key))
            .on_action(cx.listener(|this, _: &ForwardTerminalTab, _, cx| {
                this.choose_next_waiting(true, cx);
            }))
            .on_action(cx.listener(|this, _: &ForwardTerminalBacktab, _, cx| {
                this.choose_next_waiting(false, cx);
            }))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    if let Some(view) = this.fleet_view.as_mut()
                        && view.in_pane
                    {
                        view.in_pane = false;
                        view.focus.focus(window, cx);
                        cx.notify();
                    }
                }),
            )
            .absolute()
            .left(px(frame.x))
            .top(px(frame.y))
            .w(px(frame.width))
            .h(px(frame.height))
            .rounded(px(12.0))
            .bg(color(SURFACE))
            .border_1()
            .border_color(if focused {
                ink.color().opacity(0.55)
            } else {
                hairline(0.07)
            })
            .overflow_hidden()
            .font_family(EMBEDDED_TERMINAL_FONT)
            .text_size(px(13.0))
            .text_color(color(TEXT))
            .whitespace_nowrap()
            .child(
                h_flex()
                    .flex_none()
                    .h(px(40.0))
                    .pl(px(30.0))
                    .pr(px(16.0))
                    .gap(px(12.0))
                    .items_center()
                    .child(div().font_weight(FontWeight::BOLD).child("fleet"))
                    .child(div().flex_1())
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.0))
                            .text_color(color(FAINT_TEXT))
                            .child(said),
                    ),
            )
            .child(list)
            .children(hooks_line)
            .children(card)
            .child(self.fleet_hints(short))
            .when(focused, |slab| {
                slab.child(focus_notch("fleet-roster-focus".into(), ink, self.motion))
            });
        slab.into_any_element()
    }

    /// A quiet slab in the view's right side, for what has no live pane.
    fn fleet_slab(frame: layout::Rect) -> gpui::Div {
        v_flex()
            .absolute()
            .left(px(frame.x))
            .top(px(frame.y))
            .w(px(frame.width))
            .h(px(frame.height))
            .rounded(px(12.0))
            .bg(color(SURFACE))
            .border_1()
            .border_color(hairline(0.07))
            .overflow_hidden()
            .font_family(EMBEDDED_TERMINAL_FONT)
            .text_size(px(13.0))
            .text_color(color(TEXT))
    }

    /// What is remembered of an ended agent, over its pane: what it was
    /// asked, the last thing it said, and the keys that pick it up or let
    /// it go.
    fn render_fleet_ended(agent: &FleetAgent, frame: layout::Rect) -> gpui::AnyElement {
        let resumes = agent.resume_command();
        Self::fleet_slab(frame)
            .px(px(18.0))
            .py(px(11.0))
            .gap(px(4.0))
            .whitespace_nowrap()
            .child(
                h_flex()
                    .flex_none()
                    .h(px(20.0))
                    .gap(px(10.0))
                    .child(
                        div()
                            .flex_none()
                            .font_weight(FontWeight::BOLD)
                            .child(agent_name(agent.kind)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .children(agent.task.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_color(color(FAINT_TEXT))
                            .child(state_and_age(agent, now_ms())),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .h(px(18.0))
                    .min_w_0()
                    .truncate()
                    .text_size(px(12.0))
                    .text_color(color(MUTED_TEXT))
                    .child(
                        fleet_said(agent, agent.message.as_deref())
                            .unwrap_or_else(|| "it said nothing at the end".to_owned()),
                    ),
            )
            .child(div().flex_1())
            .child(
                h_flex()
                    .flex_none()
                    .h(px(20.0))
                    .gap(px(12.0))
                    .text_size(px(12.0))
                    .text_color(color(FAINT_TEXT))
                    .child(
                        h_flex()
                            .flex_none()
                            .gap(px(6.0))
                            .child(kbd("r", color(MUTED_TEXT)))
                            .child("resume"),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .gap(px(6.0))
                            .child(kbd("x", color(MUTED_TEXT)))
                            .child("forget"),
                    )
                    .child(div().flex_1())
                    .children(resumes.map(|command| div().min_w_0().truncate().child(command))),
            )
            .into_any_element()
    }

    /// The pane of an agent in another session: no terminal here, so the
    /// last of what it shows as text, under a line that says where it is.
    fn render_fleet_remote(
        &self,
        view: &FleetView,
        agent: &FleetAgent,
        frame: layout::Rect,
    ) -> gpui::AnyElement {
        let session = self
            .sessions
            .iter()
            .find(|session| session.id == agent.session_id)
            .map_or_else(
                || "another session".to_owned(),
                |session| format!("session {}", session.name),
            );
        let text = view.remote_text.get(&agent.pane_id).map(String::as_str);
        let metrics = self.metrics;
        let room = ((frame.height - 36.0 - 12.0) / metrics.cell_height)
            .floor()
            .max(1.0) as usize;
        let lines = text.map(|text| {
            let lines = text.trim_end().lines().collect::<Vec<_>>();
            lines[lines.len().saturating_sub(room)..]
                .iter()
                .map(|line| (*line).to_owned())
                .collect::<Vec<_>>()
        });
        Self::fleet_slab(frame)
            .child(
                h_flex()
                    .flex_none()
                    .h(px(36.0))
                    .px(px(18.0))
                    .gap(px(12.0))
                    .items_center()
                    .border_b_1()
                    .border_color(hairline(0.07))
                    .text_size(px(12.0))
                    .text_color(color(MUTED_TEXT))
                    .whitespace_nowrap()
                    .child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .text_color(color(TEXT))
                            .child(agent_name(agent.kind)),
                    )
                    .child(format!("in {session}"))
                    .child(div().flex_1())
                    .child(
                        h_flex()
                            .flex_none()
                            .gap(px(6.0))
                            .text_color(color(FAINT_TEXT))
                            .child(kbd("↵", color(MUTED_TEXT)))
                            .child("go there"),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .justify_end()
                    .overflow_hidden()
                    .px(px(8.0))
                    .pb(px(6.0))
                    .font_family(self.terminal_font.clone())
                    .text_size(px(metrics.font_size))
                    .line_height(px(metrics.cell_height))
                    .text_color(color(MUTED_TEXT))
                    .whitespace_nowrap()
                    .map(|body| match lines {
                        Some(lines) => body.children(lines.into_iter().map(|line| {
                            div()
                                .flex_none()
                                .h(px(metrics.cell_height))
                                .child(if line.is_empty() {
                                    " ".to_owned()
                                } else {
                                    line
                                })
                        })),
                        None => body.child(
                            div()
                                .text_color(color(FAINT_TEXT))
                                .child("reading its pane…"),
                        ),
                    }),
            )
            .into_any_element()
    }

    /// One tile of the wall: a head of two lines, the agent's mark, name,
    /// project and state, then what it was asked (or what it asks, while it
    /// waits), over its pane drawn small. The chosen tile wears the ink a
    /// focused pane does, and one that waits on the user a peach edge.
    #[allow(clippy::too_many_lines)]
    fn render_wall_tile(
        &self,
        tile: &WallTile,
        agent: &FleetAgent,
        pane: Option<TilePane>,
        chosen: bool,
        now: u64,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let pane_id = tile.pane_id;
        let frame = tile.frame;
        let body = tile.body();
        let ink = self.active_ink();
        let wants = agent.state.wants_user();
        let ended = agent.state == FleetState::Ended;
        // What a waiting agent asks takes the place of what it was asked.
        let beside = if wants {
            fleet_headline(agent).or(agent.task.clone())
        } else {
            agent.task.clone()
        };
        // A pane of another session says so beside its project, where the
        // head has room for both.
        let place = if self.panes.contains_key(&pane_id) || frame.width < 320.0 {
            agent.project.name.clone()
        } else {
            let session = self
                .sessions
                .iter()
                .find(|session| session.id == agent.session_id)
                .map_or_else(
                    || "another session".to_owned(),
                    |session| session.name.clone(),
                );
            format!("{} · in {session}", agent.project.name)
        };
        let head = v_flex()
            .flex_none()
            .h(px(TILE_HEAD - 1.0))
            .px(px(10.0))
            .pt(px(4.0))
            .text_size(px(12.0))
            .child(
                h_flex()
                    .flex_none()
                    .h(px(20.0))
                    .gap(px(8.0))
                    .items_center()
                    .child(self.fleet_dot(agent))
                    .child(
                        div()
                            .flex_none()
                            .font_weight(FontWeight::BOLD)
                            .text_color(color(if chosen { TEXT } else { MUTED_TEXT }))
                            .child(agent_name(agent.kind)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(color(FAINT_TEXT))
                            .child(place),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_color(self.fleet_state_tone(agent))
                            .child(state_and_age(agent, now)),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .h(px(18.0))
                    .pl(px(14.0))
                    .truncate()
                    .text_size(px(12.0))
                    .text_color(color(if wants { TEXT } else { MUTED_TEXT }))
                    .children(beside),
            );
        let shown = match pane {
            Some(pane) => {
                // A pane sits in its tile's top left, as its text does in
                // it; one that holds more than the tile is tall shows the
                // foot of it, where its latest output and its cursor are.
                let top = (body.height - pane.foot).min(0.0);
                div()
                    .absolute()
                    .left_0()
                    .top(px(top))
                    .w(px(pane.width))
                    .h(px(pane.height))
                    .child(pane.view)
                    .into_any_element()
            }
            // A pane of another session has no terminal here: the foot of
            // its text, at the size a tile's pane reads at.
            None => {
                let text = self
                    .fleet_view
                    .as_ref()
                    .and_then(|view| view.remote_text.get(&pane_id));
                let line_height = (TILE_MIN_FONT * 1.4).round();
                let room = (body.height / line_height).floor().max(1.0) as usize;
                let lines = text.map(|text| {
                    let lines = text.trim_end().lines().collect::<Vec<_>>();
                    lines[lines.len().saturating_sub(room)..]
                        .iter()
                        .map(|line| (*line).to_owned())
                        .collect::<Vec<_>>()
                });
                v_flex()
                    .size_full()
                    .justify_end()
                    .overflow_hidden()
                    .px(px(4.0))
                    .font_family(self.terminal_font.clone())
                    .text_size(px(TILE_MIN_FONT))
                    .line_height(px(line_height))
                    .text_color(color(MUTED_TEXT))
                    .children(lines.into_iter().flatten().map(|line| {
                        div()
                            .flex_none()
                            .h(px(line_height))
                            .child(if line.is_empty() {
                                " ".to_owned()
                            } else {
                                line
                            })
                    }))
                    .into_any_element()
            }
        };
        v_flex()
            .id(SharedString::from(format!("fleet-tile-{pane_id}")))
            .absolute()
            .left(px(frame.x))
            .top(px(frame.y))
            .w(px(frame.width))
            .h(px(frame.height))
            .rounded(px(10.0))
            .bg(color(SURFACE))
            .border_1()
            .border_color(if chosen {
                ink.color().opacity(0.55)
            } else if wants {
                color(SIGNAL).opacity(0.5)
            } else {
                hairline(0.07)
            })
            .overflow_hidden()
            .cursor_pointer()
            .font_family(EMBEDDED_TERMINAL_FONT)
            .whitespace_nowrap()
            .on_click(
                cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                    this.select_fleet_agent(Some(pane_id), cx);
                    this.focus_fleet_roster(window, cx);
                    if event.click_count() == 2 {
                        this.enter_wall_tile(window, cx);
                    }
                    cx.notify();
                }),
            )
            .child(head)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .mx(px(TILE_PAD))
                    .mb(px(TILE_PAD))
                    .overflow_hidden()
                    .when(ended, |body| body.opacity(0.55))
                    .child(shown),
            )
            .when(chosen, |tile| {
                tile.child(focus_notch(
                    SharedString::from(format!("fleet-tile-focus-{pane_id}")),
                    ink,
                    self.motion,
                ))
            })
            .into_any_element()
    }

    /// The wall: a tile for each agent on the page, and a foot that sums
    /// the fleet up and says the keys. It takes the keys the roster does.
    fn render_fleet_wall(
        &self,
        view: &FleetView,
        wall: &WallLayout,
        mut panes: HashMap<PaneId, TilePane>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let now = now_ms();
        let chosen = self.fleet_selected().map(|agent| agent.pane_id);
        // Said from the right, so the row reads choose … roster; in a
        // narrow window the ones at its left give way first, whole.
        let hints = [
            ("v", "roster"),
            ("g", "its tab"),
            ("↵", "type to it"),
            ("⇥", "next waiting"),
            ("←↑↓→", "choose"),
        ];
        // The summary has the foot's near half at most, in whole parts.
        let room = ((wall.foot.width * 0.45 - 70.0) / 7.3).floor().max(0.0) as usize;
        let foot = h_flex()
            .absolute()
            .left(px(wall.foot.x))
            .top(px(wall.foot.y))
            .w(px(wall.foot.width))
            .h(px(wall.foot.height))
            .px(px(18.0))
            .gap(px(12.0))
            .items_center()
            .rounded(px(10.0))
            .bg(color(SURFACE))
            .border_1()
            .border_color(hairline(0.07))
            .overflow_hidden()
            .text_size(px(12.0))
            .text_color(color(FAINT_TEXT))
            .child(
                div()
                    .flex_none()
                    .text_size(px(13.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(color(TEXT))
                    .child("fleet"),
            )
            .child(div().flex_none().child(summary_within(&self.fleet, room)))
            .when(wall.pages > 1, |foot| {
                foot.child(
                    div()
                        .flex_none()
                        .text_color(color(MUTED_TEXT))
                        .child(format!("page {} of {}", wall.page + 1, wall.pages)),
                )
            })
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .h(px(20.0))
                    .flex_wrap()
                    .flex_row_reverse()
                    .overflow_hidden()
                    .gap_x(px(12.0))
                    .children(hints.into_iter().map(|(key, label)| {
                        h_flex()
                            .flex_none()
                            .h(px(20.0))
                            .gap(px(6.0))
                            .items_center()
                            .child(kbd(key, color(MUTED_TEXT)))
                            .child(label)
                    })),
            );
        div()
            .id("fleet-wall")
            .key_context("MuxFleet")
            .track_focus(&view.focus)
            .on_key_down(cx.listener(Self::fleet_key))
            .on_action(cx.listener(|this, _: &ForwardTerminalTab, _, cx| {
                this.choose_next_waiting(true, cx);
            }))
            .on_action(cx.listener(|this, _: &ForwardTerminalBacktab, _, cx| {
                this.choose_next_waiting(false, cx);
            }))
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .font_family(EMBEDDED_TERMINAL_FONT)
            .text_size(px(13.0))
            .text_color(color(TEXT))
            .whitespace_nowrap()
            .children(wall.tiles.iter().filter_map(|tile| {
                let agent = self
                    .fleet
                    .iter()
                    .find(|agent| agent.pane_id == tile.pane_id)?;
                Some(self.render_wall_tile(
                    tile,
                    agent,
                    panes.remove(&tile.pane_id),
                    chosen == Some(tile.pane_id),
                    now,
                    cx,
                ))
            }))
            .child(foot)
            .into_any_element()
    }

    /// Everything of the view but the chosen agent's live pane, which the
    /// window draws as it draws any pane.
    pub(super) fn render_fleet(
        &self,
        layout: &FleetLayout,
        tiles: HashMap<PaneId, TilePane>,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let Some(view) = self.fleet_view.as_ref() else {
            return Vec::new();
        };
        if let Some(wall) = &layout.wall {
            return vec![self.render_fleet_wall(view, wall, tiles, cx)];
        }
        let mut parts = vec![self.render_fleet_roster(view, layout.roster, cx)];
        let chosen = self.fleet_selected();
        if let (Some(card), Some(agent)) = (layout.card, chosen) {
            parts.push(Self::render_fleet_ended(agent, card));
        }
        if layout.pane.is_none()
            && let Some(agent) = chosen
        {
            let frame = layout.card.map_or(layout.detail, |card| layout::Rect {
                y: layout.detail.y + card.height + layout::PANE_GAP,
                height: (layout.detail.height - card.height - layout::PANE_GAP).max(1.0),
                ..layout.detail
            });
            parts.push(self.render_fleet_remote(view, agent, frame));
        }
        parts
    }

    /// A key while the roster has the keyboard.
    pub(super) fn fleet_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keystroke = &event.keystroke;
        let key = keystroke.key.as_str();
        let Some(view) = self.fleet_view.as_ref() else {
            return;
        };
        if view.in_pane {
            return;
        }
        let modifiers = keystroke.modifiers;
        let bare = !modifiers.modified();
        if let Some(card) = view.card {
            if bare || (modifiers.shift && !modifiers.control && !modifiers.platform) {
                self.fleet_card_key(card, key, cx);
                cx.stop_propagation();
                cx.notify();
            }
            return;
        }
        if view.wall {
            if !bare {
                return;
            }
            match key {
                "escape" | "v" => self.set_fleet_wall(false, cx),
                "left" | "right" | "up" | "down" | "h" | "j" | "k" | "l" => {
                    self.move_wall_choice(key, cx);
                }
                "enter" => self.enter_wall_tile(window, cx),
                "g" => self.go_to_chosen_agent(window, cx),
                _ => return,
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        // The roster is the pane on the left: the keys that go right from
        // a pane step into the agent's.
        let goes_right = key_chord(keystroke).is_some_and(|chord| {
            matches!(
                self.keymap.resolve(InputMode::Normal, chord),
                Some(
                    Action::FocusPaneOrTab(Direction::Right) | Action::FocusPane(Direction::Right)
                )
            )
        });
        if goes_right {
            self.enter_fleet_pane(window, cx);
            cx.stop_propagation();
            return;
        }
        if !bare {
            return;
        }
        match key {
            "escape" => self.close_fleet(true, window, cx),
            "up" | "k" => self.move_fleet_choice(-1, cx),
            "down" | "j" => self.move_fleet_choice(1, cx),
            "enter" => self.enter_fleet_pane(window, cx),
            "g" => self.go_to_chosen_agent(window, cx),
            "r" => self.resume_chosen_agent(cx),
            "x" => self.forget_chosen_agent(cx),
            "n" => {
                let here = self.fleet_selected().is_none();
                self.open_fleet_start(here, window, cx);
            }
            "h" => self.open_fleet_hooks(window, cx),
            "v" => self.set_fleet_wall(true, cx),
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// A key while a card is open over the roster's foot.
    fn fleet_card_key(&mut self, card: Card, key: &str, cx: &mut Context<Self>) {
        let rows = match card {
            Card::Start { .. } => self.fleet_launchers.as_ref().map_or(0, Vec::len),
            Card::Hooks { .. } => self.fleet_hooks.len(),
        };
        let step = |choice: usize, delta: isize| {
            if rows == 0 {
                0
            } else {
                wrapping_step(choice.min(rows - 1), delta, rows)
            }
        };
        let next = match (card, key) {
            (_, "escape") => None,
            (Card::Start { choice, here }, "up" | "k") => Some(Card::Start {
                choice: step(choice, -1),
                here,
            }),
            (Card::Start { choice, here }, "down" | "j" | "tab") => Some(Card::Start {
                choice: step(choice, 1),
                here,
            }),
            (Card::Start { choice, here }, "enter") => {
                let kind = self
                    .fleet_launchers
                    .as_ref()
                    .and_then(|found| found.get(choice.min(rows.saturating_sub(1))))
                    .copied();
                if let Some(kind) = kind {
                    self.start_fleet_agent(kind, here, cx);
                }
                return;
            }
            (Card::Hooks { choice }, "up" | "k") => Some(Card::Hooks {
                choice: step(choice, -1),
            }),
            (Card::Hooks { choice }, "down" | "j" | "tab") => Some(Card::Hooks {
                choice: step(choice, 1),
            }),
            (Card::Hooks { choice }, "enter" | "u") => {
                let chosen = self
                    .fleet_hooks
                    .get(choice.min(rows.saturating_sub(1)))
                    .copied();
                if let Some((kind, status)) = chosen {
                    if status == HookStatus::Absent {
                        self.say_in_strip(
                            format!("{} isn't set up on this Mac", agent_name(kind)),
                            false,
                            cx,
                        );
                    } else {
                        Self::set_fleet_hooks(kind, key == "enter", cx);
                    }
                }
                return;
            }
            _ => return,
        };
        if let Some(view) = self.fleet_view.as_mut() {
            view.card = next;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mux_fleet::{Project, Signal};

    fn agent(project: &str, state: FleetState, since_ms: u64) -> FleetAgent {
        FleetAgent {
            pane_id: PaneId::new(),
            session_id: SessionId::new(),
            kind: AgentKind::Claude,
            state,
            need: None,
            task: None,
            activity: None,
            message: None,
            agent_session: None,
            transcript: None,
            cwd: PathBuf::from(format!("/src/{project}")),
            project: Project {
                name: project.to_owned(),
                root: PathBuf::from(format!("/src/{project}")),
                branch: None,
                worktree: false,
            },
            since_ms,
            started_ms: 0,
            turns: 0,
            signal: Signal::Hooks,
        }
    }

    #[test]
    fn the_roster_groups_by_project_with_what_presses_first() {
        let agents = vec![
            agent("mux", FleetState::Working, 5),
            agent("gofer", FleetState::Idle, 1),
            agent("mux", FleetState::Blocked, 9),
            agent("atlas", FleetState::Idle, 2),
            agent("gofer", FleetState::Done, 3),
        ];
        let groups = roster(&agents);
        let read = groups
            .iter()
            .map(|group| {
                (
                    group.name,
                    group
                        .agents
                        .iter()
                        .map(|agent| agent.state)
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            read,
            vec![
                ("mux", vec![FleetState::Blocked, FleetState::Working]),
                ("gofer", vec![FleetState::Done, FleetState::Idle]),
                ("atlas", vec![FleetState::Idle]),
            ]
        );
        assert_eq!(roster_order(&agents)[0], agents[2].pane_id);
        // A row is scrolled to by its place among the rows and the heads
        // over them: mux's head, its two rows, gofer's head, then gofer's.
        assert_eq!(roster_list_index(&agents, agents[2].pane_id), Some(1));
        assert_eq!(roster_list_index(&agents, agents[4].pane_id), Some(4));
        assert_eq!(roster_list_index(&agents, agents[3].pane_id), Some(7));
        assert_eq!(roster_list_index(&agents, PaneId::new()), None);
    }

    #[test]
    fn paths_are_said_from_where_the_agent_works() {
        let mut agent = agent("gofer", FleetState::Blocked, 1);
        agent.cwd = PathBuf::from("/Users/r/src/gofer/crates/core");
        agent.project.root = PathBuf::from("/Users/r/src/gofer");
        let home = PathBuf::from("/Users/r");
        let said = |line: &str| said_from_project(&agent, line, Some(&home));
        assert_eq!(
            said("Write: /Users/r/src/gofer/crates/core/src/retry.rs"),
            "Write: src/retry.rs"
        );
        assert_eq!(
            said("Edit: /Users/r/src/gofer/README.md"),
            "Edit: README.md"
        );
        assert_eq!(said("Read: /Users/r/.zshrc"), "Read: ~/.zshrc");
        assert_eq!(said("Bash: cargo test"), "Bash: cargo test");
    }

    #[test]
    fn two_checkouts_of_one_name_are_two_projects() {
        let mut other = agent("mux", FleetState::Idle, 1);
        other.project.root = PathBuf::from("/tmp/mux");
        let agents = vec![agent("mux", FleetState::Idle, 2), other];
        assert_eq!(roster(&agents).len(), 2);
    }

    #[test]
    fn a_row_says_its_state_and_for_how_long() {
        let now = 200_000;
        assert_eq!(
            state_and_age(&agent("mux", FleetState::Blocked, 80_000), now),
            "waiting 2m"
        );
        assert_eq!(
            state_and_age(&agent("mux", FleetState::Working, 159_000), now),
            "working 41s"
        );
        assert_eq!(
            state_and_age(&agent("mux", FleetState::Idle, 1), now),
            "idle"
        );
        assert_eq!(
            state_and_age(&agent("mux", FleetState::Unknown, 190_000), now),
            "running 10s"
        );
        // A clock that reads behind the daemon's never counts backwards.
        assert_eq!(
            state_and_age(&agent("mux", FleetState::Done, 300_000), now),
            "done 0s"
        );
    }

    #[test]
    fn the_fleet_sums_up_in_a_line() {
        let agents = vec![
            agent("mux", FleetState::Blocked, 1),
            agent("mux", FleetState::Blocked, 2),
            agent("mux", FleetState::Working, 3),
            agent("mux", FleetState::Done, 4),
            agent("mux", FleetState::Unknown, 5),
        ];
        assert_eq!(summary(&agents), "2 waiting · 1 working · 1 done · 1 idle");
        assert_eq!(summary(&[]), "");
        assert_eq!(summary_within(&agents, 40), summary(&agents));
        assert_eq!(
            summary_within(&agents, 30),
            "2 waiting · 1 working · 1 done"
        );
        assert_eq!(summary_within(&agents, 12), "2 waiting");
        assert_eq!(summary_within(&agents, 3), summary(&agents));
    }

    #[test]
    fn tab_goes_round_the_agents_that_wait() {
        let order = [PaneId::new(), PaneId::new(), PaneId::new(), PaneId::new()];
        let waits = |pane_id| pane_id == order[1] || pane_id == order[3];
        assert_eq!(
            next_waiting(&order, waits, Some(order[0]), true),
            Some(order[1])
        );
        assert_eq!(
            next_waiting(&order, waits, Some(order[1]), true),
            Some(order[3])
        );
        assert_eq!(
            next_waiting(&order, waits, Some(order[3]), true),
            Some(order[1])
        );
        assert_eq!(
            next_waiting(&order, waits, Some(order[1]), false),
            Some(order[3])
        );
        assert_eq!(next_waiting(&order, waits, None, true), Some(order[1]));
        // The only one waiting is where Tab stays.
        let one = |pane_id| pane_id == order[2];
        assert_eq!(
            next_waiting(&order, one, Some(order[2]), true),
            Some(order[2])
        );
        assert_eq!(next_waiting(&order, |_| false, Some(order[0]), true), None);
        assert_eq!(next_waiting(&[], |_| true, None, true), None);
    }

    #[test]
    fn the_wall_tiles_what_waits_first() {
        let agents = vec![
            agent("mux", FleetState::Idle, 1),
            agent("gofer", FleetState::Working, 2),
            agent("atlas", FleetState::Blocked, 9),
            agent("mux", FleetState::Ended, 3),
            agent("gofer", FleetState::Blocked, 4),
        ];
        let order = wall_order(&agents);
        assert_eq!(
            order,
            vec![
                agents[4].pane_id,
                agents[2].pane_id,
                agents[1].pane_id,
                agents[0].pane_id,
                agents[3].pane_id,
            ]
        );
    }

    #[test]
    fn the_wall_shares_its_room_by_how_many_agents_there_are() {
        // A window of 1440 by 800 or so.
        let (width, height) = (1400.0, 760.0);
        assert_eq!(wall_grid(1, width, height), (1, 1));
        // Two sit side by side in a wide window, and one over the other in
        // a tall one.
        assert_eq!(wall_grid(2, width, height), (2, 1));
        assert_eq!(wall_grid(2, 700.0, 1000.0), (1, 2));
        assert_eq!(wall_grid(6, width, height), (3, 2));
        assert_eq!(wall_grid(12, width, height), (4, 3));
        // More than fit at the least size go a page at a time.
        let (columns, rows) = wall_grid(40, width, height);
        assert_eq!((columns, rows), (4, 4));
        // A window too small for two tiles still shows one.
        assert_eq!(wall_grid(5, 200.0, 100.0), (1, 1));
    }

    #[test]
    fn the_arrows_walk_the_wall_and_stop_at_its_edges() {
        // Seven tiles, three to a row: the last row has one.
        assert_eq!(wall_step(0, 7, 3, "right"), 1);
        assert_eq!(wall_step(2, 7, 3, "right"), 3);
        assert_eq!(wall_step(6, 7, 3, "right"), 6);
        assert_eq!(wall_step(0, 7, 3, "left"), 0);
        assert_eq!(wall_step(1, 7, 3, "down"), 4);
        assert_eq!(wall_step(4, 7, 3, "down"), 6);
        assert_eq!(wall_step(6, 7, 3, "down"), 6);
        assert_eq!(wall_step(6, 7, 3, "up"), 3);
        assert_eq!(wall_step(1, 7, 3, "k"), 1);
    }

    #[test]
    fn an_age_changes_by_the_second_then_by_the_minute() {
        assert_eq!(until_age_changes(0, 1_400), Duration::from_millis(600));
        assert_eq!(until_age_changes(0, 59_000), Duration::from_secs(1));
        assert_eq!(until_age_changes(0, 61_000), Duration::from_secs(59));
        assert_eq!(until_age_changes(5_000, 1_000), Duration::from_secs(1));
    }

    #[test]
    fn a_project_path_is_typed_whole() {
        assert_eq!(
            shell_quoted(std::path::Path::new("/src/my app")),
            "'/src/my app'"
        );
        assert_eq!(
            shell_quoted(std::path::Path::new("/src/it's")),
            "'/src/it'\\''s'"
        );
    }
}
