//! Option-click at a prompt moves the cursor there, as in Terminal and
//! iTerm. The shell is sent the arrow presses that walk its cursor to the
//! clicked cell, so it works wherever the arrows do: at a shell's prompt,
//! in an editor, in a REPL. Option-drag still selects a rectangle; only a
//! click that stays put moves the cursor.
//!
//! A plain click does the same on the line being typed at a shell's prompt,
//! as in Ghostty, unless its config sets `cursor-click-to-move = false`. The
//! shell's prompt marks say where that line is, so a click on what a command
//! printed only selects, and the walk stops at the ends of what is typed so
//! the shell has no reason to ring.

use std::rc::Rc;

use mux_terminal::{RenderFrame, TerminalInteraction as _, TerminalPoint};

use super::{CommandMessage, MuxApp, PaneId, ghostty_config, layout, terminal_key_event};

/// The furthest a click walks the cursor, so a click far from a short
/// prompt doesn't send a flood of arrows.
const MOST_STEPS: i32 = 2_000;

#[derive(Clone, Copy)]
pub(super) struct CursorClick {
    pane_id: PaneId,
    rect: layout::Rect,
    at: gpui::Point<gpui::Pixels>,
    /// A plain click, which moves the cursor only on the prompt's line.
    plain: bool,
}

/// Whether a plain click at a prompt moves the cursor.
fn plain_click_moves() -> bool {
    static MOVES: ghostty_config::Reloadable<bool> =
        ghostty_config::Reloadable::new(mux_terminal_ghostty::user_cursor_click_to_move);
    MOVES.get()
}

impl MuxApp {
    /// Note an Option-click, or a plain click in the pane that already had
    /// the keys, which moves the cursor if it ends where it started.
    pub(super) fn begin_cursor_click(
        &mut self,
        pane_id: PaneId,
        rect: layout::Rect,
        event: &gpui::MouseDownEvent,
        had_keys: bool,
    ) {
        let modifiers = event.modifiers;
        let plain = !modifiers.modified();
        let clicks = event.button == gpui::MouseButton::Left
            && event.click_count == 1
            && ((modifiers.alt && !modifiers.shift && !modifiers.platform)
                || (plain && had_keys && plain_click_moves()));
        self.cursor_click = clicks.then_some(CursorClick {
            pane_id,
            rect,
            at: event.position,
            plain,
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
        let point = if click.plain {
            if pane.program.is_some() {
                return;
            }
            let Some(point) = self.on_prompt_line(click.pane_id, &frame, point) else {
                return;
            };
            point
        } else {
            point
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

impl MuxApp {
    /// Where a plain click on the line being typed at the prompt walks the
    /// cursor to: the clicked cell, kept within what is typed. None for a
    /// click anywhere else, or when the shell sends no prompt marks.
    fn on_prompt_line(
        &self,
        pane_id: PaneId,
        frame: &RenderFrame,
        point: TerminalPoint,
    ) -> Option<TerminalPoint> {
        let cursor = frame.cursor.as_ref().filter(|cursor| cursor.visible)?;
        if frame.scroll.is_scrolled() {
            return None;
        }
        let top = usize::try_from(frame.scroll.offset).ok()?;
        let prompt = *self.cached_prompt_rows(pane_id)?.last()?;
        let cursor_row = top + usize::from(cursor.y);
        if prompt > cursor_row {
            return None;
        }
        let pane = self.panes.get(&pane_id)?;
        let lines = pane
            .with_engine(|engine| {
                (prompt..top + usize::from(frame.rows))
                    .map(|row| engine.row_text(row))
                    .collect::<Result<Vec<_>, _>>()
            })
            .ok()?;
        let typed = typed_span(&lines, cursor_row - prompt, usize::from(cursor.x))?;
        let row = (top + usize::from(point.row)).checked_sub(prompt)?;
        let (row, column) = clamp_to(typed, (row, usize::from(point.column)))?;
        Some(TerminalPoint {
            row: u16::try_from(prompt + row - top).ok()?,
            column: u16::try_from(column).ok()?,
        })
    }
}

/// Where the typed text at a prompt starts and ends, as (row, column) from
/// the prompt's first row: from just after the prompt's `$ ` or `❯ ` (or
/// the start of the line, if there is none) to just after its last
/// character, or the cursor if that is further on.
type Span = ((usize, usize), (usize, usize));

fn typed_span(lines: &[String], cursor_row: usize, cursor_column: usize) -> Option<Span> {
    let start = lines[..=cursor_row.min(lines.len().checked_sub(1)?)]
        .iter()
        .enumerate()
        .find_map(|(row, line)| prompt_end(line).map(|column| (row, column)))
        .unwrap_or((0, 0));
    let last = lines
        .iter()
        .enumerate()
        .skip(cursor_row)
        .filter_map(|(row, line)| {
            let width = line.trim_end().chars().count();
            (width > 0).then_some((row, width))
        })
        .next_back()
        .unwrap_or((cursor_row, cursor_column));
    let end = last.max((cursor_row, cursor_column));
    Some((start, end))
}

/// The column after a prompt's last mark, if the line has one.
fn prompt_end(line: &str) -> Option<usize> {
    ["$ ", "# ", "% ", "> ", "❯ "]
        .iter()
        .filter_map(|mark| line.find(mark).map(|at| at + mark.len()))
        .min()
        .map(|end| line[..end].chars().count())
}

/// The clicked (row, column), moved into the typed span when it is on the
/// span's rows. None for a click on a row outside them.
fn clamp_to(span: Span, click: (usize, usize)) -> Option<(usize, usize)> {
    let (start, end) = span;
    if click.0 > end.0 {
        return None;
    }
    Some(click.clamp(start, end.max(start)))
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

    use super::{clamp_to, cursor_steps, typed_span};

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

    #[test]
    fn a_plain_click_keeps_to_what_is_typed() {
        let lines = |rows: &[&str]| rows.iter().map(|row| (*row).to_owned()).collect::<Vec<_>>();
        let screen = lines(&["~/src", "❯ git status", "", ""]);
        let span = typed_span(&screen, 1, 12).unwrap();
        assert_eq!(span, ((1, 2), (1, 12)));
        assert_eq!(clamp_to(span, (1, 6)), Some((1, 6)));
        assert_eq!(clamp_to(span, (1, 0)), Some((1, 2)), "not into the prompt");
        assert_eq!(clamp_to(span, (1, 40)), Some((1, 12)), "not past the end");
        assert_eq!(
            clamp_to(span, (0, 3)),
            Some((1, 2)),
            "the prompt's first line"
        );
        assert_eq!(clamp_to(span, (2, 3)), None, "below what is typed");
        let wrapped = lines(&["$ echo aaaa", "bbbb", ""]);
        let span = typed_span(&wrapped, 0, 6).unwrap();
        assert_eq!(span, ((0, 2), (1, 4)), "on to the end of a wrapped line");
        assert_eq!(clamp_to(span, (1, 9)), Some((1, 4)));
    }
}
