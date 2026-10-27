//! Links in a pane's output.
//!
//! Holding ⌘ over a link (an OSC 8 hyperlink, or a URL written out in plain
//! text, even one the terminal wrapped across rows) underlines it in the
//! tab's ink and shows where it goes in the strip, as a browser's status line
//! would; ⌘-click opens it. A file link is shown in Finder rather than
//! opened, so a link can never start a program. A path written in the
//! output, such as a compiler's `src/main.rs:12:5`, is a link too: found
//! from the pane's working directory and shown in Finder.

use super::*;
use mux_terminal::RenderCell;
use std::path::Path;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Link {
    pub(super) url: String,
    /// Whether `url` is a path as written in the output, to be found from
    /// the pane's working directory, rather than a URL.
    pub(super) path: bool,
    /// Each grid row the link covers, with its first column and the column
    /// after its last.
    pub(super) spans: Vec<(u16, u16, u16)>,
}

const SCHEMES: [&str; 3] = ["https://", "http://", "file://"];

/// The link under a cell, if there is one.
pub(super) fn link_at(frame: &RenderFrame, point: TerminalPoint) -> Option<Link> {
    hyperlink_at(frame, point).or_else(|| written_url_at(frame, point))
}

fn cell(frame: &RenderFrame, row: u16, column: u16) -> Option<&RenderCell> {
    frame
        .cells
        .get(usize::from(row) * usize::from(frame.cols) + usize::from(column))
}

/// An OSC 8 hyperlink: the run of cells that carry the same target.
fn hyperlink_at(frame: &RenderFrame, point: TerminalPoint) -> Option<Link> {
    let url = cell(frame, point.row, point.column)?.hyperlink.clone()?;
    let columns = usize::from(frame.cols);
    let same = |index: usize| frame.cells[index].hyperlink.as_deref() == Some(url.as_str());
    let at = usize::from(point.row) * columns + usize::from(point.column);
    let mut first = at;
    while first > 0 && same(first - 1) {
        first -= 1;
    }
    let mut last = at;
    while last + 1 < frame.cells.len() && same(last + 1) {
        last += 1;
    }
    let cells = (first..=last).map(|index| ((index / columns) as u16, (index % columns) as u16));
    Some(Link {
        url,
        path: false,
        spans: spans_of(cells),
    })
}

/// A URL written out in the text, read across the rows the terminal wrapped.
fn written_url_at(frame: &RenderFrame, point: TerminalPoint) -> Option<Link> {
    let wrapped = |row: u16| {
        frame
            .row_metadata
            .get(usize::from(row))
            .is_some_and(|meta| meta.wrapped)
    };
    let mut first = point.row;
    while first > 0 && wrapped(first - 1) {
        first -= 1;
    }
    let mut last = point.row;
    while last + 1 < frame.rows && wrapped(last) {
        last += 1;
    }
    // Each character of the logical line, and the cell it sits in.
    let mut characters = Vec::new();
    let mut hit = None;
    for row in first..=last {
        for column in 0..frame.cols {
            let cell = cell(frame, row, column)?;
            if matches!(cell.width, CellWidth::SpacerTail | CellWidth::SpacerHead) {
                continue;
            }
            if row == point.row && column == point.column {
                hit = Some(characters.len());
            }
            if cell.grapheme.is_empty() {
                characters.push((' ', row, column));
            } else {
                characters.extend(
                    cell.grapheme
                        .chars()
                        .map(|character| (character, row, column)),
                );
            }
        }
    }
    let hit = hit?;
    let text = characters
        .iter()
        .map(|(character, ..)| *character)
        .collect::<Vec<_>>();
    let under = |spans: Vec<(usize, usize)>| {
        spans
            .into_iter()
            .find(|(start, end)| (*start..*end).contains(&hit))
    };
    let (path, (start, end)) = under(url_spans(&text))
        .map(|span| (false, span))
        .or_else(|| under(quick_select::path_spans(&text)).map(|span| (true, span)))?;
    Some(Link {
        url: text[start..end].iter().collect(),
        path,
        spans: spans_of(
            characters[start..end]
                .iter()
                .map(|(_, row, column)| (*row, *column)),
        ),
    })
}

