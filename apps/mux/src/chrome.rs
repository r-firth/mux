//! The warm "slabs" chrome: tab inks, the dot-matrix face, and terminal titles.

use gpui::{Hsla, IntoElement, Styled, canvas, fill, point, px, rgb, size};

/// The colour a tab carries. It tints the focused pane, the mode pill and the
/// grain ground, so the active tab can be told apart at a glance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ink {
    Peach,
    Rose,
    Teal,
    Gold,
}

impl Ink {
    pub const ALL: [Self; 4] = [Self::Peach, Self::Rose, Self::Teal, Self::Gold];

    #[must_use]
    pub const fn for_position(index: usize) -> Self {
        Self::ALL[index % Self::ALL.len()]
    }

    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Peach => Self::Rose,
            Self::Rose => Self::Teal,
            Self::Teal => Self::Gold,
            Self::Gold => Self::Peach,
        }
    }

    #[must_use]
    pub const fn rgb(self) -> u32 {
        match self {
            Self::Peach => 0x00f2_9a6b,
            Self::Rose => 0x00d0_708b,
            Self::Teal => 0x003f_b3b5,
            Self::Gold => 0x00d8_a35c,
        }
    }

    #[must_use]
    pub fn color(self) -> Hsla {
        rgb(self.rgb()).into()
    }

    /// The soft ring drawn around whatever this ink marks as focused.
    #[must_use]
    pub fn wash(self) -> Hsla {
        self.color().opacity(0.13)
    }

    /// The dithered grain ground painted behind the slabs for this ink.
    #[must_use]
    pub const fn ground(self) -> &'static str {
        match self {
            Self::Peach => "mux/grounds/peach.png",
            Self::Rose => "mux/grounds/rose.png",
            Self::Teal => "mux/grounds/teal.png",
            Self::Gold => "mux/grounds/gold.png",
        }
    }
}

/// Embedded grain grounds, served through the application's asset source.
#[must_use]
pub fn ground_asset(path: &str) -> Option<&'static [u8]> {
    Some(match path {
        "mux/grounds/peach.png" => include_bytes!("../assets/grounds/peach.png").as_slice(),
        "mux/grounds/rose.png" => include_bytes!("../assets/grounds/rose.png").as_slice(),
        "mux/grounds/teal.png" => include_bytes!("../assets/grounds/teal.png").as_slice(),
        "mux/grounds/gold.png" => include_bytes!("../assets/grounds/gold.png").as_slice(),
        _ => return None,
    })
}

#[must_use]
pub fn ground_assets() -> Vec<gpui::SharedString> {
    Ink::ALL
        .iter()
        .map(|ink| gpui::SharedString::from(ink.ground()))
        .collect()
}

const GLYPH_ROWS: usize = 7;
/// One blank row of unlit dots above and below the glyphs, like an LED panel.
const PANEL_ROWS: usize = GLYPH_ROWS + 2;
const GLYPH_COLUMNS: usize = 5;

