//! Quick select: the URLs, paths, hashes and addresses on a pane's screen,
//! each under a key or two in the tab's ink while the rest of the screen
//! falls back. Typing a target's keys copies it; typing them with Shift puts
//! it at the prompt instead.

use super::gpui_terminal::{MarkSpan, TerminalMarks};
use super::links::{spans_of, url_spans};
use super::*;

/// The keys labels are made of, easiest first: the home row, then the rows
/// above and below it.
const LABEL_KEYS: &str = "asdfjklghqweruiopzxcvnmtyb";

/// How long a copied target stays lit once the labels have gone.
const CHOSEN_FLASH: Duration = Duration::from_millis(280);

/// A row of the screen, the first column a target covers on it, and the
/// column after its last.
type Span = (u16, u16, u16);

pub(super) struct QuickSelect {
    pane_id: PaneId,
    /// The screen the targets were read from, to read them again when it
    /// changes under the labels.
    frame: Rc<RenderFrame>,
    targets: Vec<Target>,
    typed: String,
}

/// One place a target appears. The same text appearing twice is two
/// targets under the same keys.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Target {
    text: String,
    spans: Vec<Span>,
    label: String,
}

/// A copied target, lit for a moment after the labels go.
pub(super) struct ChosenTarget {
    pane_id: PaneId,
    spans: Vec<Span>,
    lit: Instant,
}

/// Every target on a screen with the cells it covers: OSC 8 hyperlinks by
/// where they go, then what is written out in the text.
fn targets_in(frame: &RenderFrame) -> Vec<(String, Vec<Span>)> {
    let mut targets = hyperlinks_in(frame);
    let taken = targets
        .iter()
        .flat_map(|(_, spans)| spans.iter())
        .flat_map(|&(row, start, end)| (start..end).map(move |column| (row, column)))
        .collect::<HashSet<_>>();
    for line in logical_lines(frame) {
        let text = line
            .iter()
            .map(|(character, ..)| *character)
            .collect::<Vec<_>>();
        for (start, end) in matches_in(&text) {
            let cells = line[start..end]
                .iter()
                .map(|(_, row, column)| (*row, *column));
            if cells.clone().any(|cell| taken.contains(&cell)) {
                continue;
            }
            targets.push((text[start..end].iter().collect(), spans_of(cells)));
        }
    }
    targets
}

/// The runs of cells that each carry one OSC 8 target.
fn hyperlinks_in(frame: &RenderFrame) -> Vec<(String, Vec<Span>)> {
    let columns = usize::from(frame.cols).max(1);
    let mut found = Vec::new();
    let mut index = 0;
    while index < frame.cells.len() {
        let Some(url) = frame.cells[index].hyperlink.clone() else {
            index += 1;
            continue;
        };
        let start = index;
        while index < frame.cells.len() && frame.cells[index].hyperlink.as_deref() == Some(&url) {
            index += 1;
        }
        let cells = (start..index).map(|cell| ((cell / columns) as u16, (cell % columns) as u16));
        found.push((url, spans_of(cells)));
    }
    found
}

