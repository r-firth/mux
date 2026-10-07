//! A pane's history.
//!
//! The wheel moves through it. ⌘home and ⌘end go to either end, and ⌘ or ⇧
//! with page up and page down go a page at a time. Typing anything returns a
//! pane to its latest output, as in Ghostty, so what is typed is never sent
//! somewhere out of sight. While a pane is scrolled back, a thin thumb on its
//! right edge says where in its history it is.

use super::*;
use mux_terminal::TerminalScrollState;

/// The keys that move a pane through its history rather than reaching its
/// program. ⇧ with page up or down reaches the program when there is no
/// history to move through, as in a full-screen program's own screen.
fn scrollback_key(
    keystroke: &gpui::Keystroke,
    rows: u16,
    has_history: bool,
) -> Option<TerminalViewportScroll> {
    let modifiers = keystroke.modifiers;
    if modifiers.control || modifiers.alt {
        return None;
    }
    let command = modifiers.platform && !modifiers.shift;
    let shift = modifiers.shift && !modifiers.platform;
    let page = i64::from(rows.saturating_sub(1).max(1));
    match keystroke.key.as_str() {
        "home" if command => Some(TerminalViewportScroll::Top),
        "end" if command => Some(TerminalViewportScroll::Bottom),
        "pageup" if command || (shift && has_history) => Some(TerminalViewportScroll::Delta(-page)),
        "pagedown" if command || (shift && has_history) => {
            Some(TerminalViewportScroll::Delta(page))
        }
        _ => None,
    }
}

/// The thumb's height and its offset from the top of a track `track` tall.
fn thumb_geometry(scroll: TerminalScrollState, track: f32) -> (f32, f32) {
    let total = scroll.total.max(1) as f32;
    let shown = scroll.len as f32;
    let height = (track * shown / total).clamp(track.min(24.0), track);
    let travel = (total - shown).max(1.0);
    let top = (track - height) * (scroll.offset as f32 / travel).clamp(0.0, 1.0);
    (height, top)
}

impl MuxApp {
    /// A history key typed into the pane with the keys. Returns whether the
    /// key was one.
    pub(super) fn scrollback_key_down(
        &mut self,
        keystroke: &gpui::Keystroke,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(pane_id) = self.terminal_input_pane_id() else {
            return false;
        };
        let Some(pane) = self.panes.get(&pane_id) else {
            return false;
        };
        let scroll = pane.frame.scroll;
        let Some(movement) = scrollback_key(keystroke, pane.frame.rows, scroll.total > scroll.len)
        else {
            return false;
        };
        if self.scroll_viewport(pane_id, movement) {
            cx.notify();
        }
        true
    }

    /// Typing into a pane scrolled back through its history brings it back to
    /// the latest output.
    pub(super) fn return_to_latest(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        if self
            .panes
            .get(&pane_id)
            .is_some_and(|pane| pane.frame.scroll.is_scrolled())
            && self.scroll_viewport(pane_id, TerminalViewportScroll::Bottom)
        {
            cx.notify();
        }
    }

    pub(super) fn scroll_viewport(
        &mut self,
        pane_id: PaneId,
        movement: TerminalViewportScroll,
    ) -> bool {
        let Some(pane) = self.panes.get_mut(&pane_id) else {
            return false;
        };
        if let Err(error) = pane.with_engine(|engine| engine.scroll_viewport(movement)) {
            error!(%pane_id, %error, "could not scroll terminal viewport");
            return false;
        }
        if let Err(error) = pane.publish_frame() {
            error!(%pane_id, %error, "could not render scrolled terminal viewport");
            return false;
        }
        true
    }

    /// The thumb on a scrolled-back pane's right edge. While find is open in
    /// a pane with history, it shows there too, beside the ticks for what was
    /// found.
    pub(super) fn render_scroll_thumb(
        &self,
        geometry: layout::PaneGeometry,
    ) -> Option<gpui::AnyElement> {
        let scroll = self.panes.get(&geometry.pane_id)?.frame.scroll;
        let finding = self
            .find
            .as_ref()
            .is_some_and(|find| find.pane_id == geometry.pane_id)
            && scroll.total > scroll.len;
        if !scroll.is_scrolled() && !finding {
            return None;
        }
        let rect = geometry.surface();
        let inset = 6.0;
        let (height, top) = thumb_geometry(scroll, rect.height - inset * 2.0);
        Some(
            div()
                .absolute()
                .left(px(rect.x + rect.width - 7.0))
                .top(px(rect.y + inset + top))
                .w(px(3.0))
                .h(px(height))
                .rounded_full()
                .bg(color(MUTED_TEXT).opacity(0.38))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keystroke(key: &str, platform: bool, shift: bool) -> gpui::Keystroke {
        gpui::Keystroke {
            modifiers: gpui::Modifiers {
                platform,
                shift,
                ..gpui::Modifiers::default()
            },
            key: key.to_owned(),
            key_char: None,
        }
    }

    #[test]
    fn history_keys_move_the_view_and_leave_programs_their_own_keys() {
        assert_eq!(
            scrollback_key(&keystroke("end", true, false), 40, true),
            Some(TerminalViewportScroll::Bottom)
        );
        assert_eq!(
            scrollback_key(&keystroke("pageup", false, true), 40, true),
            Some(TerminalViewportScroll::Delta(-39))
        );
        // A full-screen program has no history; its ⇧page keys are its own.
        assert_eq!(
            scrollback_key(&keystroke("pageup", false, true), 40, false),
            None
        );
        // Plain page keys always reach the program.
        assert_eq!(
            scrollback_key(&keystroke("pagedown", false, false), 40, true),
            None
        );
    }

    #[test]
    fn the_thumb_sits_where_the_view_is() {
        let at = |offset| TerminalScrollState {
            total: 400,
            offset,
            len: 40,
        };
        let (height, top) = thumb_geometry(at(0), 400.0);
        assert!((height - 40.0).abs() < 0.01);
        assert!(top.abs() < 0.01);
        let (_, top) = thumb_geometry(at(360), 400.0);
        assert!((top - 360.0).abs() < 0.01);
        // A long history keeps the thumb big enough to see.
        let (height, _) = thumb_geometry(
            TerminalScrollState {
                total: 100_000,
                offset: 0,
                len: 40,
            },
            400.0,
        );
        assert!((height - 24.0).abs() < 0.01);
    }
}
