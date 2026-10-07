//! Window titles, and calls for attention, from a pane's output.

/// Follows OSC 0/2 window-title sequences in a pane's output so the pane head
/// can name what is running. It also notices a program asking to be noticed,
/// which shares the same escape grammar: the bell, and the desktop
/// notifications of OSC 9 (iTerm2) and OSC 777 (rxvt, Ghostty). Sequences may
/// be split across output chunks.
#[derive(Debug, Default)]
pub struct TitleScanner {
    state: TitleScan,
    buffer: Vec<u8>,
    title: Option<String>,
    attention: Option<Attention>,
}

/// A program asking to be noticed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Attention {
    Bell,
    /// A desktop notification, with what it says.
    Notification(String),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum TitleScan {
    #[default]
    Ground,
    Escape,
    Number(u16),
    Title,
    TitleEscape,
    Notice(u16),
    NoticeEscape(u16),
    Other,
    OtherEscape,
}

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;
const MAX_TITLE_BYTES: usize = 256;
const MAX_NOTICE_BYTES: usize = 512;

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

    /// What the program has asked to be noticed by since this was last
    /// called. A notification outranks a bell rung alongside it.
    pub fn take_attention(&mut self) -> Option<Attention> {
        self.attention.take()
    }

    pub fn scan(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.state = match (self.state, byte) {
                (TitleScan::Title, BEL) | (TitleScan::TitleEscape, b'\\') => {
                    self.commit();
                    TitleScan::Ground
                }
                (TitleScan::Notice(number), BEL) | (TitleScan::NoticeEscape(number), b'\\') => {
                    self.commit_notice(number);
                    TitleScan::Ground
                }
                (TitleScan::Notice(number), ESC) => TitleScan::NoticeEscape(number),
                (TitleScan::Notice(number), _) => {
                    if self.buffer.len() < MAX_NOTICE_BYTES {
                        self.buffer.push(byte);
                    }
                    TitleScan::Notice(number)
                }
                (TitleScan::Ground | TitleScan::Escape, BEL) => {
                    if self.attention.is_none() {
                        self.attention = Some(Attention::Bell);
                    }
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
                (TitleScan::Number(number @ (9 | 777)), b';') => {
                    self.buffer.clear();
                    TitleScan::Notice(number)
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

    fn commit_notice(&mut self, number: u16) {
        let text = String::from_utf8_lossy(&self.buffer).into_owned();
        self.buffer.clear();
        let said = if number == 9 {
            // ConEmu numbers its own OSC 9 extensions (9;4 is progress); only
            // iTerm2's free text is a notification.
            let numbered = text
                .split_once(';')
                .map_or(text.as_str(), |(head, _)| head)
                .bytes()
                .all(|byte| byte.is_ascii_digit());
            if numbered {
                return;
            }
            text
        } else {
            // OSC 777 ; notify ; title ; body
            let mut parts = text.splitn(3, ';');
            if parts.next() != Some("notify") {
                return;
            }
            let title = parts.next().unwrap_or_default();
            let body = parts.next().unwrap_or_default();
            if body.trim().is_empty() {
                title.to_owned()
            } else {
                body.to_owned()
            }
        };
        let said = said.split_whitespace().collect::<Vec<_>>().join(" ");
        self.attention = Some(if said.is_empty() {
            Attention::Bell
        } else {
            Attention::Notification(said)
        });
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
    fn a_bell_asks_for_attention_but_one_ending_a_sequence_does_not() {
        let mut scanner = TitleScanner::default();
        scanner.scan(b"\x1b]0;~/src\x07\x1b]7;file://host/tmp\x07plain");
        assert_eq!(scanner.take_attention(), None);
        scanner.scan(b"done\x07");
        assert_eq!(scanner.take_attention(), Some(Attention::Bell));
        assert_eq!(scanner.take_attention(), None);
    }

    #[test]
    fn notifications_say_what_they_say() {
        let mut scanner = TitleScanner::default();
        scanner.scan(b"\x1b]9;\n\nClaude needs your permission\x07\x07");
        assert_eq!(
            scanner.take_attention(),
            Some(Attention::Notification(
                "Claude needs your permission".to_owned()
            ))
        );
        scanner.scan(b"\x1b]777;notify;Codex;Turn complete\x1b\\");
        assert_eq!(
            scanner.take_attention(),
            Some(Attention::Notification("Turn complete".to_owned()))
        );
        // ConEmu's progress report is not a notification.
        scanner.scan(b"\x1b]9;4;1;50\x07");
        assert_eq!(scanner.take_attention(), None);
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
