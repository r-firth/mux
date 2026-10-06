//! Go to.
//!
//! One line over the dimmed workspace, as in gofer. With nothing typed it is
//! a map of the session: every tab with its number and ink, the panes of a
//! split tab under it with what each is running, then the other sessions,
//! then what mux can do, each command beside the keys that do it, so the
//! palette teaches its own shortcuts. Typing narrows all of it at once by
//! the letters of a name in order (`sr` finds split right), and the letters
//! that matched turn peach. Enter goes there. It opens on the tab you were in
//! before this one, so ⌘P then enter goes back and forth between two tabs.
//! Each agent mux can start is there too, but only once something is typed
//! (`codex`), so agents stay out of the way of the map.

use super::*;
use gpui::{HighlightStyle, StyledText};
use std::{cmp::Reverse, ops::Range};

const PALETTE_WIDTH: f32 = 576.0;
const QUERY_HEIGHT: f32 = 48.0;
const ROW_HEIGHT: f32 = 28.0;
const LIST_PAD: f32 = 8.0;
const ROW_PAD: f32 = 8.0;
/// gofer's gutter: a tab's number, a command's keys, or what a row is.
const GUTTER: f32 = 56.0;
const GUTTER_GAP: f32 = 12.0;
const MARK: f32 = 6.0;
const MARK_GAP: f32 = 10.0;
/// Where the tabs' dots and the panes' rings sit; the prompt sits over them.
const MARK_X: f32 = LIST_PAD + ROW_PAD + GUTTER + GUTTER_GAP;
/// Where names start; what is typed lines up with them.
const LABEL_X: f32 = MARK_X + MARK + MARK_GAP;

pub(super) struct Palette {
    input: Entity<InputState>,
    query: String,
    /// The row the keys act on, by where it leads, so the list can change
    /// under it (a title updating, a tab closing) without losing it. `None`
    /// is the row the palette opens on.
    selected: Option<Destination>,
    scroll: ScrollHandle,
    /// Where the pointer last moved, so a list scrolled under a resting
    /// pointer doesn't take the choice away from the keys.
    pointer: Option<gpui::Point<gpui::Pixels>>,
    _subscription: gpui::Subscription,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Destination {
    Tab(TabId),
    Pane(TabId, PaneId),
    Session(SessionId),
    Command(Command),
    /// One of the agent profiles, by its place in the list.
    Agent(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    NewTab,
    SplitRight,
    SplitDown,
    Zoom,
    Resize,
    RenameTab,
    TabInk,
    Find,
    QuickSelect,
    Agent,
    Sessions,
    NewSession,
    Settings,
    ClosePane,
    CloseTab,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Mark {
    None,
    /// A tab, in its ink, as in the strip.
    Dot(Hsla),
    /// A pane, in its tab's ink, as in an unfocused pane's head.
    Ring(Hsla),
}

struct Row {
    destination: Destination,
    kind: SharedString,
    mark: Mark,
    label: String,
    tone: u32,
    hint: Option<(String, u32)>,
    /// Said where the hint would be once the list is narrowed and a pane no
    /// longer sits under its tab: the tab's name.
    home: Option<String>,
    /// Matched as well, though not shown: a tab's panes, a pane's tab, a
    /// command's other words.
    also: String,
    /// The map leaves a little room between tabs, sessions and commands.
    group: u8,
    /// Left out of the map, and found only by typing.
    quiet: bool,
    score: i32,
    /// Which characters of the label the query matched.
    matched: Vec<usize>,
}

impl Row {
    fn new(
        destination: Destination,
        kind: impl Into<SharedString>,
        label: String,
        group: u8,
    ) -> Self {
        Self {
            destination,
            kind: kind.into(),
            mark: Mark::None,
            label,
            tone: TEXT,
            hint: None,
            home: None,
            also: String::new(),
            group,
            quiet: false,
            score: 0,
            matched: Vec::new(),
        }
    }
}

impl MuxApp {
    /// Open the palette, or close it if it is open: ⌘P does both.
    pub(super) fn toggle_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_some() {
            self.close_palette(window, cx);
            return;
        }
        self.finish_tab_rename(true, window, cx);
        self.session_sheet = None;
        self.settings_sheet = None;
        self.backend.send(CommandMessage::ListSessions);
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder("tab, pane, session or command"));
        let subscription =
            cx.subscribe_in(&input, window, |this, input, event: &InputEvent, _, cx| {
                if !matches!(event, InputEvent::Change) {
                    return;
                }
                let query = input.read(cx).value().to_string();
                if let Some(palette) = this.palette.as_mut() {
                    palette.query = query;
                    palette.selected = None;
                    palette.scroll.set_offset(gpui::Point::default());
                }
                cx.notify();
            });
        let focus = input.clone();
        window.on_next_frame(move |window, cx| {
            focus.update(cx, |input, cx| input.focus(window, cx));
        });
        self.palette = Some(Palette {
            input,
            query: String::new(),
            selected: None,
            scroll: ScrollHandle::new(),
            pointer: None,
            _subscription: subscription,
        });
        self.mode = InputMode::Normal;
        cx.notify();
    }

    pub(super) fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.take().is_some() {
            self.restore_keyboard(window, cx);
            cx.notify();
        }
    }

