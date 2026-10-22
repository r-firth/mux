//! The menu a right click opens over a terminal pane, or over a tab's chip.
//!
//! What can be done to the pane, each beside the keys that do it, as a Mac
//! app's context menu offers: copy and paste, splits, zoom, evening out,
//! naming the pane or moving it to a tab of its own, clear, find, showing its directory and close;
//! on a link, opening or copying it; and on a command or what it printed,
//! when the shell marks its prompts, copying the command, its output, or both. A
//! program that takes the mouse, such as an editor, gets the click instead,
//! unless Shift is held. Escape, a click elsewhere or a key that
//! isn't the menu's puts it away.

use super::*;

const MENU_WIDTH: f32 = 236.0;
const ITEM_HEIGHT: f32 = 26.0;
const MENU_PAD: f32 = 5.0;
const RULE_GAP: f32 = 9.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Item {
    OpenLink,
    CopyLink,
    CopyCommand,
    CopyOutput,
    CopyTranscript,
    Copy,
    Paste,
    SelectAll,
    SplitRight,
    SplitDown,
    Zoom,
    EvenOut,
    NamePane,
    BreakPane,
    Clear,
    Find,
    OpenHistory,
    RevealDirectory,
    Close,
    RenameTab,
    TabInk,
    SyncTab,
    MoveTabLeft,
    MoveTabRight,
    CloseTab,
    CloseOtherTabs,
}

pub(super) struct PaneMenu {
    pane_id: PaneId,
    /// The tab whose chip was right-clicked, for a tab's menu.
    tab: Option<TabId>,
    position: gpui::Point<gpui::Pixels>,
    /// The link that was right-clicked, if one was.
    link: Option<links::Link>,
    /// The prompt of the command whose line or output was right-clicked.
    command: Option<usize>,
    /// The item the keys or the pointer are on.
    chosen: Option<usize>,
}

/// One line of the menu: what it does, its label, its keys, and whether it
/// can be done now. A rule goes above the first item of each group.
struct Line {
    item: Item,
    label: &'static str,
    keys: &'static str,
    enabled: bool,
    rule_above: bool,
}

impl MuxApp {
    pub(super) fn open_pane_menu(
        &mut self,
        pane_id: PaneId,
        rect: layout::Rect,
        position: gpui::Point<gpui::Pixels>,
    ) {
        let link = self.link_under(pane_id, rect, position);
        let command = self.prompt_under(pane_id, rect, position);
        self.pane_menu = Some(PaneMenu {
            pane_id,
            tab: None,
            position,
            link,
            command,
            chosen: None,
        });
    }

    /// A tab's menu, by its chip: what the Tab mode keys do to it. The tab
    /// is shown first, so the menu acts on what is on screen.
    pub(super) fn open_tab_menu(&mut self, tab_id: TabId, position: gpui::Point<gpui::Pixels>) {
        let Some(tab) = self
            .session
            .as_ref()
            .and_then(|session| session.tabs.iter().find(|tab| tab.id == tab_id))
        else {
            return;
        };
        let pane_id = tab.focused_pane;
        if self.active_tab_id() != Some(tab_id) {
            self.send_workspace(WorkspaceCommand::SelectTab(tab_id));
        }
        self.pane_menu = Some(PaneMenu {
            pane_id,
            tab: Some(tab_id),
            // Under the strip, so the chip stays in sight.
            position: gpui::point(position.x, position.y.max(px(30.0))),
            link: None,
            command: None,
            chosen: None,
        });
    }

    fn tab_menu_lines(&self, tab_id: TabId) -> Vec<Line> {
        let (place, count) = self
            .session
            .as_ref()
            .and_then(|session| {
                let place = session.tabs.iter().position(|tab| tab.id == tab_id)?;
                Some((place, session.tabs.len()))
            })
            .unwrap_or((0, 1));
        let synced = self.synced_tabs.contains(&tab_id);
        let line = |item, label, keys, enabled, rule_above| Line {
            item,
            label,
            keys,
            enabled,
            rule_above,
        };
        vec![
            line(Item::RenameTab, "rename", "⌃t r", true, false),
            line(Item::TabInk, "change colour", "⌃t c", true, false),
            line(
                Item::SyncTab,
                if synced {
                    "stop typing into every pane"
                } else {
                    "type into every pane"
                },
                "⌃t s",
                true,
                false,
            ),
            line(Item::MoveTabLeft, "move left", "⌃t ⇧h", place > 0, true),
            line(
                Item::MoveTabRight,
                "move right",
                "⌃t ⇧l",
                place + 1 < count,
                false,
            ),
            line(Item::CloseTab, "close tab", "⌃t x", count > 1, true),
            line(
                Item::CloseOtherTabs,
                "close other tabs",
                "",
                count > 1,
                false,
            ),
        ]
    }