/// The screen's text a line at a time, a line running on across the rows
/// the terminal wrapped, each character with the cell it sits in.
fn logical_lines(frame: &RenderFrame) -> Vec<Vec<(char, u16, u16)>> {
    let columns = usize::from(frame.cols);
    let mut lines = Vec::new();
    let mut line = Vec::new();
    for row in 0..frame.rows {
        for column in 0..frame.cols {
            let Some(cell) = frame
                .cells
                .get(usize::from(row) * columns + usize::from(column))
            else {
                continue;
            };
            if matches!(cell.width, CellWidth::SpacerTail | CellWidth::SpacerHead) {
                continue;
            }
            if cell.grapheme.is_empty() {
                line.push((' ', row, column));
            } else {
                line.extend(
                    cell.grapheme
                        .chars()
                        .map(|character| (character, row, column)),
                );
            }
        }
        let wrapped = frame
            .row_metadata
            .get(usize::from(row))
            .is_some_and(|meta| meta.wrapped);
        if !wrapped {
            lines.push(std::mem::take(&mut line));
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// Where targets start and end in a line, as character indexes: URLs, then
/// any word that reads as a path, an address, a UUID or a hash.
fn matches_in(text: &[char]) -> Vec<(usize, usize)> {
    let mut found = url_spans(text);
    let mut start = 0;
    while start < text.len() {
        if !is_word_character(text[start]) {
            start += 1;
            continue;
        }
        let mut end = start;
        while end < text.len() && is_word_character(text[end]) {
            end += 1;
        }
        // Without the punctuation of the sentence it ends.
        let mut last = end;
        while last > start && matches!(text[last - 1], '.' | ':' | '!' | '?') {
            last -= 1;
        }
        let word = text[start..last].iter().collect::<String>();
        let free = !found
            .iter()
            .any(|&(taken_start, taken_end)| start < taken_end && taken_start < last);
        if free && last > start && is_target(&word) {
            found.push((start, last));
        }
        start = end;
    }
    found.sort_unstable();
    found
}

fn is_word_character(character: char) -> bool {
    !character.is_whitespace()
        && !character.is_control()
        && !matches!(
            character,
            '"' | '\''
                | '`'
                | '<'
                | '>'
                | '|'
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | ','
                | ';'
                | '='
        )
}

fn is_target(word: &str) -> bool {
    is_path(word) || is_address(word) || is_uuid(word) || is_hash(word)
}

/// `~/src/mux`, `crates/mux/src/lib.rs:12:5`, `/usr/share`, `Cargo.toml`.
fn is_path(word: &str) -> bool {
    let path = without_line_and_column(word);
    if path.contains("://") || path.starts_with("//") {
        return false;
    }
    if path.contains('/') {
        return path.chars().any(char::is_alphabetic);
    }
    // A file on its own: a name with a letter in it, then a short extension
    // that starts with a letter, so `v1.2`, `0.04s` and `e.g` are not files.
    let Some((name, extension)) = path.rsplit_once('.') else {
        return false;
    };
    let extension_length = extension.chars().count();
    name.chars().any(char::is_alphabetic)
        && name
            .chars()
            .all(|character| character.is_alphanumeric() || "._-+@".contains(character))
        && (1..=5).contains(&extension_length)
        && (extension_length > 1 || name.chars().count() > 1)
        && extension.starts_with(|character: char| character.is_ascii_alphabetic())
        && extension
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
}

/// A path without the line and column a compiler or grep put after it.
fn without_line_and_column(word: &str) -> &str {
    let mut path = word;
    for _ in 0..2 {
        match path.rsplit_once(':') {
            Some((head, tail)) if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) => {
                path = head;
            }
            _ => break,
        }
    }
    path
}

/// `127.0.0.1`, `10.0.0.2:8080`, `localhost:5173`.
fn is_address(word: &str) -> bool {
    let (host, port) = match word.rsplit_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (word, None),
    };
    if port.is_some_and(|port| {
        port.is_empty() || port.len() > 5 || !port.chars().all(|c| c.is_ascii_digit())
    }) {
        return false;
    }
    if host == "localhost" {
        return port.is_some();
    }
    let parts = host.split('.').collect::<Vec<_>>();
    parts.len() == 4
        && parts.iter().all(|part| {
            (1..=3).contains(&part.len())
                && part.chars().all(|c| c.is_ascii_digit())
                && part.parse::<u16>().is_ok_and(|value| value <= 255)
        })
}

fn is_uuid(word: &str) -> bool {
    word.len() == 36
        && word.char_indices().all(|(index, character)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                character == '-'
            } else {
                character.is_ascii_hexdigit()
            }
        })
}

