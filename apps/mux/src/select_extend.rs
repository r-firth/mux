//! Shift-click extends a selection to where you click, as it does in every
//! Mac text view: from the selection's far end, so clicking past its end
//! grows it forward and clicking before its start grows it back. Cmd+A
//! selects everything a pane holds, its history too.

use mux_terminal::{RenderFrame, TerminalInteraction as _, TerminalPoint, TerminalSelection};

use super::{MuxApp, PaneId, layout};

impl MuxApp {
    /// Select all of the pane with the keys, history and screen, ready for
    /// Cmd+C.
    pub(super) fn select_all(&mut self) {
        let Some(pane_id) = self.terminal_input_pane_id() else {
            return;
        };
        if self.selected_pane != Some(pane_id)
            && let Some(previous) = self.selected_pane.take()
            && let Some(pane) = self.panes.get_mut(&previous)
        {
            let _ = pane.with_engine(|engine| engine.set_selection(None));
            let _ = pane.publish_frame();
        }
        let Some(pane) = self.panes.get_mut(&pane_id) else {
            return;
        };
        if pane
            .with_engine(mux_terminal_ghostty::GhosttyEngine::select_all)
            .is_ok()
        {
            let _ = pane.publish_frame();
            self.selected_pane = Some(pane_id);
        }
    }

    /// Extend the pane's selection to a Shift-click, if it has one on
    /// screen. Returns whether the click was taken.
    pub(super) fn extend_selection(
        &mut self,
        pane_id: PaneId,
        rect: layout::Rect,
        event: &gpui::MouseDownEvent,
    ) -> bool {
        if !event.modifiers.shift
            || event.modifiers.alt
            || event.modifiers.platform
            || event.click_count != 1
            || self.selected_pane != Some(pane_id)
        {
            return false;
        }
        let Some(frame) = self
            .panes
            .get(&pane_id)
            .map(|pane| std::rc::Rc::clone(&pane.frame))
        else {
            return false;
        };
        let Some(click) = self.selection_pointer(rect, &frame, event.position).point else {
            return false;
        };
        let Some(selection) = extended_selection(&frame, click) else {
            return false;
        };
        let Some(pane) = self.panes.get_mut(&pane_id) else {
            return false;
        };
        if pane
            .with_engine(|engine| engine.set_selection(Some(selection)))
            .is_err()
        {
            return false;
        }
        let _ = pane.publish_frame();
        true
    }
}

/// The selection on screen in `frame`, reaching to `click` from whichever
/// of its ends is further from it. None when nothing on screen is selected.
fn extended_selection(frame: &RenderFrame, click: TerminalPoint) -> Option<TerminalSelection> {
    let columns = usize::from(frame.cols.max(1));
    let point = |index: usize| TerminalPoint {
        column: u16::try_from(index % columns).unwrap_or(u16::MAX),
        row: u16::try_from(index / columns).unwrap_or(u16::MAX),
    };
    let first = frame.cells.iter().position(|cell| cell.selected)?;
    let last = frame.cells.iter().rposition(|cell| cell.selected)?;
    let at = usize::from(click.row) * columns + usize::from(click.column);
    let anchor = if at < first { last } else { first };
    Some(TerminalSelection {
        anchor: point(anchor),
        focus: click,
        rectangular: false,
    })
}

#[cfg(test)]
mod tests {
    use mux_terminal::{
        CellStyle, CellWidth, RenderCell, RenderDirty, RenderFrame, Rgb, SemanticContent,
        TerminalPoint, TerminalScrollState,
    };

    use super::extended_selection;

    fn frame(selected: std::ops::Range<usize>) -> RenderFrame {
        let cells = (0..40)
            .map(|index| RenderCell {
                grapheme: "x".to_owned(),
                foreground: Rgb::default(),
                background: Rgb::default(),
                underline_color: Rgb::default(),
                style: CellStyle::default(),
                width: CellWidth::Narrow,
                semantic: SemanticContent::Output,
                selected: selected.contains(&index),
                hyperlink: None,
            })
            .collect();
        RenderFrame {
            cols: 10,
            rows: 4,
            dirty: RenderDirty::Full,
            background: Rgb::default(),
            foreground: Rgb::default(),
            cursor: None,
            scroll: TerminalScrollState::default(),
            row_metadata: Vec::new(),
            cells,
        }
    }

    const fn at(column: u16, row: u16) -> TerminalPoint {
        TerminalPoint { column, row }
    }

    #[test]
    fn a_shift_click_grows_the_selection_from_its_far_end() {
        // Cells 12 to 15 selected: row 1, columns 2 to 5.
        let selection = frame(12..16);
        let forward = extended_selection(&selection, at(8, 2)).expect("extended");
        assert_eq!((forward.anchor, forward.focus), (at(2, 1), at(8, 2)));
        let back = extended_selection(&selection, at(0, 0)).expect("extended");
        assert_eq!((back.anchor, back.focus), (at(5, 1), at(0, 0)));
        assert!(!back.rectangular);
    }

    #[test]
    fn nothing_selected_has_nothing_to_extend() {
        assert_eq!(extended_selection(&frame(0..0), at(3, 3)), None);
    }
}