    pub(super) fn pane_menu_open(&self) -> bool {
        self.pane_menu.is_some()
    }

    fn pane_menu_lines(&self, menu: &PaneMenu) -> Vec<Line> {
        if let Some(tab_id) = menu.tab {
            return self.tab_menu_lines(tab_id);
        }
        let pane_id = menu.pane_id;
        let zoomed = self
            .session
            .as_ref()
            .and_then(Session::active_tab)
            .is_some_and(|tab| tab.zoomed_pane == Some(pane_id));
        let split = self
            .session
            .as_ref()
            .and_then(Session::active_tab)
            .is_some_and(|tab| !matches!(tab.layout, mux_workspace::PaneLayout::Leaf(_)));
        let line = |item, label, keys, enabled, rule_above| Line {
            item,
            label,
            keys,
            enabled,
            rule_above,
        };
        let mut lines = Vec::new();
        if let Some(link) = &menu.link {
            let file = link.path || link.url.starts_with("file://");
            let open = if file { "show in Finder" } else { "open link" };
            let copy = if file { "copy path" } else { "copy link" };
            lines.push(line(Item::OpenLink, open, "⌘click", true, false));
            lines.push(line(Item::CopyLink, copy, "", true, false));
        }
        if menu.command.is_some() {
            let rule = !lines.is_empty();
            lines.push(line(Item::CopyCommand, "copy command", "", true, rule));
            lines.push(line(Item::CopyOutput, "copy its output", "", true, false));
            lines.push(line(
                Item::CopyTranscript,
                "copy command and output",
                "",
                true,
                false,
            ));
        }
        let linked = !lines.is_empty();
        lines.extend([
            line(
                Item::Copy,
                "copy",
                "⌘c",
                self.selected_text().is_some(),
                linked,
            ),
            line(Item::Paste, "paste", "⌘v", true, false),
            line(Item::SelectAll, "select all", "⌘a", true, false),
            line(Item::SplitRight, "split right", "⌘d", true, true),
            line(Item::SplitDown, "split down", "⌘⇧d", true, false),
            line(
                Item::Zoom,
                if zoomed { "show every pane" } else { "zoom" },
                "⌘⇧↩",
                split || zoomed,
                false,
            ),
            line(Item::EvenOut, "even out panes", "⌘⌃=", split, false),
            line(Item::NamePane, "name pane…", "⌃p c", true, true),
            line(Item::BreakPane, "move to new tab", "⌃t b", split, false),
            line(Item::Clear, "clear", "⌘k", true, true),
            line(Item::Find, "find…", "⌘f", true, false),
            line(Item::OpenHistory, "open history in editor", "", true, false),
            line(
                Item::RevealDirectory,
                "show directory in Finder",
                "",
                true,
                false,
            ),
            line(Item::Close, "close pane", "⌘w", true, true),
        ]);
        lines
    }