    /// Remember the tab left behind when another becomes active, for the
    /// palette to open on.
    pub(super) fn remember_previous_tab(&mut self, before: Option<(SessionId, TabId)>) {
        let Some(session) = &self.session else {
            return;
        };
        match before {
            Some((session_id, tab_id)) if session_id == session.id => {
                if tab_id != session.active_tab {
                    self.previous_tab = Some(tab_id);
                }
            }
            _ => self.previous_tab = None,
        }
        if self
            .previous_tab
            .is_some_and(|tab_id| !session.tabs.iter().any(|tab| tab.id == tab_id))
        {
            self.previous_tab = None;
        }
    }

    pub(super) fn move_palette(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(query) = self.palette.as_ref().map(|palette| palette.query.clone()) else {
            return;
        };
        let rows = self.palette_rows(&query);
        if rows.is_empty() {
            return;
        }
        let index = wrapping_step(self.palette_choice(&rows), delta, rows.len());
        if let Some(palette) = self.palette.as_mut() {
            palette.selected = Some(rows[index].destination);
            palette.scroll.scroll_to_item(index);
        }
        cx.notify();
    }

    pub(super) fn choose_palette_row(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(query) = self.palette.as_ref().map(|palette| palette.query.clone()) else {
            return;
        };
        let rows = self.palette_rows(&query);
        if let Some(row) = rows.get(self.palette_choice(&rows)) {
            self.go_to(row.destination, window, cx);
        }
    }

    fn go_to(&mut self, destination: Destination, window: &mut Window, cx: &mut Context<Self>) {
        self.close_palette(window, cx);
        let active = self.active_tab_id();
        match destination {
            Destination::Tab(tab_id) => {
                if Some(tab_id) != active {
                    self.send_workspace(WorkspaceCommand::SelectTab(tab_id));
                }
            }
            Destination::Pane(tab_id, pane_id) => {
                if Some(tab_id) != active {
                    self.send_workspace(WorkspaceCommand::SelectTab(tab_id));
                }
                self.request_pane_focus(pane_id);
            }
            Destination::Session(session_id) => {
                self.backend.send(CommandMessage::AttachSession(session_id));
            }
            Destination::Command(command) => self.run_palette_command(command, window, cx),
            Destination::Agent(index) => self.open_agent_from_palette(index, window, cx),
        }
        cx.notify();
    }

    fn run_palette_command(
        &mut self,
        command: Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let action = match command {
            Command::NewTab => Action::NewTab,
            Command::SplitRight => Action::SplitPane(mux_workspace::SplitAxis::Horizontal),
            Command::SplitDown => Action::SplitPane(mux_workspace::SplitAxis::Vertical),
            Command::Zoom => Action::TogglePaneZoom,
            Command::Resize => Action::EnterMode(InputMode::Resize),
            Command::RenameTab => Action::RenameTab,
            Command::Agent => Action::OpenAgentSurface,
            Command::Sessions => Action::OpenSessionSwitcher,
            Command::Settings => Action::OpenSettings,
            Command::ClosePane => Action::ClosePane,
            Command::CloseTab => Action::CloseTab,
            Command::TabInk => {
                self.cycle_active_tab_ink();
                return;
            }
            Command::Find => {
                self.open_find(window, cx);
                return;
            }
            Command::QuickSelect => {
                self.toggle_quick_select(window, cx);
                return;
            }
            Command::NewSession => {
                self.start_session_here();
                return;
            }
        };
        self.perform_action(action, window, cx);
    }

    /// The pointer moved over a row: it becomes the choice, unless the
    /// pointer hasn't really moved and the list scrolled under it instead.
    fn point_at_palette_row(
        &mut self,
        destination: Destination,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(palette) = self.palette.as_mut() else {
            return;
        };
        if palette.pointer.replace(position) == Some(position) {
            return;
        }
        if palette.selected != Some(destination) {
            palette.selected = Some(destination);
            cx.notify();
        }
    }