/// A commit or object hash: lowercase hex with both digits and letters, so
/// plain numbers and words are not hashes.
fn is_hash(word: &str) -> bool {
    (7..=64).contains(&word.len())
        && word
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
        && word.chars().any(|c| c.is_ascii_digit())
        && word.chars().any(|c| c.is_ascii_alphabetic())
}

/// Labels for `count` targets, easiest first: a key each while there are
/// few enough of them, two each beyond that.
fn labels(count: usize) -> Vec<String> {
    let keys = LABEL_KEYS.chars().collect::<Vec<_>>();
    if count <= keys.len() {
        keys.iter().take(count).map(char::to_string).collect()
    } else {
        keys.iter()
            .flat_map(|first| keys.iter().map(move |second| format!("{first}{second}")))
            .take(count)
            .collect()
    }
}

/// Give targets their keys. The nearest the bottom of the screen, where the
/// latest output is, get the easiest; the same text gets the same keys
/// wherever it appears; and a text that already had keys keeps them, so the
/// labels hold still while the screen changes under them.
fn labelled(found: Vec<(String, Vec<Span>)>, kept: &[Target]) -> Vec<Target> {
    let mut nearest = found.iter().collect::<Vec<_>>();
    nearest.sort_by_key(|(_, spans)| {
        std::cmp::Reverse(spans.last().map(|&(row, start, _)| (row, start)))
    });
    let mut texts = Vec::new();
    for (text, _) in nearest {
        if !texts.contains(text) {
            texts.push(text.clone());
        }
    }
    let short = texts.len() <= LABEL_KEYS.len();
    let kept = kept
        .iter()
        .filter(|target| short == (target.label.chars().count() == 1))
        .map(|target| (target.text.clone(), target.label.clone()))
        .collect::<HashMap<_, _>>();
    let in_use = texts
        .iter()
        .filter_map(|text| kept.get(text).cloned())
        .collect::<HashSet<_>>();
    let mut free = labels(if short {
        LABEL_KEYS.len()
    } else {
        LABEL_KEYS.len() * LABEL_KEYS.len()
    })
    .into_iter()
    .filter(|label| !in_use.contains(label));
    let mut given = HashMap::new();
    for text in texts {
        let label = kept.get(&text).cloned().or_else(|| free.next());
        if let Some(label) = label {
            given.insert(text, label);
        }
    }
    found
        .into_iter()
        .filter_map(|(text, spans)| {
            let label = given.get(&text)?.clone();
            Some(Target { text, spans, label })
        })
        .collect()
}

impl MuxApp {
    /// Open quick select over the pane with the keys, or close it.
    pub(super) fn toggle_quick_select(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.quick_select.take().is_some() {
            cx.notify();
            return;
        }
        let Some(pane_id) = self.terminal_input_pane_id() else {
            return;
        };
        if self.active_agent_pane() == Some(pane_id) {
            return;
        }
        let Some(pane) = self.panes.get(&pane_id) else {
            return;
        };
        let targets = labelled(targets_in(&pane.frame), &[]);
        if targets.is_empty() {
            self.say_in_strip("nothing on screen to select", false, cx);
            return;
        }
        let frame = Rc::clone(&pane.frame);
        if self.find_has_keyboard(window, cx) {
            self.close_find(window, cx);
        }
        self.quick_select = Some(QuickSelect {
            pane_id,
            frame,
            targets,
            typed: String::new(),
        });
        cx.notify();
    }

    /// Keep the labels on what they label as output moves it, and close
    /// quick select when its pane loses the keys.
    pub(super) fn refresh_quick_select(&mut self, window: &Window, cx: &App) {
        let Some(select) = self.quick_select.as_ref() else {
            return;
        };
        let pane_id = select.pane_id;
        if self.terminal_input_pane_id() != Some(pane_id)
            || self.field_has_keyboard()
            || self.find_has_keyboard(window, cx)
        {
            self.quick_select = None;
            return;
        }
        let Some(pane) = self.panes.get(&pane_id) else {
            self.quick_select = None;
            return;
        };
        let Some(select) = self.quick_select.as_mut() else {
            return;
        };
        if Rc::ptr_eq(&select.frame, &pane.frame) {
            return;
        }
        select.frame = Rc::clone(&pane.frame);
        select.targets = labelled(targets_in(&pane.frame), &select.targets);
        if !select
            .targets
            .iter()
            .any(|target| target.label.starts_with(&select.typed))
        {
            select.typed.clear();
        }
    }