    fn run_pane_menu_item(&mut self, item: Item, window: &mut Window, cx: &mut Context<Self>) {
        let Some(PaneMenu {
            pane_id,
            tab,
            link,
            command,
            ..
        }) = self.pane_menu.take()
        else {
            return;
        };
        if let Some(tab_id) = tab {
            self.run_tab_menu_item(item, tab_id, window, cx);
            cx.notify();
            return;
        }
        let pane_id = Some(pane_id);
        match item {
            Item::OpenLink => {
                if let (Some(pane_id), Some(link)) = (pane_id, link) {
                    self.follow_link(pane_id, &link, cx);
                }
            }
            Item::CopyLink => {
                if let Some(link) = link
                    && let Some(clipboard) = &mut self.clipboard
                    && clipboard
                        .set_text(links::link_text(&link.url).to_owned())
                        .is_ok()
                {
                    let what = if link.path || link.url.starts_with("file://") {
                        "copied the path"
                    } else {
                        "copied the link"
                    };
                    self.say_in_strip(what, false, cx);
                }
            }
            Item::CopyCommand => {
                if let (Some(pane_id), Some(prompt)) = (pane_id, command) {
                    self.copy_command(pane_id, prompt, cx);
                }
            }
            Item::CopyOutput => {
                if let (Some(pane_id), Some(prompt)) = (pane_id, command) {
                    self.copy_output(pane_id, prompt, cx);
                }
            }
            Item::CopyTranscript => {
                if let (Some(pane_id), Some(prompt)) = (pane_id, command) {
                    self.copy_transcript(pane_id, prompt, cx);
                }
            }
            Item::Copy => self.copy_selection(cx),
            Item::Paste => self.paste_clipboard(cx),
            Item::SelectAll => self.select_all(),
            Item::SplitRight => self.run_shortcut(
                Action::SplitPane(mux_workspace::SplitAxis::Horizontal),
                window,
                cx,
            ),
            Item::SplitDown => {
                self.run_shortcut(
                    Action::SplitPane(mux_workspace::SplitAxis::Vertical),
                    window,
                    cx,
                );
            }
            Item::Zoom => self.run_shortcut(Action::TogglePaneZoom, window, cx),
            Item::EvenOut => self.run_shortcut(Action::EqualizePanes, window, cx),
            Item::NamePane => {
                if let Some(pane_id) = pane_id {
                    self.request_pane_focus(pane_id);
                }
                self.open_rename_pane(window, cx);
            }
            Item::BreakPane => {
                if let Some(pane_id) = pane_id {
                    self.request_pane_focus(pane_id);
                }
                self.run_shortcut(Action::BreakPaneToTab, window, cx);
            }
            Item::Clear => self.clear_pane(cx),
            Item::Find => self.open_find(window, cx),
            Item::OpenHistory => {
                if let Some(pane_id) = pane_id {
                    self.open_history_in_editor(pane_id, cx);
                }
            }
            Item::RevealDirectory => {
                if let Some(pane_id) = pane_id {
                    self.reveal_directory(pane_id);
                }
            }
            Item::Close => self.run_shortcut(Action::ClosePane, window, cx),
            _ => {}
        }
        cx.notify();
    }

    fn run_tab_menu_item(
        &mut self,
        item: Item,
        tab_id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match item {
            Item::RenameTab => self.open_rename_tab(window, cx),
            Item::TabInk => self.cycle_active_tab_ink(),
            Item::SyncTab => self.run_shortcut(Action::ToggleSyncTab, window, cx),
            Item::MoveTabLeft => self.run_shortcut(Action::MoveTab(Direction::Left), window, cx),
            Item::MoveTabRight => {
                self.run_shortcut(Action::MoveTab(Direction::Right), window, cx);
            }
            Item::CloseTab => self.request_close(CloseTarget::Tab(tab_id)),
            Item::CloseOtherTabs => self.request_close(CloseTarget::OtherTabs(tab_id)),
            _ => {}
        }
    }