    /// The row the keys act on: the one chosen while it is listed, else the
    /// best match, or with nothing typed the tab you were in before this one.
    fn palette_choice(&self, rows: &[Row]) -> usize {
        let Some(palette) = self.palette.as_ref() else {
            return 0;
        };
        let position =
            |destination: Destination| rows.iter().position(|row| row.destination == destination);
        if let Some(index) = palette.selected.and_then(position) {
            return index;
        }
        if !palette.query.trim().is_empty() {
            return 0;
        }
        let active = self.active_tab_id();
        self.previous_tab
            .and_then(|tab_id| position(Destination::Tab(tab_id)))
            .or_else(|| {
                rows.iter().position(|row| {
                    matches!(row.destination, Destination::Tab(tab_id) if Some(tab_id) != active)
                })
            })
            .unwrap_or(0)
    }

    /// The map narrowed to what `query` matches, best first.
    fn palette_rows(&self, query: &str) -> Vec<Row> {
        let mut rows = self.palette_map();
        let terms = query.split_whitespace().collect::<Vec<_>>();
        if terms.is_empty() {
            rows.retain(|row| !row.quiet);
            return rows;
        }
        let number = query.trim().parse::<usize>().ok();
        rows.retain_mut(|row| {
            let Some((score, matched)) = match_row(row, &terms, number) else {
                return false;
            };
            row.score = score;
            row.matched = matched;
            if row.hint.is_none() {
                row.hint = row.home.take().map(|home| (home, FAINT_TEXT));
            }
            true
        });
        rank(&mut rows);
        rows
    }

    /// Everything the palette can go to, in the order of the map.
    fn palette_map(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        if let Some(session) = &self.session {
            for (position, tab) in session.tabs.iter().enumerate() {
                self.push_tab_rows(&mut rows, position, tab, tab.id == session.active_tab);
            }
        }
        let current = self.session.as_ref().map(|session| session.id);
        for session in self
            .sessions
            .iter()
            .filter(|session| Some(session.id) != current)
        {
            let mut row = Row::new(
                Destination::Session(session.id),
                "session",
                session.name.clone(),
                1,
            );
            row.hint = Some((format_session_pane_count(session.pane_count), FAINT_TEXT));
            rows.push(row);
        }
        for (command, keys, label, also) in self.palette_commands() {
            let mut row = Row::new(Destination::Command(command), keys, label.to_owned(), 2);
            row.tone = MUTED_TEXT;
            also.clone_into(&mut row.also);
            rows.push(row);
        }
        self.push_agent_rows(&mut rows);
        rows
    }

    /// A row for each agent ⌃a offers: the one already in this tab to go
    /// back to, else one to start where the keys are.
    fn push_agent_rows(&self, rows: &mut Vec<Row>) {
        for (index, profile) in self.profiles.iter().enumerate() {
            if !self.settings.agent_enabled(&profile.id) {
                continue;
            }
            let name = agent_view::profile_title(profile);
            let running = self
                .agents_for_active_tab()
                .find(|agent| agent.name == profile.spec.name);
            let (label, hint) = match running {
                Some(agent) => (
                    format!("show {name}"),
                    agent_view::agent_state(agent.status)
                        .map(|(state, tone)| (state.to_owned(), tone)),
                ),
                None => (
                    format!("open {name} here"),
                    (self.agent_availability.get(&profile.id) == Some(&false))
                        .then(|| ("not installed".to_owned(), FAINT_TEXT)),
                ),
            };
            let mut row = Row::new(Destination::Agent(index), "agent", label, 2);
            row.tone = MUTED_TEXT;
            row.hint = hint;
            row.quiet = true;
            row.also = format!("{} agent acp", agent_view::agent_handle(profile));
            rows.push(row);
        }
    }

    /// Show an agent in the pane with the keys: the session of it already
    /// in this tab, or a new one. One that isn't installed opens the
    /// launcher on it, which says how to install it.
    fn open_agent_from_palette(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(profile) = self.profiles.get(index).cloned() else {
            return;
        };
        if self
            .active_agent_pane()
            .is_none_or(|pane_id| Some(pane_id) != self.focused_pane_id())
        {
            self.toggle_agent_pane(window, cx);
        }
        let running = self
            .agents_for_active_tab()
            .find(|agent| agent.name == profile.spec.name)
            .map(|agent| agent.id);
        if let Some(session_id) = running {
            self.select_active_tab_agent(Some(session_id));
            self.focus_agent_composer(window);
        } else if self.agent_availability.get(&profile.id) == Some(&false) {
            self.select_active_tab_agent(None);
            self.launcher_choice = Some(profile.id);
        } else {
            self.launch_agent_profile(&profile, window, cx);
        }
        cx.notify();
    }