    /// A key while quick select is open. Returns whether quick select took
    /// it; a key with ⌘, ⌃ or ⌥ closes it and goes on to do what it does.
    pub(super) fn quick_select_key(
        &mut self,
        keystroke: &gpui::Keystroke,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(select) = self.quick_select.as_mut() else {
            return false;
        };
        let modifiers = keystroke.modifiers;
        if modifiers.platform || modifiers.control || modifiers.alt {
            self.quick_select = None;
            cx.notify();
            return false;
        }
        match keystroke.key.as_str() {
            "escape" => {
                self.quick_select = None;
                cx.notify();
                return true;
            }
            "backspace" => {
                if select.typed.pop().is_none() {
                    self.quick_select = None;
                }
                cx.notify();
                return true;
            }
            _ => {}
        }
        let mut characters = keystroke.key.chars();
        let (Some(key), None) = (characters.next(), characters.next()) else {
            return true;
        };
        let typed = format!("{}{}", select.typed, key.to_ascii_lowercase());
        let chosen = select
            .targets
            .iter()
            .filter(|target| target.label == typed)
            .collect::<Vec<_>>();
        if let Some(first) = chosen.first() {
            let text = first.text.clone();
            let spans = chosen
                .iter()
                .flat_map(|target| target.spans.iter().copied())
                .collect();
            let pane_id = select.pane_id;
            self.quick_select = None;
            self.use_target(pane_id, &text, spans, modifiers.shift, cx);
        } else if select
            .targets
            .iter()
            .any(|target| target.label.starts_with(&typed))
        {
            select.typed = typed;
        }
        cx.notify();
        true
    }