/// Where URLs start and end in some text, as character indexes.
pub(super) fn url_spans(text: &[char]) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut index = 0;
    while index < text.len() {
        let scheme = SCHEMES.iter().find(|scheme| {
            let scheme = scheme.chars().collect::<Vec<_>>();
            text[index..].starts_with(&scheme)
        });
        let Some(scheme) = scheme else {
            index += 1;
            continue;
        };
        let start = index;
        let mut end = start + scheme.len();
        while end < text.len() && is_url_character(text[end]) {
            end += 1;
        }
        let end = start + trimmed_url_length(&text[start..end]);
        if end > start + scheme.len() {
            spans.push((start, end));
        }
        index = end.max(start + 1);
    }
    spans
}

fn is_url_character(character: char) -> bool {
    !character.is_whitespace()
        && !character.is_control()
        && !matches!(
            character,
            '<' | '>' | '"' | '\'' | '`' | '{' | '}' | '|' | '\\' | '^'
        )
}

/// A URL without the punctuation of the sentence around it: a full stop
/// after it, or the bracket it was written inside.
fn trimmed_url_length(url: &[char]) -> usize {
    let mut length = url.len();
    loop {
        let Some(&last) = url[..length].last() else {
            return length;
        };
        let count = |wanted: char| url[..length].iter().filter(|c| **c == wanted).count();
        let unbalanced = match last {
            ')' => count('(') < count(')'),
            ']' => count('[') < count(']'),
            '.' | ',' | ';' | ':' | '!' | '?' => true,
            _ => false,
        };
        if !unbalanced {
            return length;
        }
        length -= 1;
    }
}

/// Cells in reading order, gathered into one span per row.
/// Where a path written in a pane's output is: from the home directory for
/// `~/`, else from the pane's working directory, without the line and column
/// a compiler or grep put after it.
pub(super) fn resolve_written_path(written: &str, cwd: &Path, home: Option<&Path>) -> PathBuf {
    let path = quick_select::without_line_and_column(written);
    match (path.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => home.join(rest),
        _ if path == "~" => home.map_or_else(|| cwd.join(path), Path::to_path_buf),
        _ => cwd.join(path),
    }
}

/// Show a path written in a pane's output in Finder, or a folder's
/// contents; a file is never opened, so a click can't start a program.
pub(super) fn reveal_path(path: &Path) -> std::io::Result<()> {
    let mut command = if cfg!(target_os = "macos") {
        Command::new("/usr/bin/open")
    } else {
        Command::new("xdg-open")
    };
    if path.is_dir() {
        command.arg(path);
    } else if cfg!(target_os = "macos") {
        command.arg("-R").arg(path);
    } else {
        command.arg(path.parent().unwrap_or(Path::new("/")));
    }
    command.spawn().map(|_| ())
}

/// What copying a link puts on the clipboard: a file's path, or the URL.
pub(super) fn link_text(url: &str) -> &str {
    url.strip_prefix("file://").map_or(url, |path| {
        path.find('/').map_or(path, |slash| &path[slash..])
    })
}

pub(super) fn spans_of(cells: impl Iterator<Item = (u16, u16)>) -> Vec<(u16, u16, u16)> {
    let mut spans: Vec<(u16, u16, u16)> = Vec::new();
    for (row, column) in cells {
        match spans.last_mut() {
            Some((last_row, _, end)) if *last_row == row => *end = column + 1,
            _ => spans.push((row, column, column + 1)),
        }
    }
    spans
}

/// Open a link the way the system opens it. Files are shown, not opened.
pub(super) fn open_link(url: &str) -> std::io::Result<()> {
    let mut command = if cfg!(target_os = "macos") {
        Command::new("/usr/bin/open")
    } else {
        Command::new("xdg-open")
    };
    if let Some(path) = url.strip_prefix("file://") {
        // file://host/path: keep the path.
        let path = path.find('/').map_or(path, |slash| &path[slash..]);
        if cfg!(target_os = "macos") {
            command.arg("-R").arg(path);
        } else {
            command.arg(
                std::path::Path::new(path)
                    .parent()
                    .unwrap_or(std::path::Path::new("/")),
            );
        }
    } else {
        command.arg(url);
    }
    command.spawn().map(|_| ())
}

impl MuxApp {
    pub(super) fn link_under(
        &self,
        pane_id: PaneId,
        rect: layout::Rect,
        position: gpui::Point<gpui::Pixels>,
    ) -> Option<Link> {
        let pane = self.panes.get(&pane_id)?;
        let point = self.selection_pointer(rect, &pane.frame, position).point?;
        link_at(&pane.frame, point)
    }