    /// A tab's row, then a row for each of its panes when it has more than
    /// one.
    fn push_tab_rows(
        &self,
        rows: &mut Vec<Row>,
        position: usize,
        tab: &mux_workspace::Tab,
        active: bool,
    ) {
        let ink = self.tab_ink(tab.id);
        let name = self.tab_label(tab);
        let mut panes = Vec::new();
        tab.layout.pane_ids(&mut panes);
        let places = panes
            .iter()
            .enumerate()
            .map(|(index, pane_id)| self.pane_place(tab.id, *pane_id, index + 1))
            .collect::<Vec<_>>();
        let hint = self.palette_tab_hint(tab, active, &name, &places);
        let mut row = Row::new(
            Destination::Tab(tab.id),
            (position + 1).to_string(),
            name.clone(),
            0,
        );
        let calling = hint.as_ref().is_some_and(|(_, tone)| *tone == SIGNAL);
        row.mark = Mark::Dot(if calling { color(SIGNAL) } else { ink.color() });
        row.hint = hint;
        row.also = places.join(" ");
        rows.push(row);
        if panes.len() < 2 {
            return;
        }
        let focused = self.focused_pane_id();
        for (pane_id, place) in panes.into_iter().zip(places) {
            let mut row = Row::new(Destination::Pane(tab.id, pane_id), "", place, 0);
            row.tone = MUTED_TEXT;
            row.mark = if self.pane_call(pane_id).is_some() {
                Mark::Dot(color(SIGNAL))
            } else if self.pane_exits.contains_key(&pane_id) {
                Mark::Ring(color(FAINT_TEXT))
            } else {
                Mark::Ring(ink.color())
            };
            row.hint = self.palette_pane_hint(tab.id, pane_id, active && focused == Some(pane_id));
            row.home = Some(name.clone());
            row.also.clone_from(&name);
            rows.push(row);
        }
    }

    /// What a pane is showing, as its head says it.
    fn pane_place(&self, tab_id: TabId, pane_id: PaneId, number: usize) -> String {
        if self.agent_panes.get(&tab_id) == Some(&pane_id) {
            return self
                .tab_agent(tab_id)
                .map_or_else(|| "agent".to_owned(), agent_view::agent_title);
        }
        self.panes
            .get(&pane_id)
            .and_then(|pane| pane.title.title())
            .map_or_else(
                || format!("pane {number}"),
                |title| chrome::terminal_place(title).to_owned(),
            )
    }

    /// The agent a tab's agent pane shows, as `active_agent` finds it for
    /// the tab on screen.
    pub(super) fn tab_agent(&self, tab_id: TabId) -> Option<&AgentSessionSnapshot> {
        let mut agents = self.agents.iter().filter(move |agent| {
            agent.tab_id == Some(tab_id) && agent_session_is_visible(agent.status)
        });
        match self.selected_agents.get(&tab_id) {
            Some(Some(session_id)) => {
                let agents = agents.collect::<Vec<_>>();
                agents
                    .iter()
                    .find(|agent| agent.id == *session_id)
                    .or_else(|| agents.first())
                    .copied()
            }
            Some(None) => None,
            None => agents.next(),
        }
    }

    /// What a tab's row says after its name: what one of its panes or its
    /// agent wants, that it is the tab on screen, or, for a tab with one
    /// pane and a name of its own, what that pane is showing.
    fn palette_tab_hint(
        &self,
        tab: &mux_workspace::Tab,
        active: bool,
        name: &str,
        places: &[String],
    ) -> Option<(String, u32)> {
        if let Some(call) = self.tab_call(tab) {
            return Some((pane_calls::call_words(call).to_owned(), SIGNAL));
        }
        let agents = || {
            self.agents
                .iter()
                .filter(|agent| agent.tab_id == Some(tab.id))
        };
        let unseen = agents().any(|agent| self.agent_unseen.contains(&agent.id));
        match agent_tab_activity(agents().map(|agent| agent.status)) {
            Some(AgentTabActivity::Attention) => {
                return Some(("needs you".to_owned(), SIGNAL));
            }
            Some(AgentTabActivity::Working) => {
                return Some(("agent working".to_owned(), MUTED_TEXT));
            }
            Some(AgentTabActivity::Idle) if unseen => {
                return Some(("agent finished".to_owned(), SAGE));
            }
            _ => {}
        }
        if active {
            return Some(("current".to_owned(), SAGE));
        }
        match places {
            [place] if place != name => Some((place.clone(), FAINT_TEXT)),
            _ => None,
        }
    }

    fn palette_pane_hint(
        &self,
        tab_id: TabId,
        pane_id: PaneId,
        current: bool,
    ) -> Option<(String, u32)> {
        if let Some(call) = self.pane_call(pane_id) {
            return Some((pane_calls::call_words(call).to_owned(), SIGNAL));
        }
        if let Some(status) = self.pane_exits.get(&pane_id) {
            let tone = if status.success { FAINT_TEXT } else { SIGNAL };
            return Some((pane_exit::exit_words(*status), tone));
        }
        if self.agent_panes.get(&tab_id) == Some(&pane_id)
            && let Some((state, tone)) = self
                .tab_agent(tab_id)
                .and_then(|agent| agent_view::agent_state(agent.status))
        {
            return Some((state.to_owned(), tone));
        }
        current.then(|| ("current".to_owned(), SAGE))
    }

