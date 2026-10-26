//! Shift with an arrow moves the end of a selection, as Ghostty's
//! `adjust_selection` does by default: a cell left or right, a row up or
//! down. Without a selection on screen the keys go to the program as ever.

use mux_terminal::{RenderFrame, TerminalInteraction as _, TerminalPoint, TerminalSelection};

use super::{Context, MuxApp};

/// The keyboard's last adjustment, so the end it moves stays the end it
/// moved even after it crosses the other.
pub(super) type KeyedSelection = Option<(super::PaneId, TerminalSelection)>;

/// How an arrow moves the end of a selection: columns, then rows.
fn arrow_step(key: &str) -> Option<(i32, i32)> {
    match key {
        "left" => Some((-1, 0)),
        "right" => Some((1, 0)),
        "up" => Some((0, -1)),
        "down" => Some((0, 1)),
        _ => None,
    }
}

/// The first and last selected cells on screen, in reading order.
fn selected_span(frame: &RenderFrame) -> Option<(TerminalPoint, TerminalPoint)> {
    let columns = usize::from(frame.cols.max(1));
    let point = |index: usize| TerminalPoint {
        column: u16::try_from(index % columns).unwrap_or(u16::MAX),
        row: u16::try_from(index / columns).unwrap_or(u16::MAX),
    };
    let first = frame.cells.iter().position(|cell| cell.selected)?;
    let last = frame.cells.iter().rposition(|cell| cell.selected)?;
    Some((point(first), point(last)))
}

/// Move `point` a step, reading the screen as text: left from a row's start
/// goes to the end of the row above, and it stays on screen.
fn stepped(
    point: TerminalPoint,
    (columns, rows): (i32, i32),
    frame: &RenderFrame,
) -> TerminalPoint {
    let width = i32::from(frame.cols.max(1));
    let cells = width * i32::from(frame.rows.max(1));
    let at = i32::from(point.row) * width + i32::from(point.column);
    let to = (at + columns + rows * width).clamp(0, cells - 1);
    TerminalPoint {
        column: u16::try_from(to % width).unwrap_or(0),
        row: u16::try_from(to / width).unwrap_or(0),
    }
}

/// Order two points as text reads.
fn ordered(a: TerminalPoint, b: TerminalPoint) -> (TerminalPoint, TerminalPoint) {
    if (a.row, a.column) <= (b.row, b.column) {
        (a, b)
    } else {
        (b, a)
    }
}

impl MuxApp {
    /// Shift and an arrow, with a selection on screen in the pane with the
    /// keys, moves the selection's end. Returns whether the key did.
    pub(super) fn adjust_selection_key(
        &mut self,
        keystroke: &gpui::Keystroke,
        cx: &mut Context<Self>,
    ) -> bool {
        let modifiers = keystroke.modifiers;
        if !modifiers.shift || modifiers.control || modifiers.alt || modifiers.platform {
            return false;
        }
        let Some(step) = arrow_step(&keystroke.key) else {
            return false;
        };
        let Some(pane_id) = self
            .terminal_input_pane_id()
            .filter(|pane_id| self.selected_pane == Some(*pane_id))
        else {
            return false;
        };
        let Some(pane) = self.panes.get_mut(&pane_id) else {
            return false;
        };
        let frame = std::rc::Rc::clone(&pane.frame);
        let Some((first, last)) = selected_span(&frame) else {
            return false;
        };
        let current = self
            .keyed_selection
            .filter(|(keyed, selection)| {
                *keyed == pane_id && ordered(selection.anchor, selection.focus) == (first, last)
            })
            .map_or(
                TerminalSelection {
                    anchor: first,
                    focus: last,
                    rectangular: false,
                },
                |(_, selection)| selection,
            );
        let selection = TerminalSelection {
            focus: stepped(current.focus, step, &frame),
            ..current
        };
        if pane
            .with_engine(|engine| engine.set_selection(Some(selection)))
            .is_err()
        {
            return false;
        }
        let _ = pane.publish_frame();
        self.keyed_selection = Some((pane_id, selection));
        cx.notify();
        true
    }
}

#[cfg(test)]
mod tests {
    use mux_terminal::{RenderDirty, RenderFrame, Rgb, TerminalPoint, TerminalScrollState};

    use super::{arrow_step, stepped};

    fn frame() -> RenderFrame {
        RenderFrame {
            cols: 10,
            rows: 4,
            dirty: RenderDirty::Full,
            background: Rgb::default(),
            foreground: Rgb::default(),
            cursor: None,
            scroll: TerminalScrollState {
                total: 4,
                offset: 0,
                len: 4,
            },
            row_metadata: Vec::new(),
            cells: Vec::new(),
        }
    }

    #[test]
    fn the_end_steps_as_text_reads_and_stays_on_screen() {
        let frame = frame();
        let at = |column, row| TerminalPoint { column, row };
        let step = |point, key| stepped(point, arrow_step(key).unwrap(), &frame);
        assert_eq!(step(at(3, 1), "right"), at(4, 1));
        assert_eq!(step(at(0, 1), "left"), at(9, 0), "back onto the row above");
        assert_eq!(step(at(3, 1), "down"), at(3, 2));
        assert_eq!(
            step(at(3, 3), "down"),
            at(9, 3),
            "no further than the screen"
        );
        assert_eq!(step(at(0, 0), "up"), at(0, 0));
    }
}
