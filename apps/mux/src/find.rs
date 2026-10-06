//! Find in a pane.
//!
//! ⌘F puts a field in the focused pane's own head, so finding costs no rows
//! of the terminal. Every place the text appears in the pane's history is
//! marked: the current one solid in the tab's ink, the rest washed in it,
//! with a tick for each on the pane's right edge to say where in the history
//! they are. Search starts from the latest output; Return or ↑ goes to older
//! places and ⇧Return or ↓ to newer ones. Lowercase text finds either case;
//! a capital asks for an exact match. New output is searched as it arrives.

use super::gpui_terminal::{MarkSpan, TerminalMarks};
use super::*;
use mux_terminal::{CellWidth, RenderCell, TerminalScrollState};

/// More places than this are not worth marking one by one; the newest are
/// the ones kept.
const MOST_FOUND: usize = 10_000;

/// New output is searched at most this often, so a busy pane with a long
/// history spends a frame on it now and then rather than every frame.
const FIND_REFRESH: Duration = Duration::from_millis(100);

/// One place the text was found: a row counted from the top of the pane's
/// history, and the characters of that row it covers, end exclusive.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Found {
    row: usize,
    start: usize,
    end: usize,
}

pub(super) struct PaneFind {
    pub(super) pane_id: PaneId,
    input: Entity<InputState>,
    query: String,
    found: Vec<Found>,
    current: Option<usize>,
    /// The text of the row the current place is on, to find it again when
    /// new output moves the rows under it.
    current_line: String,
    /// The pane's output sequence and grid size the places were found at;
    /// new output or a reflow means looking again.
    searched: (u64, u16, u16),
    /// When the places were last found, and whether a look is already due.
    searched_when: Instant,
    refresh_due: bool,
    _subscription: gpui::Subscription,
}

impl PaneFind {
    fn current(&self) -> Option<Found> {
        self.found.get(self.current?).copied()
    }

    /// Look again in `text`. With `keep`, the current place follows its row
    /// as new output moves it; otherwise the search starts over from the row
    /// `from`, the bottom of what is on screen.
    fn search(&mut self, text: &str, keep: bool, from: usize) {
        let lines: Vec<&str> = text.lines().collect();
        let previous = self.current().filter(|_| keep);
        self.found = find_all(&lines, &self.query);
        self.current = match previous {
            Some(previous) => relocate(previous, &self.current_line, &self.found, &lines),
            None => nearest_at_or_above(&self.found, from),
        };
        self.current_line = self
            .current()
            .and_then(|found| lines.get(found.row))
            .map_or_else(String::new, |line| (*line).to_owned());
    }
}