    /// Follow the pointer over a pane: with ⌘ held, the link under it.
    pub(super) fn hover_link(
        &mut self,
        pane_id: PaneId,
        rect: layout::Rect,
        position: gpui::Point<gpui::Pixels>,
        command: bool,
        cx: &mut Context<Self>,
    ) {
        self.link_pointer = Some((pane_id, rect, position));
        let hover = command
            .then(|| self.link_under(pane_id, rect, position))
            .flatten()
            .map(|link| (pane_id, link));
        if hover != self.link_hover {
            self.link_hover = hover;
            cx.notify();
        }
    }

    /// ⌘ pressed or let go with the pointer still.
    pub(super) fn link_modifiers_changed(&mut self, command: bool, cx: &mut Context<Self>) {
        if let Some((pane_id, rect, position)) = self.link_pointer {
            self.hover_link(pane_id, rect, position, command, cx);
        }
    }

    /// The pointer moved anywhere in the window: off the pane it was over,
    /// there is no link under it.
    pub(super) fn track_link_pointer(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        let (x, y) = (f32::from(position.x), f32::from(position.y));
        let off = self.link_pointer.is_some_and(|(_, rect, _)| {
            x < rect.x || y < rect.y || x >= rect.x + rect.width || y >= rect.y + rect.height
        });
        if off {
            self.link_pointer = None;
            if self.link_hover.take().is_some() {
                cx.notify();
            }
        }
    }

    /// Output moves text under a still pointer; the link under it follows.
    pub(super) fn refresh_link_hover(&mut self, cx: &mut Context<Self>) {
        if self.link_hover.is_some()
            && let Some((pane_id, rect, position)) = self.link_pointer
        {
            self.hover_link(pane_id, rect, position, true, cx);
        }
    }

    /// Put a link, or a path's text, on the clipboard.
    pub(super) fn copy_link(&mut self, link: &Link, cx: &mut Context<Self>) {
        if let Some(clipboard) = &mut self.clipboard
            && clipboard.set_text(link_text(&link.url).to_owned()).is_ok()
        {
            let what = if link.path || link.url.starts_with("file://") {
                "copied the path"
            } else {
                "copied the link"
            };
            self.say_in_strip(what, false, cx);
        }
    }

    /// ⌘-click on a link opens it. Returns whether the click was a link's.
    pub(super) fn open_link_under(
        &mut self,
        pane_id: PaneId,
        rect: layout::Rect,
        event: &gpui::MouseDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if event.button != gpui::MouseButton::Left || !event.modifiers.platform {
            return false;
        }
        let Some(link) = self.link_under(pane_id, rect, event.position) else {
            return false;
        };
        self.follow_link(pane_id, &link, cx);
        true
    }

    /// Open a link, or have the daemon find a written path from where the
    /// pane's shell is and show it.
    pub(super) fn follow_link(&mut self, pane_id: PaneId, link: &Link, cx: &mut Context<Self>) {
        if link.path {
            self.backend.send(CommandMessage::RevealPath {
                pane_id,
                written: link.url.clone(),
            });
        } else if let Err(error) = open_link(&link.url) {
            self.say_in_strip(format!("couldn't open {}: {error}", link.url), true, cx);
        }
    }

    /// Open the directory a pane's shell is in now, in Finder.
    pub(super) fn reveal_directory(&self, pane_id: PaneId) {
        self.backend.send(CommandMessage::RevealPath {
            pane_id,
            written: ".".to_owned(),
        });
    }

    pub(super) fn pane_link_hovered(&self, pane_id: PaneId) -> bool {
        self.link_hover
            .as_ref()
            .is_some_and(|(hovered, _)| *hovered == pane_id)
    }

    /// The hovered link's underline, in the tab's ink.
    pub(super) fn render_link_underline(
        &self,
        geometry: layout::PaneGeometry,
    ) -> Vec<gpui::AnyElement> {
        let Some((_, link)) = self
            .link_hover
            .as_ref()
            .filter(|(pane_id, _)| *pane_id == geometry.pane_id)
        else {
            return Vec::new();
        };
        let Some(pane) = self.panes.get(&geometry.pane_id) else {
            return Vec::new();
        };
        let metrics = self.metrics;
        let rect = geometry.rect;
        let padding =
            metrics.balanced_padding(rect.width, rect.height, pane.frame.cols, pane.frame.rows);
        let underline = self.active_ink().color();
        link.spans
            .iter()
            .map(|(row, start, end)| {
                div()
                    .absolute()
                    .left(px(rect.x
                        + padding.left
                        + f32::from(*start) * metrics.cell_width))
                    .top(px(rect.y
                        + padding.top
                        + f32::from(*row + 1) * metrics.cell_height
                        - 2.0))
                    .w(px(f32::from(end - start) * metrics.cell_width))
                    .h(px(1.0))
                    .bg(underline)
                    .into_any_element()
            })
            .collect()
    }

