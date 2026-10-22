//! Option-click at a prompt moves the cursor there, as in Terminal and
//! iTerm. The shell is sent the arrow presses that walk its cursor to the
//! clicked cell, so it works wherever the arrows do: at a shell's prompt,
//! in an editor, in a REPL. Option-drag still selects a rectangle; only a
//! click that stays put moves the cursor.

use std::rc::Rc;

use mux_terminal::{RenderFrame, TerminalInteraction as _, TerminalPoint};

use super::{CommandMessage, MuxApp, PaneId, layout, terminal_key_event};

/// The furthest a click walks the cursor, so a click far from a short
/// prompt doesn't send a flood of arrows.
const MOST_STEPS: i32 = 2_000;

#[derive(Clone, Copy)]
pub(super) struct CursorClick {
    pane_id: PaneId,
    rect: layout::Rect,
    at: gpui::Point<gpui::Pixels>,
}

impl MuxApp {
    /// Note an Option-click, which moves the cursor if it ends where it
    /// started.
    pub(super) fn begin_cursor_click(
        &mut self,
        pane_id: PaneId,
        rect: layout::Rect,
        event: &gpui::MouseDownEvent,
    ) {
        self.cursor_click = (event.button == gpui::MouseButton::Left
            && event.modifiers.alt
            && !event.modifiers.shift
            && !event.modifiers.platform
            && event.click_count == 1)
            .then_some(CursorClick {
                pane_id,
                rect,
                at: event.position,
            });
    }

    /// A pointer that moves more than a cell from where it went down is
    /// dragging a selection, not clicking.
    pub(super) fn follow_cursor_click(&mut self, position: gpui::Point<gpui::Pixels>) {
        if let Some(click) = self.cursor_click {
            let moved = (f32::from(position.x) - f32::from(click.at.x))
                .abs()
                .max((f32::from(position.y) - f32::from(click.at.y)).abs());
            if moved > self.metrics.cell_width {
                self.cursor_click = None;
            }
        }
    }

    /// When an Option-click comes up where it went down, walk the cursor to
    /// the clicked cell.
    pub(super) fn finish_cursor_click(&mut self) {
        let Some(click) = self.cursor_click.take() else {
            return;
        };
        let Some(pane) = self.panes.get(&click.pane_id) else {
            return;
        };
        let frame = Rc::clone(&pane.frame);
        let Some(point) = self.selection_pointer(click.rect, &frame, click.at).point else {
            return;
        };
        let Some(steps) = cursor_steps(&frame, point) else {
            return;
        };
        let key = if steps < 0 { "left" } else { "right" };
        let keystroke = gpui::Keystroke {
            modifiers: gpui::Modifiers::default(),
            key: key.to_owned(),
            key_char: None,
        };
        let event = terminal_key_event(&keystroke, false, false, false);
        let Ok(arrow) = pane.with_engine(|engine| engine.encode_key(&event)) else {
            return;
        };
        let bytes = arrow.repeat(steps.unsigned_abs() as usize);
        if !bytes.is_empty() {
            let pane_id = click.pane_id;
            let _ = pane.with_engine(|engine| engine.set_selection(None));
            self.note_pane_key(pane_id);
            self.backend.send(CommandMessage::Write { pane_id, bytes });
        }
    }
}

/// How many cells, back (negative) or on, the cursor is from `point`,
/// reading the screen row by row. None when there is no cursor on screen
/// to walk, as when the pane is scrolled back, or the click is on it.
fn cursor_steps(frame: &RenderFrame, point: TerminalPoint) -> Option<i32> {
    if frame.scroll.is_scrolled() {
        return None;
    }
    let cursor = frame.cursor.as_ref().filter(|cursor| cursor.visible)?;
    let columns = i32::from(frame.cols);
    let from = i32::from(cursor.y) * columns + i32::from(cursor.x);
    let to = i32::from(point.row) * columns + i32::from(point.column);
    let steps = (to - from).clamp(-MOST_STEPS, MOST_STEPS);
    (steps != 0).then_some(steps)
}

#[cfg(test)]
mod tests {
    use mux_terminal::{
        CursorStyle, RenderCursor, RenderDirty, RenderFrame, Rgb, TerminalPoint,
        TerminalScrollState,
    };

    use super::cursor_steps;

    fn frame(cursor: Option<(u16, u16)>, scroll: TerminalScrollState) -> RenderFrame {
        RenderFrame {
            cols: 200,
            rows: 24,
            dirty: RenderDirty::Full,
            background: Rgb::default(),
            foreground: Rgb::default(),
            cursor: cursor.map(|(x, y)| RenderCursor {
                visible: true,
                blinking: false,
                x,
                y,
                style: CursorStyle::Block,
                color: Rgb::default(),
            }),
            scroll,
            row_metadata: Vec::new(),
            cells: Vec::new(),
        }
    }

    const AT_BOTTOM: TerminalScrollState = TerminalScrollState {
        total: 24,
        offset: 0,
        len: 24,
    };

    #[test]
    fn a_click_walks_the_cursor_across_rows_as_text_reads() {
        let frame = frame(Some((30, 2)), AT_BOTTOM);
        let click = |column, row| cursor_steps(&frame, TerminalPoint { column, row });
        assert_eq!(click(10, 2), Some(-20));
        assert_eq!(click(5, 3), Some(175), "on into the wrapped row below");
        assert_eq!(click(30, 2), None, "a click on the cursor moves nothing");
        assert_eq!(click(0, 23), Some(2_000), "far clicks are capped");
    }

    #[test]
    fn no_walk_without_a_cursor_on_screen() {
        let point = TerminalPoint { column: 1, row: 1 };
        assert_eq!(cursor_steps(&frame(None, AT_BOTTOM), point), None);
        let scrolled = TerminalScrollState {
            total: 100,
            offset: 10,
            len: 24,
        };
        assert_eq!(cursor_steps(&frame(Some((0, 0)), scrolled), point), None);
    }
}
