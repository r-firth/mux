//! Window titles from a pane's output.

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
    /// A scanner that already knows the title, as a client restoring a pane
    /// from a checkpoint does: the sequence that set it is long gone.
    #[must_use]
    pub fn seeded(title: Option<String>) -> Self {
        Self {
            title: title.filter(|title| !title.trim().is_empty()),
            ..Self::default()
        }
    }

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
    fn title_scanner_starts_from_a_remembered_title() {
        let mut scanner = TitleScanner::seeded(Some("~/src/mux".to_owned()));
        assert_eq!(scanner.title(), Some("~/src/mux"));
        scanner.scan(b"plain output");
        assert_eq!(scanner.title(), Some("~/src/mux"));
        scanner.scan(b"\x1b]2;cargo test\x07");
        assert_eq!(scanner.title(), Some("cargo test"));
    }
}