    /// Where the hovered link goes, said in the strip.
    pub(super) fn render_link_target(&self) -> Option<gpui::AnyElement> {
        let (_, link) = self.link_hover.as_ref()?;
        let verb = if link.path || link.url.starts_with("file://") {
            "show"
        } else {
            "open"
        };
        Some(
            h_flex()
                .min_w(px(0.0))
                .h(px(24.0))
                .px(px(6.0))
                .gap(px(8.0))
                .items_center()
                .child(kbd("⌘click", color(TEXT)))
                .child(div().flex_none().text_color(color(FAINT_TEXT)).child(verb))
                .child(
                    div()
                        .min_w(px(0.0))
                        .truncate()
                        .text_color(color(MUTED_TEXT))
                        .child(link.url.clone()),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mux_terminal::{CellStyle, RenderDirty, RenderRow, SemanticContent, TerminalScrollState};

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
            images: Vec::new(),
            held: false,
            redraw_in: None,
        }
    }

    fn at(column: u16, row: u16) -> TerminalPoint {
        TerminalPoint { column, row }
    }

    #[test]
    fn a_written_path_is_a_link_found_from_the_working_directory() {
        let output = frame(
            &["error at src/main.rs:12:5, see ~/notes.md."],
            60,
            &[false],
        );
        let link = link_at(&output, at(12, 0)).expect("path link");
        assert!(link.path);
        assert_eq!(link.url, "src/main.rs:12:5");
        assert_eq!(link.spans, vec![(0, 9, 25)]);
        let notes = link_at(&output, at(34, 0)).expect("home path link");
        assert_eq!(notes.url, "~/notes.md");
        assert!(link_at(&output, at(2, 0)).is_none());

        let cwd = Path::new("/work/mux");
        let home = Some(Path::new("/Users/r"));
        assert_eq!(
            resolve_written_path("src/main.rs:12:5", cwd, home),
            Path::new("/work/mux/src/main.rs")
        );
        assert_eq!(
            resolve_written_path("~/notes.md", cwd, home),
            Path::new("/Users/r/notes.md")
        );
        assert_eq!(
            resolve_written_path("/etc/hosts", cwd, home),
            Path::new("/etc/hosts")
        );
    }

    #[test]
    fn a_copied_file_link_is_its_path() {
        assert_eq!(
            link_text("file://mac.local/Users/r/a b.txt"),
            "/Users/r/a b.txt"
        );
        assert_eq!(link_text("file:///tmp/x"), "/tmp/x");
        assert_eq!(link_text("https://example.com/a"), "https://example.com/a");
    }

    #[test]
    fn a_written_url_is_found_without_the_sentence_around_it() {
        let frame = frame(&["see (https://example.com/a_(b)) now."], 40, &[false]);
        let link = link_at(&frame, at(10, 0)).expect("a link");
        assert_eq!(link.url, "https://example.com/a_(b)");
        assert_eq!(link.spans, vec![(0, 5, 30)]);
        assert_eq!(link_at(&frame, at(1, 0)), None);
    }

    #[test]
    fn a_url_wrapped_across_rows_is_one_link() {
        let frame = frame(&["go https://exa", "mple.com/x ok"], 14, &[true, false]);
        let link = link_at(&frame, at(2, 1)).expect("a link");
        assert_eq!(link.url, "https://example.com/x");
        assert_eq!(link.spans, vec![(0, 3, 14), (1, 0, 10)]);
    }

    #[test]
    fn a_hyperlink_covers_the_cells_that_carry_it() {
        let mut frame = frame(&["read the docs here"], 20, &[false]);
        for column in 9..13 {
            frame.cells[column].hyperlink = Some("https://docs.example".to_owned());
        }
        let link = link_at(&frame, at(10, 0)).expect("a hyperlink");
        assert_eq!(link.url, "https://docs.example");
        assert_eq!(link.spans, vec![(0, 9, 13)]);
    }
}