/// 5×7 glyphs, one byte per row, leftmost dot in bit 4.
fn glyph(character: char) -> Option<[u8; GLYPH_ROWS]> {
    Some(match character.to_ascii_lowercase() {
        'a' => [0x00, 0x00, 0x0e, 0x01, 0x0f, 0x11, 0x0f],
        'b' => [0x10, 0x10, 0x16, 0x19, 0x11, 0x11, 0x1e],
        'c' => [0x00, 0x00, 0x0e, 0x10, 0x10, 0x11, 0x0e],
        'd' => [0x01, 0x01, 0x0d, 0x13, 0x11, 0x11, 0x0f],
        'e' => [0x00, 0x00, 0x0e, 0x11, 0x1f, 0x10, 0x0e],
        'f' => [0x06, 0x09, 0x08, 0x1c, 0x08, 0x08, 0x08],
        'g' => [0x00, 0x0f, 0x11, 0x11, 0x0f, 0x01, 0x0e],
        'h' => [0x10, 0x10, 0x16, 0x19, 0x11, 0x11, 0x11],
        'i' => [0x04, 0x00, 0x0c, 0x04, 0x04, 0x04, 0x0e],
        'j' => [0x02, 0x00, 0x06, 0x02, 0x02, 0x12, 0x0c],
        'k' => [0x10, 0x10, 0x12, 0x14, 0x18, 0x14, 0x12],
        'l' => [0x0c, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0e],
        'm' => [0x00, 0x00, 0x1a, 0x15, 0x15, 0x15, 0x15],
        'n' => [0x00, 0x00, 0x16, 0x19, 0x11, 0x11, 0x11],
        'o' => [0x00, 0x00, 0x0e, 0x11, 0x11, 0x11, 0x0e],
        'p' => [0x00, 0x1e, 0x11, 0x11, 0x1e, 0x10, 0x10],
        'q' => [0x00, 0x0f, 0x11, 0x11, 0x0f, 0x01, 0x01],
        'r' => [0x00, 0x00, 0x16, 0x19, 0x10, 0x10, 0x10],
        's' => [0x00, 0x00, 0x0f, 0x10, 0x0e, 0x01, 0x1e],
        't' => [0x08, 0x08, 0x1c, 0x08, 0x08, 0x09, 0x06],
        'u' => [0x00, 0x00, 0x11, 0x11, 0x11, 0x13, 0x0d],
        'v' => [0x00, 0x00, 0x11, 0x11, 0x11, 0x0a, 0x04],
        'w' => [0x00, 0x00, 0x11, 0x11, 0x15, 0x15, 0x0a],
        'x' => [0x00, 0x00, 0x11, 0x0a, 0x04, 0x0a, 0x11],
        'y' => [0x00, 0x11, 0x11, 0x11, 0x0f, 0x01, 0x0e],
        'z' => [0x00, 0x00, 0x1f, 0x02, 0x04, 0x08, 0x1f],
        '0' => [0x0e, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0e],
        '1' => [0x04, 0x0c, 0x04, 0x04, 0x04, 0x04, 0x0e],
        '2' => [0x0e, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1f],
        '3' => [0x1f, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0e],
        '4' => [0x02, 0x06, 0x0a, 0x12, 0x1f, 0x02, 0x02],
        '5' => [0x1f, 0x10, 0x1e, 0x01, 0x01, 0x11, 0x0e],
        '6' => [0x06, 0x08, 0x10, 0x1e, 0x11, 0x11, 0x0e],
        '7' => [0x1f, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0e, 0x11, 0x11, 0x0e, 0x11, 0x11, 0x0e],
        '9' => [0x0e, 0x11, 0x11, 0x0f, 0x01, 0x02, 0x0c],
        '-' => [0x00, 0x00, 0x00, 0x1f, 0x00, 0x00, 0x00],
        '_' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1f],
        '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04],
        ' ' => [0x00; GLYPH_ROWS],
        _ => return None,
    })
}

/// Lit/unlit dots for `text`, column by column, or `None` when a character
/// has no glyph and the caller should fall back to ordinary type.
fn dot_columns(text: &str, max_characters: usize) -> Option<Vec<[bool; GLYPH_ROWS]>> {
    let mut columns = Vec::new();
    for (index, character) in text.chars().take(max_characters).enumerate() {
        let rows = glyph(character)?;
        if index > 0 {
            columns.push([false; GLYPH_ROWS]);
        }
        for column in 0..GLYPH_COLUMNS {
            let bit = 1 << (GLYPH_COLUMNS - 1 - column);
            columns.push(rows.map(|row| row & bit != 0));
        }
    }
    (!columns.is_empty()).then_some(columns)
}

/// gofer's dot-matrix face, used for "where" (the session) and nothing else.
/// Returns `None` for text it cannot draw.
#[must_use]
pub fn dot_matrix(text: &str, pitch: f32, lit: Hsla) -> Option<gpui::AnyElement> {
    let columns = dot_columns(text, 16)?;
    let width = pitch * columns.len() as f32;
    let height = pitch * PANEL_ROWS as f32;
    let radius = pitch * 0.38;
    let unlit = lit.opacity(0.08);
    Some(
        canvas(
            |_, _, _| (),
            move |bounds, (), window, _| {
                for (x, column) in columns.iter().enumerate() {
                    for y in 0..PANEL_ROWS {
                        let on = (1..=GLYPH_ROWS).contains(&y) && column[y - 1];
                        let center = point(
                            bounds.origin.x + px(pitch * (x as f32 + 0.5)),
                            bounds.origin.y + px(pitch * (y as f32 + 0.5)),
                        );
                        let dot = gpui::Bounds::new(
                            point(center.x - px(radius), center.y - px(radius)),
                            size(px(radius * 2.0), px(radius * 2.0)),
                        );
                        window.paint_quad(
                            fill(dot, if on { lit } else { unlit }).corner_radii(px(radius)),
                        );
                    }
                }
            },
        )
        .w(px(width))
        .h(px(height))
        .into_any_element(),
    )
}