    /// What mux can do, each with the keys that do it and other words it
    /// answers to. Some say what they will do to the pane or tab on screen.
    fn palette_commands(&self) -> [(Command, &'static str, &'static str, &'static str); 15] {
        let zoomed = self
            .session
            .as_ref()
            .and_then(Session::active_tab)
            .is_some_and(|tab| tab.zoomed_pane.is_some());
        let agent_here = self
            .active_agent_pane()
            .is_some_and(|pane_id| Some(pane_id) == self.focused_pane_id());
        [
            (Command::NewTab, "⌃t n", "new tab", "create open add"),
            (
                Command::SplitRight,
                "⌃p r",
                "split right",
                "new pane beside vertical",
            ),
            (
                Command::SplitDown,
                "⌃p d",
                "split down",
                "new pane below horizontal",
            ),
            (
                Command::Zoom,
                "⌃p f",
                if zoomed {
                    "show every pane"
                } else {
                    "zoom this pane"
                },
                "maximise maximize fullscreen unzoom",
            ),
            (Command::Resize, "⌃p ⌃n", "resize panes", "size grow shrink"),
            (Command::RenameTab, "⌃t r", "rename this tab", "name title"),
            (
                Command::TabInk,
                "⌃t c",
                "change this tab's ink",
                "colour color",
            ),
            (
                Command::Find,
                "⌘f",
                "find in this pane",
                "search look grep history scrollback",
            ),
            (
                Command::QuickSelect,
                "⌘⇧space",
                "quick select",
                "copy paste url link path hash hint pick yank",
            ),
            (
                Command::Agent,
                "⌃a",
                if agent_here {
                    "back to the shell"
                } else {
                    "open an agent here"
                },
                "claude code codex copilot acp ai chat",
            ),
            (
                Command::Sessions,
                "⌘⇧s",
                "show sessions",
                "switch attach workspace",
            ),
            (
                Command::NewSession,
                "",
                "start a session here",
                "new session create workspace",
            ),
            (
                Command::Settings,
                "⌘,",
                "settings",
                "preferences agents config",
            ),
            (Command::ClosePane, "⌃p x", "close this pane", "kill exit"),
            (Command::CloseTab, "⌃t x", "close this tab", "kill exit"),
        ]
    }

    /// The palette over everything, on a scrim that closes it when clicked.
    pub(super) fn render_palette(
        &self,
        viewport: gpui::Size<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let palette = self.palette.as_ref()?;
        let rows = self.palette_rows(&palette.query);
        let chosen = self.palette_choice(&rows);
        let (width, height) = (f32::from(viewport.width), f32::from(viewport.height));
        let slab_width = PALETTE_WIDTH.min(width - 32.0);

        let list = Self::palette_list(palette, &rows, chosen, cx);
        let query = palette_query(&palette.input);
        let top = height * 0.14;
        let slab = v_flex()
            .id("palette")
            .key_context("MuxPalette")
            .on_action(cx.listener(|this, _: &NextPaletteRow, _, cx| this.move_palette(1, cx)))
            .on_action(cx.listener(|this, _: &PreviousPaletteRow, _, cx| {
                this.move_palette(-1, cx);
            }))
            .on_action(cx.listener(|this, _: &ChoosePaletteRow, window, cx| {
                this.choose_palette_row(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ClosePalette, window, cx| {
                this.close_palette(window, cx);
            }))
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .absolute()
            .top(px(top))
            .left(px((width - slab_width) / 2.0))
            .w(px(slab_width))
            .max_h(px(height * 0.72))
            .overflow_hidden()
            .rounded(px(12.0))
            .border_1()
            .border_color(hairline(0.10))
            .bg(color(SURFACE))
            .shadow_lg()
            .font_family(EMBEDDED_TERMINAL_FONT)
            .text_size(px(13.0))
            .text_color(color(TEXT))
            .child(query)
            .child(list);
        let scrim = div()
            .id("palette-scrim")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .bg(color(GROUND).opacity(0.66))
            .occlude()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, window, cx| this.close_palette(window, cx)),
            );
        if self.motion == MotionPreference::Reduced {
            return Some(scrim.child(slab).into_any_element());
        }
        let slab = slab.with_animation(
            "palette-drop",
            interface_animation(160),
            move |slab, delta| slab.top(px(top - 6.0 * (1.0 - delta))),
        );
        Some(
            scrim
                .child(slab)
                .with_animation("palette-in", interface_animation(120), |scrim, delta| {
                    scrim.opacity(delta)
                })
                .into_any_element(),
        )
    }

    /// The rows, scrolling once there are more than fit. With nothing typed
    /// a little room opens between tabs, sessions and commands.
    fn palette_list(
        palette: &Palette,
        rows: &[Row],
        chosen: usize,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let narrowed = !palette.query.trim().is_empty();
        let mut list = v_flex()
            .id("palette-rows")
            .flex_initial()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&palette.scroll)
            .p(px(LIST_PAD));
        let mut group = None;
        for (index, row) in rows.iter().enumerate() {
            let gap = !narrowed && group.is_some_and(|group| group != row.group);
            group = Some(row.group);
            list = list.child(Self::palette_line(index, row, index == chosen, gap, cx));
        }
        if rows.is_empty() {
            list = list.child(
                h_flex()
                    .h(px(ROW_HEIGHT))
                    .pl(px(LABEL_X - LIST_PAD))
                    .text_color(color(FAINT_TEXT))
                    .child("nothing matches"),
            );
        }
        list.into_any_element()
    }