    /// Copy a chosen target, lighting it for a moment, or with `paste` type
    /// it at the pane's prompt.
    fn use_target(
        &mut self,
        pane_id: PaneId,
        text: &str,
        spans: Vec<Span>,
        paste: bool,
        cx: &mut Context<Self>,
    ) {
        if paste {
            if let Some(pane) = self.panes.get(&pane_id)
                && let Ok(bytes) = pane.engine.encode_paste(text)
            {
                self.backend.send(CommandMessage::Write { pane_id, bytes });
                self.return_to_latest(pane_id, cx);
            }
            return;
        }
        let copied = self
            .clipboard
            .as_mut()
            .is_some_and(|clipboard| clipboard.set_text(text).is_ok());
        if !copied {
            self.say_in_strip("couldn't reach the clipboard", true, cx);
            return;
        }
        self.say_in_strip(format!("copied {text}"), false, cx);
        let lit = Instant::now();
        self.chosen_target = Some(ChosenTarget {
            pane_id,
            spans,
            lit,
        });
        cx.spawn(async move |entity, cx| {
            cx.background_executor().timer(CHOSEN_FLASH).await;
            let _ = entity.update(cx, |this, cx| {
                if this
                    .chosen_target
                    .as_ref()
                    .is_some_and(|chosen| chosen.lit == lit)
                {
                    this.chosen_target = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// What quick select marks on a pane, if anything: its targets washed in
    /// the tab's ink with the rest of the screen faded, or the target just
    /// copied, solid.
    pub(super) fn quick_select_marks(&self, pane_id: PaneId) -> Option<Rc<TerminalMarks>> {
        let ink = self.active_ink().color();
        let marks = |spans: Vec<MarkSpan>, dim: Option<Rgb>| {
            Rc::new(TerminalMarks {
                spans,
                wash: ink.opacity(0.24),
                current: ink,
                current_text: find::rgb_of(GROUND),
                dim,
            })
        };
        let mark = |&(row, start, end): &Span, current: bool| MarkSpan {
            row: usize::from(row),
            start: usize::from(start),
            end: usize::from(end),
            current,
        };
        if let Some(select) = self
            .quick_select
            .as_ref()
            .filter(|select| select.pane_id == pane_id)
        {
            let spans = select
                .targets
                .iter()
                .filter(|target| target.label.starts_with(&select.typed))
                .flat_map(|target| target.spans.iter().map(|span| mark(span, false)))
                .collect();
            return Some(marks(spans, Some(find::rgb_of(SURFACE))));
        }
        let chosen = self
            .chosen_target
            .as_ref()
            .filter(|chosen| chosen.pane_id == pane_id)?;
        Some(marks(
            chosen.spans.iter().map(|span| mark(span, true)).collect(),
            None,
        ))
    }

    /// Each target's keys, solid in the tab's ink over the start of it; the
    /// keys already typed fade.
    pub(super) fn render_quick_select_labels(
        &self,
        pane_id: PaneId,
        rect: layout::Rect,
    ) -> Vec<gpui::AnyElement> {
        let Some(select) = self
            .quick_select
            .as_ref()
            .filter(|select| select.pane_id == pane_id)
        else {
            return Vec::new();
        };
        let Some(pane) = self.panes.get(&pane_id) else {
            return Vec::new();
        };
        let metrics = self.metrics;
        let padding =
            metrics.balanced_padding(rect.width, rect.height, pane.frame.cols, pane.frame.rows);
        let ink = self.active_ink().color();
        let typed = select.typed.chars().count();
        select
            .targets
            .iter()
            .filter(|target| target.label.starts_with(&select.typed))
            .filter_map(|target| {
                let &(row, column, _) = target.spans.first()?;
                let keys = target.label.chars().count();
                let (pressed, remaining): (String, String) = (
                    target.label.chars().take(typed).collect(),
                    target.label.chars().skip(typed).collect(),
                );
                Some(
                    h_flex()
                        .absolute()
                        .left(px(
                            padding.left + f32::from(column) * metrics.cell_width - 1.0
                        ))
                        .top(px(padding.top + f32::from(row) * metrics.cell_height + 1.0))
                        .w(px(keys as f32 * metrics.cell_width + 2.0))
                        .h(px(metrics.cell_height - 2.0))
                        .justify_center()
                        .items_center()
                        .rounded(px(3.0))
                        .bg(ink)
                        .font_family(self.terminal_font.clone())
                        .text_size(px(metrics.font_size))
                        .line_height(px(metrics.cell_height - 2.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(color(GROUND))
                        .when(!pressed.is_empty(), |chip| {
                            chip.child(div().text_color(color(GROUND).opacity(0.45)).child(pressed))
                        })
                        .child(remaining)
                        .into_any_element(),
                )
            })
            .collect()
    }

    pub(super) fn quick_select_open(&self) -> bool {
        self.quick_select.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mux_terminal::{
        CellStyle, RenderCell, RenderDirty, RenderRow, SemanticContent, TerminalScrollState,
    };

    fn frame(rows: &[&str], cols: u16, wrapped: &[bool]) -> RenderFrame {
        let mut cells = Vec::new();
        for row in rows {
            let mut characters = row.chars();
            for _ in 0..cols {
                cells.push(RenderCell {
                    grapheme: characters.next().map(String::from).unwrap_or_default(),
                    foreground: Rgb::default(),
                    background: Rgb::default(),
                    underline_color: Rgb::default(),
                    style: CellStyle::default(),
                    width: CellWidth::Narrow,
                    semantic: SemanticContent::Output,
                    selected: false,
                    hyperlink: None,
                });
            }
        }
        RenderFrame {
            cols,
            rows: rows.len() as u16,
            dirty: RenderDirty::Clean,
            background: Rgb::default(),
            foreground: Rgb::default(),
            cursor: None,
            scroll: TerminalScrollState::default(),
            row_metadata: wrapped
                .iter()
                .map(|wrapped| RenderRow {
                    wrapped: *wrapped,
                    ..RenderRow::default()
                })
                .collect(),
            cells,
        }
    }

    fn texts(frame: &RenderFrame) -> Vec<String> {
        targets_in(frame)
            .into_iter()
            .map(|(text, _)| text)
            .collect()
    }

    #[test]
    fn the_screen_offers_urls_paths_hashes_and_addresses() {
        let screen = frame(
            &[
                "see https://example.com/a. then src/main.rs:12:5:",
                "commit 783c335 on 127.0.0.1:8080 at localhost:5173",
                "(target/debug/deps/mux-0d6c9e5f) v1.2 0.04s e.g ok.",
            ],
            52,
            &[false, false, false],
        );
        assert_eq!(
            texts(&screen),
            [
                "https://example.com/a",
                "src/main.rs:12:5",
                "783c335",
                "127.0.0.1:8080",
                "localhost:5173",
                "target/debug/deps/mux-0d6c9e5f",
            ]
        );
    }

    #[test]
    fn a_path_wrapped_across_rows_is_one_target() {
        let screen = frame(&["at crates/mux-da", "emon/src/lib.rs"], 16, &[true, false]);
        let found = targets_in(&screen);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, "crates/mux-daemon/src/lib.rs");
        assert_eq!(found[0].1, [(0, 3, 16), (1, 0, 15)]);
    }

    #[test]
    fn the_nearest_the_prompt_gets_the_easiest_key_and_twins_share_one() {
        let found = vec![
            ("one.rs".to_owned(), vec![(0, 0, 6)]),
            ("two.rs".to_owned(), vec![(1, 0, 6)]),
            ("one.rs".to_owned(), vec![(2, 0, 6)]),
        ];
        let targets = labelled(found, &[]);
        let label_of = |row: u16| {
            targets
                .iter()
                .find(|target| target.spans[0].0 == row)
                .map(|target| target.label.clone())
        };
        assert_eq!(label_of(2).as_deref(), Some("a"));
        assert_eq!(label_of(0).as_deref(), Some("a"));
        assert_eq!(label_of(1).as_deref(), Some("s"));
    }

    #[test]
    fn labels_hold_still_as_the_screen_changes() {
        let before = labelled(
            vec![
                ("old.rs".to_owned(), vec![(0, 0, 6)]),
                ("kept.rs".to_owned(), vec![(1, 0, 7)]),
            ],
            &[],
        );
        let kept_label = before
            .iter()
            .find(|target| target.text == "kept.rs")
            .map(|target| target.label.clone());
        // The screen scrolled: kept.rs moved up and new.rs arrived below it.
        let after = labelled(
            vec![
                ("kept.rs".to_owned(), vec![(0, 0, 7)]),
                ("new.rs".to_owned(), vec![(1, 0, 6)]),
            ],
            &before,
        );
        let label_of = |text: &str| {
            after
                .iter()
                .find(|target| target.text == text)
                .map(|target| target.label.clone())
        };
        assert_eq!(label_of("kept.rs"), kept_label);
        assert_ne!(label_of("new.rs"), kept_label);
        assert!(label_of("new.rs").is_some());
    }

    #[test]
    fn many_targets_take_two_keys_each() {
        let found = (0..30)
            .map(|row| (format!("file{row}.rs"), vec![(row, 0, 8)]))
            .collect();
        let targets = labelled(found, &[]);
        assert!(
            targets
                .iter()
                .all(|target| target.label.chars().count() == 2)
        );
        let unique = targets
            .iter()
            .map(|target| target.label.as_str())
            .collect::<HashSet<_>>();
        assert_eq!(unique.len(), 30);
        // The nearest the bottom is the easiest to type.
        assert_eq!(targets[29].label, "aa");
    }
}
