//! Shift-click extends a selection to where you click, as it does in every
//! Mac text view: from the selection's far end, so clicking past its end
//! grows it forward and clicking before its start grows it back. Select all
//! in the pane menu selects everything a pane holds, its history too, and
//! the palette can select what the last command printed.

use mux_terminal::{RenderFrame, TerminalInteraction as _, TerminalPoint, TerminalSelection};

use super::{Context, MuxApp, PaneId, TerminalViewportScroll, layout};

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

    /// Select what the last command printed, between the prompt it was run
    /// from and the next, as iTerm's Cmd+Shift+A does (that is the agent here), and bring its start
    /// into view. A command still running gives what it has printed so far.
    pub(super) fn select_last_output(&mut self, cx: &mut Context<Self>) {
        let Some(pane_id) = self.terminal_input_pane_id() else {
            return;
        };
        let Some(pane) = self.panes.get(&pane_id) else {
            return;
        };
        let prompts = pane
            .with_engine(|engine| engine.prompt_rows())
            .unwrap_or_default();
        if prompts.is_empty() {
            self.say_in_strip("this shell marks no prompts to find output by", true, cx);
            return;
        }
        let text = pane
            .with_engine(|engine| engine.screen_text())
            .unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        let Some((first, last)) = last_output_rows(&prompts, &lines) else {
            self.say_in_strip("the last command printed nothing", false, cx);
            return;
        };
        self.select_output_rows(pane_id, first, last, cx);
    }

    /// Put what the last command printed on the clipboard, without
    /// selecting it.
    pub(super) fn copy_last_output(&mut self, cx: &mut Context<Self>) {
        let Some(pane_id) = self.terminal_input_pane_id() else {
            return;
        };
        let Some(pane) = self.panes.get(&pane_id) else {
            return;
        };
        let prompts = pane
            .with_engine(|engine| engine.prompt_rows())
            .unwrap_or_default();
        let text = pane
            .with_engine(|engine| engine.screen_text())
            .unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        let Some((first, last)) = last_output_rows(&prompts, &lines) else {
            self.say_in_strip("the last command printed nothing", false, cx);
            return;
        };
        let output = lines[first..=last]
            .iter()
            .map(|line| line.trim_end())
            .collect::<Vec<_>>()
            .join("\n");
        let count = last - first + 1;
        if let Some(clipboard) = &mut self.clipboard
            && clipboard.set_text(output).is_ok()
        {
            let copied = if count == 1 {
                "copied 1 line of output".to_owned()
            } else {
                format!("copied {count} lines of output")
            };
            self.say_in_strip(copied, false, cx);
        }
    }

    /// Select what the command run from the prompt at history row `prompt`
    /// printed, as a click on its failure mark does.
    pub(super) fn select_output_of(
        &mut self,
        pane_id: PaneId,
        prompt: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(pane) = self.panes.get(&pane_id) else {
            return;
        };
        let prompts = pane
            .with_engine(|engine| engine.prompt_rows())
            .unwrap_or_default();
        let text = pane
            .with_engine(|engine| engine.screen_text())
            .unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        let Some((first, last)) = super::scrollback::output_rows(&prompts, prompt, &lines) else {
            self.say_in_strip("that command printed nothing", false, cx);
            return;
        };
        self.select_output_rows(pane_id, first, last, cx);
    }

    /// Select history rows `first` to `last` of a pane, bring the first into
    /// view, and say how much is selected.
    fn select_output_rows(
        &mut self,
        pane_id: PaneId,
        first: usize,
        last: usize,
        cx: &mut Context<Self>,
    ) {
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
            .with_engine(|engine| engine.select_rows(first, last))
            .is_err()
        {
            return;
        }
        let _ = pane.publish_frame();
        self.selected_pane = Some(pane_id);
        let scroll = pane.frame.scroll;
        let top = usize::try_from(scroll.offset).unwrap_or(usize::MAX);
        let shown = usize::try_from(scroll.len).unwrap_or(usize::MAX);
        if first < top || first >= top.saturating_add(shown) {
            let delta =
                i64::try_from(first).unwrap_or(i64::MAX) - i64::try_from(top).unwrap_or(i64::MAX);
            self.scroll_viewport(pane_id, TerminalViewportScroll::Delta(delta));
        }
        let count = last - first + 1;
        let lines = if count == 1 {
            "1 line".to_owned()
        } else {
            format!("{count} lines")
        };
        self.say_in_strip(format!("{lines} of output selected · ⌘C copies"), false, cx);
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

/// The rows the last command printed: after the prompt it was run from, up
/// to the next prompt, or to the end while it runs. Blank rows at the end
/// are left out. `None` when it printed nothing.
fn last_output_rows(prompts: &[usize], lines: &[&str]) -> Option<(usize, usize)> {
    let &newest = prompts.last()?;
    let blank_after = |row: usize| {
        lines
            .iter()
            .skip(row + 1)
            .all(|line| line.trim().is_empty())
    };
    // At a prompt, the last command is the one run from the prompt before.
    let (from, to) = if blank_after(newest) {
        let before = *prompts.iter().rev().nth(1)?;
        (before, newest)
    } else {
        (newest, lines.len())
    };
    let first = from + 1;
    let last = (first..to)
        .rev()
        .find(|&row| lines.get(row).is_some_and(|line| !line.trim().is_empty()))?;
    Some((first, last))
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
            images: Vec::new(),
            held: false,
            redraw_in: None,
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

    #[test]
    fn the_last_output_lies_between_the_last_two_prompts() {
        let lines = ["$ ls", "a", "b", "", "$ make", "built", "", "", "$ "];
        assert_eq!(super::last_output_rows(&[0, 4, 8], &lines), Some((5, 5)));
        let blank_tail = ["$ ls", "a", "b", "", "$ "];
        assert_eq!(super::last_output_rows(&[0, 4], &blank_tail), Some((1, 2)));
        let running = ["$ ls", "a", "$ tail -f log", "one", "two"];
        assert_eq!(super::last_output_rows(&[0, 2], &running), Some((3, 4)));
        let quiet = ["$ true", "$ "];
        assert_eq!(super::last_output_rows(&[0, 1], &quiet), None);
        assert_eq!(super::last_output_rows(&[1], &["", "$ "]), None);
    }
}
