//! Text the keyboard composes. A dead key (Option+E then E for é) and an
//! input method (Japanese, Chinese, Korean) build their text over several
//! keys before handing it over; until they do, what they have so far shows
//! at the cursor, underlined, with the input method's candidates beside it.
//! What they hand over is typed into the pane as it is.
//!
//! Plain keys still reach the pane as keys, so a program that asked for the
//! kitty keyboard protocol hears them as such. With Option as Meta, Option
//! composes nothing, so nothing here listens.

use std::ops::Range;

use gpui::{InputHandler, Pixels, UTF16Selection};

use super::*;

impl MuxApp {
    /// Where the pane with the keys has its cursor, in the window, and the
    /// input handler that listens there, with what is being composed shown
    /// over the cursor.
    pub(super) fn render_ime(
        &self,
        geometry: layout::PaneGeometry,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        if option_as_alt()
            || self.terminal_input_pane_id() != Some(geometry.pane_id)
            || self.field_has_keyboard()
        {
            return Vec::new();
        }
        let Some(pane) = self.panes.get(&geometry.pane_id) else {
            return Vec::new();
        };
        let metrics = self.metrics;
        let rect = geometry.rect;
        let padding =
            metrics.balanced_padding(rect.width, rect.height, pane.frame.cols, pane.frame.rows);
        let (column, row) = pane
            .frame
            .cursor
            .map_or((0, 0), |cursor| (cursor.x, cursor.y));
        let left = rect.x + padding.left + f32::from(column) * metrics.cell_width;
        let top = rect.y + padding.top + f32::from(row) * metrics.cell_height;
        let app = cx.entity();
        let focus = self.focus_handle.clone();
        let listener = gpui::canvas(
            |_, _, _| {},
            move |bounds, (), window, cx| {
                window.handle_input(
                    &focus,
                    TerminalInput {
                        app,
                        cursor: bounds,
                    },
                    cx,
                );
            },
        )
        .absolute()
        .left(px(left))
        .top(px(top))
        .w(px(metrics.cell_width))
        .h(px(metrics.cell_height))
        .into_any_element();
        let mut elements = vec![listener];
        if let Some(marked) = self.ime_marked.as_ref().filter(|text| !text.is_empty()) {
            elements.push(
                div()
                    .absolute()
                    .left(px(left))
                    .top(px(top))
                    .h(px(metrics.cell_height))
                    .flex()
                    .items_center()
                    .bg(color(SURFACE))
                    .font_family(self.terminal_font.clone())
                    .text_size(px(metrics.font_size))
                    .text_color(color(TEXT))
                    .border_b_1()
                    .border_color(self.active_ink().color())
                    .child(marked.clone())
                    .into_any_element(),
            );
        }
        elements
    }

    /// The input method handed over its text: type it into the pane, and
    /// every pane synced with it.
    fn commit_composed(&mut self, text: &str, cx: &mut Context<Self>) {
        self.ime_marked = None;
        let Some(pane_id) = self.terminal_input_pane_id() else {
            cx.notify();
            return;
        };
        if !text.is_empty() {
            self.return_to_latest(pane_id, cx);
            self.clear_selection_on_typing(pane_id);
            for target in self.key_targets(pane_id) {
                self.note_pane_key(target);
                self.backend.send(CommandMessage::Write {
                    pane_id: target,
                    bytes: text.as_bytes().to_vec(),
                });
            }
        }
        cx.notify();
    }

    fn mark_composing(&mut self, text: &str, cx: &mut Context<Self>) {
        let marked = (!text.is_empty()).then(|| text.to_owned());
        if self.ime_marked != marked {
            self.ime_marked = marked;
            cx.notify();
        }
    }
}

/// The platform's way in for composed text, at the cursor of the pane with
/// the keys.
struct TerminalInput {
    app: Entity<MuxApp>,
    cursor: Bounds<Pixels>,
}

impl InputHandler for TerminalInput {
    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<UTF16Selection> {
        // An input method places its candidates by the selection, which in a
        // terminal is always the cursor.
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(&mut self, _window: &mut Window, cx: &mut App) -> Option<Range<usize>> {
        marked_range(self.app.read(cx).ime_marked.as_deref())
    }

    fn text_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        _adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<String> {
        None
    }

    fn replace_text_in_range(
        &mut self,
        _replacement_range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut App,
    ) {
        self.app.update(cx, |app, cx| app.commit_composed(text, cx));
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range_utf16: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut App,
    ) {
        self.app
            .update(cx, |app, cx| app.mark_composing(new_text, cx));
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut App) {
        self.app.update(cx, |app, cx| app.mark_composing("", cx));
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        let mut bounds = self.cursor;
        bounds.origin.x += bounds.size.width * range_utf16.start as f32;
        Some(bounds)
    }

    fn character_index_for_point(
        &mut self,
        _point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<usize> {
        None
    }

    fn apple_press_and_hold_enabled(&mut self) -> bool {
        // A held key repeats in a terminal; it does not offer accents.
        false
    }

    fn prefers_ime_for_printable_keys(&mut self, _window: &mut Window, _cx: &mut App) -> bool {
        // Only asked while an input method is the keyboard: then it composes
        // what is typed, rather than the pane hearing each key.
        true
    }
}

/// The span of composed text, in UTF-16 units as the platform counts.
fn marked_range(marked: Option<&str>) -> Option<Range<usize>> {
    marked
        .filter(|text| !text.is_empty())
        .map(|text| 0..text.encode_utf16().count())
}

#[cfg(test)]
mod tests {
    use super::marked_range;

    #[test]
    fn composed_text_is_measured_as_the_platform_counts() {
        assert_eq!(marked_range(None), None);
        assert_eq!(marked_range(Some("")), None);
        assert_eq!(marked_range(Some("´")), Some(0..1));
        assert_eq!(marked_range(Some("にほ")), Some(0..2));
        assert_eq!(marked_range(Some("𝄞")), Some(0..2));
    }
}