    fn palette_line(
        index: usize,
        row: &Row,
        chosen: bool,
        gap: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let destination = row.destination;
        let label = StyledText::new(row.label.clone()).with_highlights(
            highlight_ranges(&row.label, &row.matched)
                .into_iter()
                .map(|range| {
                    (
                        range,
                        HighlightStyle {
                            color: Some(color(SIGNAL)),
                            ..HighlightStyle::default()
                        },
                    )
                }),
        );
        h_flex()
            .id(SharedString::from(format!("palette-row-{index}")))
            .flex_none()
            .h(px(ROW_HEIGHT))
            .when(gap, |line| line.mt(px(8.0)))
            .px(px(ROW_PAD))
            .items_center()
            .rounded(px(6.0))
            .cursor_pointer()
            .when(chosen, |line| line.bg(wash(0.10)))
            .on_mouse_move(
                cx.listener(move |this, event: &gpui::MouseMoveEvent, _, cx| {
                    this.point_at_palette_row(destination, event.position, cx);
                }),
            )
            .on_click(cx.listener(move |this, _, window, cx| this.go_to(destination, window, cx)))
            .child(
                div()
                    .flex_none()
                    .w(px(GUTTER))
                    .truncate()
                    .text_color(color(FAINT_TEXT))
                    .child(row.kind.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .ml(px(GUTTER_GAP))
                    .size(px(MARK))
                    .rounded_full()
                    .map(|mark| match row.mark {
                        Mark::Dot(ink) => mark.bg(ink),
                        Mark::Ring(ink) => mark.border_1().border_color(ink),
                        Mark::None => mark,
                    }),
            )
            .child(
                div()
                    .ml(px(MARK_GAP))
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(color(if chosen { TEXT } else { row.tone }))
                    .child(label),
            )
            .when_some(row.hint.clone(), |line, (hint, tone)| {
                line.child(
                    div()
                        .flex_none()
                        .max_w(px(220.0))
                        .ml(px(16.0))
                        .truncate()
                        .text_color(color(tone))
                        .child(hint),
                )
            })
            .into_any_element()
    }
}

/// The query line: gofer's prompt over the marks, what is typed in line
/// with the names, and the key that closes it.
fn palette_query(input: &Entity<InputState>) -> impl IntoElement {
    h_flex()
        .relative()
        .flex_none()
        .h(px(QUERY_HEIGHT))
        // The field pads its text by 4px; this puts what is typed in line
        // with the names below.
        .pl(px(LABEL_X - 4.0))
        .pr(px(16.0))
        .gap(px(16.0))
        .items_center()
        .border_b_1()
        .border_color(hairline(0.07))
        .child(
            h_flex()
                .absolute()
                .left(px(MARK_X - 1.0))
                .top_0()
                .h_full()
                .items_center()
                .text_size(px(14.0))
                .font_weight(FontWeight::BOLD)
                .child("›"),
        )
        .child(
            div().flex_1().min_w_0().child(
                Input::new(input)
                    .appearance(false)
                    .xsmall()
                    .font_family(EMBEDDED_TERMINAL_FONT)
                    .text_size(px(14.0))
                    .text_color(color(TEXT)),
            ),
        )
        .child(kbd("esc", color(FAINT_TEXT)))
}

/// How a row answers what was typed: every word of it in the row's name,
/// or failing that at the start of a word of what the row also stands for,
/// which counts for less. A bare number goes straight to that tab.
fn match_row(row: &Row, terms: &[&str], number: Option<usize>) -> Option<(i32, Vec<usize>)> {
    if matches!(row.destination, Destination::Tab(_))
        && number.is_some_and(|number| row.kind == number.to_string().as_str())
    {
        return Some((10_000, Vec::new()));
    }
    let mut score = 0;
    let mut matched = Vec::new();
    for term in terms {
        if let Some((points, positions)) = fuzzy_match(term, &row.label) {
            score += points;
            if is_a_word_of(term, &row.label) {
                score += WHOLE_WORD;
            }
            matched.extend(positions);
        } else {
            score += word_prefix_match(term, &row.also)?;
        }
    }
    matched.sort_unstable();
    matched.dedup();
    Some((score, matched))
}

/// How well `query` matches `text`, the way fzf finds it: every character
/// of the query in order, with runs, the starts of words and the start of
/// the text worth more, and gaps worth less. Returns the score and which
/// characters of `text` matched.
fn fuzzy_match(query: &str, text: &str) -> Option<(i32, Vec<usize>)> {
    const MATCH: i32 = 16;
    const GAP_START: i32 = -3;
    const GAP_EXTEND: i32 = -1;
    const RUN: i32 = 8;
    const NONE: i32 = i32::MIN / 4;

    let query = query
        .chars()
        .flat_map(char::to_lowercase)
        .collect::<Vec<_>>();
    let chars = text.chars().collect::<Vec<_>>();
    let lower = chars
        .iter()
        .map(|character| character.to_lowercase().next().unwrap_or(*character))
        .collect::<Vec<_>>();
    let (m, n) = (query.len(), chars.len());
    if m == 0 {
        return Some((0, Vec::new()));
    }
    let mut rest = lower.iter();
    if !query
        .iter()
        .all(|wanted| rest.any(|character| character == wanted))
    {
        return None;
    }
    let bonus = (0..n)
        .map(|index| word_start_bonus(&chars, index))
        .collect::<Vec<_>>();
    // score[i * n + j]: the best way to match query[..=i] with query[i] at
    // text[j]; run is the bonus a run of matches carries along; from is the
    // previous match, for reading the positions back.
    let mut score = vec![NONE; m * n];
    let mut run = vec![0; m * n];
    let mut from = vec![usize::MAX; m * n];
    for i in 0..m {
        let (mut gap, mut gap_from) = (NONE, usize::MAX);
        for j in 0..n {
            if i > 0 && j >= 2 {
                let skipped = score[(i - 1) * n + j - 2];
                let extended = if gap > NONE { gap + GAP_EXTEND } else { NONE };
                if skipped > NONE && skipped + GAP_START >= extended {
                    gap = skipped + GAP_START;
                    gap_from = j - 2;
                } else {
                    gap = extended;
                }
            }
            if lower[j] != query[i] {
                continue;
            }
            let cell = i * n + j;
            if i == 0 {
                score[cell] = MATCH + bonus[j] * 2;
                run[cell] = bonus[j];
                continue;
            }
            let after = (j >= 1)
                .then(|| (i - 1) * n + j - 1)
                .filter(|previous| score[*previous] > NONE)
                .map(|previous| {
                    let carried = bonus[j].max(run[previous]);
                    (score[previous] + MATCH + carried.max(RUN), carried)
                });
            let jump = (gap > NONE).then(|| gap + MATCH + bonus[j]);
            match (after, jump) {
                (Some((points, carried)), jump) if jump.is_none_or(|jump| points >= jump) => {
                    score[cell] = points;
                    run[cell] = carried;
                    from[cell] = j - 1;
                }
                (_, Some(points)) => {
                    score[cell] = points;
                    run[cell] = bonus[j];
                    from[cell] = gap_from;
                }
                _ => {}
            }
        }
    }
    let last = (m - 1) * n;
    let (end, best) = (0..n)
        .map(|j| (j, score[last + j]))
        .filter(|(_, points)| *points > NONE)
        .max_by_key(|(j, points)| (*points, Reverse(*j)))?;
    let mut positions = vec![0; m];
    let mut j = end;
    for i in (0..m).rev() {
        positions[i] = j;
        if i > 0 {
            j = from[i * n + j];
        }
    }
    Some((best, positions))
}

/// Best first; between equals, the shorter name, since what was typed is
/// more of it. Otherwise rows keep the order of the map.
fn rank(rows: &mut [Row]) {
    rows.sort_by_key(|row| (Reverse(row.score), row.label.chars().count()));
}

/// What typing a whole word of a name is worth over typing the start of one,
/// so `code` finds claude code before codex.
const WHOLE_WORD: i32 = 8;

fn is_a_word_of(term: &str, text: &str) -> bool {
    text.split(|character: char| !character.is_alphanumeric())
        .any(|word| word.eq_ignore_ascii_case(term))
}

/// Whether `term` starts one of the words of `text`, and what that is
/// worth: less than a match in a name, so names come first. Letters picked
/// out of the middle of other words would only be noise here.
fn word_prefix_match(term: &str, text: &str) -> Option<i32> {
    let term = term.to_lowercase();
    let text = text.to_lowercase();
    let mut previous = None;
    for (offset, character) in text.char_indices() {
        let starts_word = character.is_alphanumeric()
            && previous.is_none_or(|previous: char| !previous.is_alphanumeric());
        if starts_word && text[offset..].starts_with(&term) {
            return Some(6 * i32::try_from(term.chars().count()).unwrap_or(i32::MAX / 12));
        }
        previous = Some(character);
    }
    None
}

/// What a match is worth for where it falls: the start of the text, of a
/// path component, of a word, or a capital inside one.
fn word_start_bonus(chars: &[char], index: usize) -> i32 {
    let Some(&previous) = index
        .checked_sub(1)
        .and_then(|previous| chars.get(previous))
    else {
        return 10;
    };
    let current = chars[index];
    if previous == '/' {
        9
    } else if !previous.is_alphanumeric() && current.is_alphanumeric() {
        8
    } else if previous.is_lowercase() && current.is_uppercase() {
        7
    } else {
        0
    }
}

/// The byte ranges of `label` covering the characters at `matched`, joined
/// where they touch.
fn highlight_ranges(label: &str, matched: &[usize]) -> Vec<Range<usize>> {
    let mut ranges: Vec<Range<usize>> = Vec::new();
    for (index, (offset, character)) in label.char_indices().enumerate() {
        if matched.binary_search(&index).is_err() {
            continue;
        }
        let end = offset + character.len_utf8();
        match ranges.last_mut() {
            Some(range) if range.end == offset => range.end = end,
            _ => ranges.push(offset..end),
        }
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    fn positions(query: &str, text: &str) -> Option<Vec<usize>> {
        fuzzy_match(query, text).map(|(_, positions)| positions)
    }

    #[test]
    fn letters_match_in_order_at_the_starts_of_words() {
        assert_eq!(positions("sr", "split right"), Some(vec![0, 6]));
        assert_eq!(positions("mux", "~/projects/mux"), Some(vec![11, 12, 13]));
        assert_eq!(positions("CW", "cargo watch"), Some(vec![0, 6]));
        assert_eq!(positions("ba", "abc"), None);
        assert_eq!(positions("", "anything"), Some(vec![]));
    }

    #[test]
    fn a_run_beats_scattered_letters() {
        let (run, _) = fuzzy_match("test", "cargo test").unwrap();
        let (scattered, _) = fuzzy_match("test", "the extra step").unwrap();
        assert!(run > scattered);
    }

    #[test]
    fn a_name_outranks_what_a_row_also_stands_for() {
        let tab = TabId::new();
        let mut gofer = Row::new(Destination::Tab(tab), "2", "gofer".to_owned(), 0);
        gofer.also = "cargo watch".to_owned();
        let mut pane = Row::new(
            Destination::Pane(tab, PaneId::new()),
            "",
            "cargo watch".to_owned(),
            0,
        );
        pane.also = "gofer".to_owned();
        let named = |row: &Row, query: &str| match_row(row, &[query], None).unwrap().0;
        assert!(named(&gofer, "gofer") > named(&pane, "gofer"));
        assert!(named(&pane, "cargo") > named(&gofer, "cargo"));
        // Each word may match a different part of the row.
        assert!(match_row(&pane, &["gofer", "watch"], None).is_some());
        assert!(match_row(&pane, &["gofer", "vim"], None).is_none());
        // What a row stands for has to be matched from the start of a word.
        assert!(match_row(&gofer, &["wat"], None).is_some());
        assert!(match_row(&gofer, &["cw"], None).is_none());
        // A bare number is that tab.
        assert_eq!(match_row(&gofer, &["2"], Some(2)).unwrap().0, 10_000);
    }

    #[test]
    fn ties_go_to_the_name_the_typing_covers_more_of() {
        let row = |label: &str| {
            Row::new(
                Destination::Session(SessionId::new()),
                "",
                label.to_owned(),
                0,
            )
        };
        let mut rows = vec![row("open claude code here"), row("open codex here")];
        let best = |rows: &mut Vec<Row>, query: &str| {
            for row in rows.iter_mut() {
                row.score = match_row(row, &[query], None).map_or(i32::MIN, |(score, _)| score);
            }
            rank(rows);
            rows[0].label.clone()
        };
        assert_eq!(best(&mut rows, "cod"), "open codex here");
        assert_eq!(best(&mut rows, "code"), "open claude code here");
    }

    #[test]
    fn highlights_cover_whole_characters() {
        assert_eq!(highlight_ranges("a—bc", &[0, 1, 3]), vec![0..4, 5..6]);
        assert_eq!(highlight_ranges("abc", &[]), Vec::<Range<usize>>::new());
    }
}
