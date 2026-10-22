//! The menu a right click opens over a terminal pane.
//!
//! What can be done to the pane, each beside the keys that do it, as a Mac
//! app's context menu offers: copy and paste, splits, zoom, clear, find and
//! close, and on a link, opening or copying it. A program that takes the mouse, such as an editor, gets the click
//! instead, unless Shift is held. Escape, a click elsewhere or a key that
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
    Copy,
    Paste,
    SelectAll,
    SplitRight,
    SplitDown,
    Zoom,
    Clear,
    Find,
    Close,
}

pub(super) struct PaneMenu {
    pane_id: PaneId,
    position: gpui::Point<gpui::Pixels>,
    /// The link that was right-clicked, if one was.
    link: Option<String>,
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
        let link = self
            .link_under(pane_id, rect, position)
            .map(|link| link.url);
        self.pane_menu = Some(PaneMenu {
            pane_id,
            position,
            link,
            chosen: None,
        });
    }

    pub(super) fn pane_menu_open(&self) -> bool {
        self.pane_menu.is_some()
    }

    fn pane_menu_lines(&self, menu: &PaneMenu) -> Vec<Line> {
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
        if let Some(url) = &menu.link {
            let file = url.starts_with("file://");
            let open = if file { "show in Finder" } else { "open link" };
            let copy = if file { "copy path" } else { "copy link" };
            lines.push(line(Item::OpenLink, open, "⌘click", true, false));
            lines.push(line(Item::CopyLink, copy, "", true, false));
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
            line(Item::Clear, "clear", "⌘k", true, true),
            line(Item::Find, "find…", "⌘f", true, false),
            line(Item::Close, "close pane", "⌘w", true, true),
        ]);
        lines
    }

    fn run_pane_menu_item(&mut self, item: Item, window: &mut Window, cx: &mut Context<Self>) {
        let link = self.pane_menu.take().and_then(|menu| menu.link);
        match item {
            Item::OpenLink => {
                if let Some(url) = link {
                    self.open_link_url(&url, cx);
                }
            }
            Item::CopyLink => {
                if let Some(url) = link
                    && let Some(clipboard) = &mut self.clipboard
                    && clipboard
                        .set_text(links::link_text(&url).to_owned())
                        .is_ok()
                {
                    let what = if url.starts_with("file://") {
                        "copied the path"
                    } else {
                        "copied the link"
                    };
                    self.say_in_strip(what, false, cx);
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
            Item::Clear => self.clear_pane(cx),
            Item::Find => self.open_find(window, cx),
            Item::Close => self.run_shortcut(Action::ClosePane, window, cx),
        }
        cx.notify();
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