/// Follows OSC 0/2 window-title sequences in a pane's output so the pane head
/// can name what is running. Sequences may be split across output chunks.
#[derive(Debug, Default)]
pub struct TitleScanner {
    state: TitleScan,
    buffer: Vec<u8>,
    title: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum TitleScan {
    #[default]
    Ground,
    Escape,
    Number(u16),
    Title,
    TitleEscape,
    Other,
    OtherEscape,
}

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;
const MAX_TITLE_BYTES: usize = 256;

impl TitleScanner {
    #[must_use]
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub fn scan(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.state = match (self.state, byte) {
                (TitleScan::Title, BEL) | (TitleScan::TitleEscape, b'\\') => {
                    self.commit();
                    TitleScan::Ground
                }
                (TitleScan::Title, ESC) => TitleScan::TitleEscape,
                (TitleScan::Title, _) => {
                    if self.buffer.len() < MAX_TITLE_BYTES {
                        self.buffer.push(byte);
                    }
                    TitleScan::Title
                }
                (TitleScan::Number(number), b'0'..=b'9') => TitleScan::Number(
                    number
                        .saturating_mul(10)
                        .saturating_add(u16::from(byte - b'0')),
                ),
                (TitleScan::Number(0 | 2), b';') => {
                    self.buffer.clear();
                    TitleScan::Title
                }
                (TitleScan::Number(_) | TitleScan::Other, ESC) => TitleScan::OtherEscape,
                (TitleScan::Number(_) | TitleScan::Other, byte) if byte != BEL => TitleScan::Other,
                (TitleScan::Escape, b']') => TitleScan::Number(0),
                // Any other escape starts a new sequence; everything else,
                // including a BEL ending a sequence we ignore, returns to text.
                (_, ESC) => TitleScan::Escape,
                _ => TitleScan::Ground,
            };
        }
    }

    fn commit(&mut self) {
        let title = String::from_utf8_lossy(&self.buffer).trim().to_owned();
        self.title = (!title.is_empty()).then_some(title);
        self.buffer.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_scanner_reads_bel_and_st_terminated_titles() {
        let mut scanner = TitleScanner::default();
        scanner.scan(b"hello\x1b]0;~/projects/mux\x07world");
        assert_eq!(scanner.title(), Some("~/projects/mux"));
        scanner.scan(b"\x1b]2;nvim layout.rs\x1b\\");
        assert_eq!(scanner.title(), Some("nvim layout.rs"));
    }

    #[test]
    fn title_scanner_survives_sequences_split_across_chunks() {
        let mut scanner = TitleScanner::default();
        scanner.scan(b"\x1b]2;car");
        assert_eq!(scanner.title(), None);
        scanner.scan(b"go test\x07");
        assert_eq!(scanner.title(), Some("cargo test"));
    }

    #[test]
    fn title_scanner_ignores_other_osc_sequences() {
        let mut scanner = TitleScanner::default();
        scanner.scan(b"\x1b]7;file://host/tmp\x07\x1b]8;;https://example.com\x1b\\link");
        assert_eq!(scanner.title(), None);
    }

    #[test]
    fn dot_matrix_draws_session_names_and_refuses_unknown_glyphs() {
        let columns = dot_columns("main", 16).expect("glyphs");
        assert_eq!(columns.len(), 4 * GLYPH_COLUMNS + 3);
        assert!(dot_columns("✨", 16).is_none());
    }

    #[test]
    fn inks_cycle_through_every_colour() {
        let mut ink = Ink::Peach;
        for expected in [Ink::Rose, Ink::Teal, Ink::Gold, Ink::Peach] {
            ink = ink.next();
            assert_eq!(ink, expected);
        }
        assert_eq!(Ink::for_position(5), Ink::Rose);
    }
}