    /// The keys while the menu is open: the arrows choose, Return does the
    /// chosen item, Escape puts the menu away. Any other key puts it away
    /// and goes on to do what it does. Returns whether the key was the
    /// menu's.
    pub(super) fn pane_menu_key(
        &mut self,
        keystroke: &gpui::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(menu) = &self.pane_menu else {
            return false;
        };
        let lines = self.pane_menu_lines(menu);
        let enabled = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.enabled)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let step = |forward: bool| {
            let at = menu
                .chosen
                .and_then(|chosen| enabled.iter().position(|index| *index == chosen));
            let next = match (at, forward) {
                (None, true) => 0,
                (None, false) => enabled.len().saturating_sub(1),
                (Some(at), true) => (at + 1) % enabled.len(),
                (Some(at), false) => (at + enabled.len() - 1) % enabled.len(),
            };
            enabled.get(next).copied()
        };
        match keystroke.key.as_str() {
            "down" => {
                let chosen = step(true);
                if let Some(menu) = &mut self.pane_menu {
                    menu.chosen = chosen;
                }
            }
            "up" => {
                let chosen = step(false);
                if let Some(menu) = &mut self.pane_menu {
                    menu.chosen = chosen;
                }
            }
            "enter" => {
                if let Some(line) = menu.chosen.and_then(|chosen| lines.get(chosen)) {
                    let item = line.item;
                    self.run_pane_menu_item(item, window, cx);
                } else {
                    self.pane_menu = None;
                }
            }
            "escape" => self.pane_menu = None,
            _ => {
                self.pane_menu = None;
                cx.notify();
                return false;
            }
        }
        cx.notify();
        true
    }

    /// The menu by the pointer, kept inside the window, on a clear scrim
    /// that puts it away when clicked.
    pub(super) fn render_pane_menu(
        &self,
        viewport: gpui::Size<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let menu = self.pane_menu.as_ref()?;
        let lines = self.pane_menu_lines(menu);
        let rules = lines.iter().filter(|line| line.rule_above).count();
        #[allow(clippy::cast_precision_loss)]
        let height =
            lines.len() as f32 * ITEM_HEIGHT + rules as f32 * RULE_GAP + MENU_PAD * 2.0 + 2.0;
        let (width, window_height) = (f32::from(viewport.width), f32::from(viewport.height));
        let left = f32::from(menu.position.x)
            .min(width - MENU_WIDTH - 8.0)
            .max(8.0);
        let top = f32::from(menu.position.y)
            .min(window_height - height - 8.0)
            .max(8.0);

        let mut list = v_flex()
            .id("pane-menu")
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(MENU_WIDTH))
            .p(px(MENU_PAD))
            .rounded(px(9.0))
            .border_1()
            .border_color(hairline(0.10))
            .bg(color(SURFACE))
            .shadow_lg()
            .font_family(EMBEDDED_TERMINAL_FONT)
            .text_size(px(12.5))
            .text_color(color(TEXT))
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(gpui::MouseButton::Right, |_, _, cx| cx.stop_propagation());
        for (index, line) in lines.into_iter().enumerate() {
            if line.rule_above {
                list = list.child(
                    div()
                        .flex_none()
                        .h(px(RULE_GAP))
                        .mx(px(6.0))
                        .flex()
                        .items_center()
                        .child(div().w_full().h(px(1.0)).bg(hairline(0.08))),
                );
            }
            list = list.child(Self::pane_menu_line(
                index,
                &line,
                menu.chosen == Some(index),
                cx,
            ));
        }
        let scrim = div()
            .id("pane-menu-scrim")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .on_any_mouse_down(cx.listener(|this, _, _, cx| {
                this.pane_menu = None;
                cx.notify();
            }))
            .child(list);
        if self.motion == MotionPreference::Reduced {
            return Some(scrim.into_any_element());
        }
        Some(
            scrim
                .with_animation("pane-menu-in", interface_animation(90), |scrim, delta| {
                    scrim.opacity(delta)
                })
                .into_any_element(),
        )
    }

    fn pane_menu_line(
        index: usize,
        line: &Line,
        chosen: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let item = line.item;
        let enabled = line.enabled;
        h_flex()
            .id(SharedString::from(format!("pane-menu-{index}")))
            .flex_none()
            .h(px(ITEM_HEIGHT))
            .px(px(9.0))
            .items_center()
            .justify_between()
            .rounded(px(5.0))
            .text_color(color(if enabled { TEXT } else { FAINT_TEXT }))
            .when(enabled && chosen, |line| line.bg(wash(0.10)))
            .when(enabled, |line| {
                line.cursor_pointer()
                    .on_mouse_move(cx.listener(move |this, _, _, cx| {
                        if let Some(menu) = &mut this.pane_menu
                            && menu.chosen != Some(index)
                        {
                            menu.chosen = Some(index);
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.run_pane_menu_item(item, window, cx);
                    }))
            })
            .child(line.label)
            .child(
                div()
                    .text_color(color(FAINT_TEXT).opacity(if enabled { 1.0 } else { 0.6 }))
                    .child(line.keys),
            )
    }
}