impl MuxApp {
    /// Open find on the focused terminal pane, or put the keys back in its
    /// field, text selected, when it is already open there.
    pub(super) fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pane_id) = self
            .terminal_input_pane_id()
            .filter(|pane_id| self.active_agent_pane() != Some(*pane_id))
        else {
            return;
        };
        if let Some(find) = self.find.as_ref().filter(|find| find.pane_id == pane_id) {
            find.input.update(cx, |input, cx| {
                input.focus(window, cx);
                input.select_all(window, cx);
            });
            return;
        }
        self.finish_tab_rename(true, window, cx);
        self.palette = None;
        self.session_sheet = None;
        self.settings_sheet = None;
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("find"));
        let subscription =
            cx.subscribe_in(&input, window, |this, input, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    let query = input.read(cx).value().to_string();
                    this.find_typed(query, cx);
                }
            });
        let focus = input.clone();
        window.on_next_frame(move |window, cx| {
            focus.update(cx, |input, cx| input.focus(window, cx));
        });
        self.find = Some(PaneFind {
            pane_id,
            input,
            query: String::new(),
            found: Vec::new(),
            current: None,
            current_line: String::new(),
            searched: (0, 0, 0),
            searched_when: Instant::now(),
            refresh_due: false,
            _subscription: subscription,
        });
        self.mode = InputMode::Normal;
        cx.notify();
    }

    pub(super) fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.take().is_some() {
            self.restore_keyboard(window, cx);
            cx.notify();
        }
    }

    /// Whether the find field holds the keys, so typing goes to it.
    pub(super) fn find_has_keyboard(&self, window: &Window, cx: &App) -> bool {
        self.find.as_ref().is_some_and(|find| {
            gpui::Focusable::focus_handle(find.input.read(cx), cx).is_focused(window)
        })
    }

    fn find_typed(&mut self, query: String, cx: &mut Context<Self>) {
        let Some(find) = self.find.as_mut() else {
            return;
        };
        let Some(pane) = self.panes.get(&find.pane_id) else {
            return;
        };
        find.query = query;
        let scroll = pane.frame.scroll;
        let bottom = usize::try_from(scroll.offset + scroll.len.max(1) - 1).unwrap_or(usize::MAX);
        match pane.engine.screen_text() {
            Ok(text) => find.search(&text, false, bottom),
            Err(error) => {
                error!(pane_id = %find.pane_id, %error, "could not read the pane to find in it");
            }
        }
        find.searched = searched_at(pane);
        find.searched_when = Instant::now();
        self.reveal_found();
        cx.notify();
    }

    /// Go to the next place, older or newer, coming round at either end.
    pub(super) fn step_find(&mut self, older: bool, cx: &mut Context<Self>) {
        let Some(find) = self.find.as_mut() else {
            return;
        };
        let count = find.found.len();
        if count == 0 {
            return;
        }
        let next = match find.current {
            Some(index) if older => (index + count - 1) % count,
            Some(index) => (index + 1) % count,
            None if older => count - 1,
            None => 0,
        };
        find.current = Some(next);
        if let Some(pane) = self.panes.get(&find.pane_id)
            && let Ok(text) = pane.engine.screen_text()
        {
            text.lines()
                .nth(find.found[next].row)
                .unwrap_or_default()
                .clone_into(&mut find.current_line);
        }
        self.reveal_found();
        cx.notify();
    }

    /// Scroll the current place into view when it is out of it.
    fn reveal_found(&mut self) {
        let Some(find) = &self.find else {
            return;
        };
        let pane_id = find.pane_id;
        let Some(found) = find.current() else {
            return;
        };
        let Some(pane) = self.panes.get(&pane_id) else {
            return;
        };
        let scroll = pane.frame.scroll;
        if let Some(offset) = reveal_offset(found.row, scroll) {
            let delta = i64::try_from(offset).unwrap_or(i64::MAX)
                - i64::try_from(scroll.offset).unwrap_or(i64::MAX);
            self.scroll_viewport(pane_id, TerminalViewportScroll::Delta(delta));
        }
    }

    /// Search new output as it arrives, keeping the current place, and let
    /// find go when its pane does.
    pub(super) fn refresh_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(find) = self.find.as_mut() else {
            return;
        };
        let Some(pane) = self.panes.get(&find.pane_id) else {
            self.close_find(window, cx);
            return;
        };
        let searched = searched_at(pane);
        if searched == find.searched || find.query.is_empty() {
            find.searched = searched;
            return;
        }
        // A reflow moves every row, so it is looked at straight away; output
        // that only adds rows waits its turn, and one look covers it all.
        let reflowed = searched.1 != find.searched.1 || searched.2 != find.searched.2;
        let wait = FIND_REFRESH.saturating_sub(find.searched_when.elapsed());
        if !reflowed && !wait.is_zero() {
            if !find.refresh_due {
                find.refresh_due = true;
                cx.spawn(async move |entity, cx| {
                    cx.background_executor().timer(wait).await;
                    let _ = entity.update(cx, |this, cx| {
                        if let Some(find) = this.find.as_mut() {
                            find.refresh_due = false;
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
            return;
        }
        find.searched = searched;
        find.searched_when = Instant::now();
        if let Ok(text) = pane.engine.screen_text() {
            find.search(&text, true, usize::MAX);
        }
    }

    /// What find has marked in a pane's viewport.
    pub(super) fn find_marks(&self, pane_id: PaneId) -> Rc<TerminalMarks> {
        let Some(find) = self.find.as_ref().filter(|find| find.pane_id == pane_id) else {
            return Rc::default();
        };
        let Some(pane) = self.panes.get(&pane_id) else {
            return Rc::default();
        };
        let frame = &pane.frame;
        let columns = usize::from(frame.cols);
        let rows = usize::from(frame.rows);
        let top = usize::try_from(frame.scroll.offset).unwrap_or(usize::MAX);
        let first = find.found.partition_point(|found| found.row < top);
        let mut spans = Vec::new();
        for (index, found) in find.found.iter().enumerate().skip(first) {
            let row = found.row - top;
            if row >= rows {
                break;
            }
            let cells = &frame.cells[row * columns..(row + 1) * columns];
            if let Some((start, end)) = columns_of(cells, found.start, found.end) {
                spans.push(MarkSpan {
                    row,
                    start,
                    end,
                    current: find.current == Some(index),
                });
            }
        }
        let ink = self.active_ink().color();
        Rc::new(TerminalMarks {
            spans,
            wash: ink.opacity(0.26),
            current: ink,
            current_text: rgb_of(GROUND),
        })
    }

    /// The find field, laid over the right of its pane's head.
    pub(super) fn render_pane_find(
        &self,
        geometry: layout::PaneGeometry,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let find = self
            .find
            .as_ref()
            .filter(|find| find.pane_id == geometry.pane_id)?;
        let frame = geometry.frame;
        let left = (frame.width * 0.4).clamp(96.0, (frame.width - 180.0).max(96.0));
        let width = (frame.width - left - 10.0).max(0.0);
        let (count, tone) = if find.query.is_empty() {
            (String::new(), FAINT_TEXT)
        } else if find.found.is_empty() {
            ("none".to_owned(), FAINT_TEXT)
        } else {
            let more = if find.found.len() >= MOST_FOUND {
                "+"
            } else {
                ""
            };
            let at = find.current.map_or(0, |index| index + 1);
            (format!("{at} of {}{more}", find.found.len()), MUTED_TEXT)
        };
        let roomy = width > 320.0;
        Some(
            h_flex()
                .id("pane-find")
                .key_context("MuxFind")
                .on_action(cx.listener(|this, _: &FindOlder, _, cx| this.step_find(true, cx)))
                .on_action(cx.listener(|this, _: &FindNewer, _, cx| this.step_find(false, cx)))
                .on_action(cx.listener(|this, _: &CloseFind, window, cx| {
                    this.close_find(window, cx);
                }))
                .absolute()
                .left(px(frame.x + left))
                .top(px(frame.y + 1.0))
                .w(px(width))
                .h(px(layout::PANE_HEAD_HEIGHT - 2.0))
                .items_center()
                .gap(px(8.0))
                .pl(px(10.0))
                .bg(color(SURFACE))
                .font_family(EMBEDDED_TERMINAL_FONT)
                .text_size(px(12.0))
                .whitespace_nowrap()
                .child(
                    div()
                        .flex_none()
                        .font_weight(FontWeight::BOLD)
                        .text_color(self.active_ink().color())
                        .child("›"),
                )
                .child(
                    div().flex_1().min_w_0().child(
                        Input::new(&find.input)
                            .appearance(false)
                            .xsmall()
                            .font_family(EMBEDDED_TERMINAL_FONT)
                            .text_size(px(12.0))
                            .text_color(color(TEXT)),
                    ),
                )
                .child(div().flex_none().text_color(color(tone)).child(count))
                .when(roomy, |row| {
                    row.child(kbd("↑↓", color(MUTED_TEXT)))
                        .child(kbd("esc", color(MUTED_TEXT)))
                })
                .into_any_element(),
        )
    }

    /// A tick on the pane's right edge for each place found, at its height
    /// in the history; the current one in the tab's ink.
    pub(super) fn render_find_ticks(
        &self,
        geometry: layout::PaneGeometry,
    ) -> Option<gpui::AnyElement> {
        let find = self
            .find
            .as_ref()
            .filter(|find| find.pane_id == geometry.pane_id)?;
        if find.found.is_empty() {
            return None;
        }
        let total = self.panes.get(&geometry.pane_id)?.frame.scroll.total.max(1) as f32;
        let rect = geometry.rect;
        let inset = 6.0;
        let track = rect.height - inset * 2.0;
        let tick = |row: usize, tone: Hsla, height: f32| {
            let y = (row as f32 + 0.5) / total * track;
            div()
                .absolute()
                .top(px((y - height / 2.0).clamp(0.0, track - height)))
                .left_0()
                .w(px(6.0))
                .h(px(height))
                .rounded_full()
                .bg(tone)
        };
        let mut ticks = div()
            .absolute()
            .left(px(rect.x + rect.width - 8.5))
            .top(px(rect.y + inset))
            .w(px(6.0))
            .h(px(track));
        // One tick to a pixel row is all the edge can show.
        let mut last = f32::NEG_INFINITY;
        for found in &find.found {
            let y = (found.row as f32 + 0.5) / total * track;
            if y - last >= 2.0 {
                last = y;
                ticks = ticks.child(tick(found.row, color(MUTED_TEXT).opacity(0.7), 2.0));
            }
        }
        if let Some(found) = find.current() {
            ticks = ticks.child(tick(found.row, self.active_ink().color(), 3.0));
        }
        Some(ticks.into_any_element())
    }
}

/// Every place `query` appears in `lines`, top first, up to the newest
/// `MOST_FOUND`. Lowercase text finds either case; any capital makes the
/// search exact.
fn find_all(lines: &[&str], query: &str) -> Vec<Found> {
    let fold = !query.chars().any(char::is_uppercase);
    let needle: Vec<char> = query.chars().map(|c| folded(c, fold)).collect();
    let mut found = Vec::new();
    if needle.is_empty() {
        return found;
    }
    for (row, line) in lines.iter().enumerate() {
        let hay: Vec<char> = line.chars().map(|c| folded(c, fold)).collect();
        let mut start = 0;
        while start + needle.len() <= hay.len() {
            if hay[start..start + needle.len()] == needle[..] {
                found.push(Found {
                    row,
                    start,
                    end: start + needle.len(),
                });
                start += needle.len();
            } else {
                start += 1;
            }
        }
    }
    let over = found.len().saturating_sub(MOST_FOUND);
    found.drain(..over);
    found
}

/// One character for one, so positions in folded text are positions in the
/// original.
fn folded(c: char, fold: bool) -> char {
    if fold {
        c.to_lowercase().next().unwrap_or(c)
    } else {
        c
    }
}

/// The place to make current when a search starts over: the last one at or
/// above `row`, else the first below it.
fn nearest_at_or_above(found: &[Found], row: usize) -> Option<usize> {
    match found.partition_point(|found| found.row <= row) {
        0 if found.is_empty() => None,
        0 => Some(0),
        after => Some(after - 1),
    }
}

/// Where the current place went after new output: the same text at the same
/// columns on the nearest row that still reads the same, else the nearest
/// place of all.
fn relocate(previous: Found, line: &str, found: &[Found], lines: &[&str]) -> Option<usize> {
    let distance = |candidate: &Found| candidate.row.abs_diff(previous.row);
    found
        .iter()
        .enumerate()
        .filter(|(_, candidate)| {
            candidate.start == previous.start && lines.get(candidate.row) == Some(&line)
        })
        .min_by_key(|(_, candidate)| distance(candidate))
        .or_else(|| {
            found
                .iter()
                .enumerate()
                .min_by_key(|(_, candidate)| distance(candidate))
        })
        .map(|(index, _)| index)
}

/// The viewport offset that brings `row` into view a third of the way down,
/// or `None` when it is already in view.
fn reveal_offset(row: usize, scroll: TerminalScrollState) -> Option<usize> {
    let row = row as u64;
    if row >= scroll.offset && row < scroll.offset + scroll.len {
        return None;
    }
    let lowest = scroll.total.saturating_sub(scroll.len);
    let offset = row.saturating_sub(scroll.len / 3).min(lowest);
    usize::try_from(offset).ok()
}

/// The columns a run of characters of a row covers, end exclusive. Wide
/// characters take two columns, and a cell with nothing in it is one space,
/// as the row reads in text.
fn columns_of(cells: &[RenderCell], start: usize, end: usize) -> Option<(usize, usize)> {
    let mut seen = 0;
    let mut first = None;
    for (column, cell) in cells.iter().enumerate() {
        if matches!(cell.width, CellWidth::SpacerTail | CellWidth::SpacerHead) {
            continue;
        }
        let count = cell.grapheme.chars().count().max(1);
        if first.is_none() && seen + count > start {
            first = Some(column);
        }
        if seen + count >= end {
            let width = if cell.width == CellWidth::Wide { 2 } else { 1 };
            return first.map(|first| (first, column + width));
        }
        seen += count;
    }
    first.map(|first| (first, cells.len()))
}

fn searched_at(pane: &PaneReplica) -> (u64, u16, u16) {
    (
        pane.engine.next_output_sequence(),
        pane.frame.cols,
        pane.frame.rows,
    )
}

fn rgb_of(value: u32) -> Rgb {
    let [_, r, g, b] = value.to_be_bytes();
    Rgb { r, g, b }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(row: usize, start: usize, end: usize) -> Found {
        Found { row, start, end }
    }

    #[test]
    fn lowercase_finds_either_case_and_a_capital_asks_for_it() {
        let lines = ["Error: one error", "no ERRORs", "fine"];
        assert_eq!(
            find_all(&lines, "error"),
            vec![at(0, 0, 5), at(0, 11, 16), at(1, 3, 8)]
        );
        assert_eq!(find_all(&lines, "ERROR"), vec![at(1, 3, 8)]);
        assert!(find_all(&lines, "").is_empty());
    }

    #[test]
    fn places_do_not_overlap_and_count_characters_not_bytes() {
        assert_eq!(find_all(&["aaaa"], "aa"), vec![at(0, 0, 2), at(0, 2, 4)]);
        assert_eq!(find_all(&["→ héllo"], "llo"), vec![at(0, 4, 7)]);
    }

    #[test]
    fn a_fresh_search_starts_at_the_bottom_of_the_screen_and_looks_up() {
        let found = [at(2, 0, 1), at(9, 0, 1), at(30, 0, 1)];
        assert_eq!(nearest_at_or_above(&found, 20), Some(1));
        assert_eq!(nearest_at_or_above(&found, 9), Some(1));
        assert_eq!(nearest_at_or_above(&found, 1), Some(0));
        assert_eq!(nearest_at_or_above(&[], 1), None);
    }

    #[test]
    fn the_current_place_follows_its_row_when_output_moves_it() {
        let lines = ["x", "warn a", "x", "warn b", "x"];
        let found = find_all(&lines, "warn");
        // "warn b" was row 5 before two rows left the top of the history.
        assert_eq!(relocate(at(5, 0, 4), "warn b", &found, &lines), Some(1));
        // Its row has gone: the nearest place stands in.
        assert_eq!(relocate(at(1, 0, 4), "warn z", &found, &lines), Some(0));
    }

    #[test]
    fn reveals_a_place_out_of_view_a_third_of_the_way_down() {
        let scroll = TerminalScrollState {
            total: 100,
            offset: 70,
            len: 30,
        };
        assert_eq!(reveal_offset(80, scroll), None);
        assert_eq!(reveal_offset(40, scroll), Some(30));
        assert_eq!(reveal_offset(5, scroll), Some(0));
    }

    fn cell(grapheme: &str, width: CellWidth) -> RenderCell {
        RenderCell {
            grapheme: grapheme.to_owned(),
            foreground: Rgb::default(),
            background: Rgb::default(),
            underline_color: Rgb::default(),
            style: mux_terminal::CellStyle::default(),
            width,
            semantic: mux_terminal::SemanticContent::Output,
            selected: false,
            hyperlink: None,
        }
    }

    #[test]
    fn wide_characters_and_blank_cells_land_on_their_columns() {
        let cells = [
            cell("中", CellWidth::Wide),
            cell("", CellWidth::SpacerTail),
            cell("", CellWidth::Narrow),
            cell("a", CellWidth::Narrow),
            cell("b", CellWidth::Narrow),
        ];
        // "中 ab": the wide character, a blank, then "ab".
        assert_eq!(columns_of(&cells, 0, 1), Some((0, 2)));
        assert_eq!(columns_of(&cells, 2, 4), Some((3, 5)));
        assert_eq!(columns_of(&cells, 9, 10), None);
    }
}
