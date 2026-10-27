//! Window titles, calls for attention, and copies to the clipboard, from a
//! pane's output.

/// Follows OSC 0/2 window-title sequences in a pane's output so the pane head
/// can name what is running. It also notices a program asking to be noticed,
/// which shares the same escape grammar: the bell, and the desktop
/// notifications of OSC 9 (iTerm2) and OSC 777 (rxvt, Ghostty). Sequences may
/// be split across output chunks. A program copying to the clipboard with OSC
/// 52, as Neovim, tmux and programs over ssh do, uses it too, and so do the
/// shell's OSC 133 marks, which say how the last command ended.
#[derive(Debug, Default)]
pub struct TitleScanner {
    state: TitleScan,
    buffer: Vec<u8>,
    title: Option<String>,
    /// The title before this one, such as a shell's directory before it
    /// titled the pane with the command it runs.
    previous_title: Option<String>,
    attention: Option<Attention>,
    /// Whether that call is only news, a program saying it is done, rather
    /// than something that wants the user.
    attention_is_news: bool,
    copied: Option<String>,
    /// The clipboard a program last asked to read with OSC 52, as it named it.
    read_asked: Option<String>,
    progress: Option<Progress>,
    exit_status: Option<i32>,
    commands: std::collections::VecDeque<CommandEnding>,
    command_started: Option<std::time::Instant>,
    marks_starts: bool,
}

/// How the command run from one prompt ended, as far as the shell said.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CommandEnding {
    /// Its exit status; `None` while it runs, or for an empty line.
    pub status: Option<i32>,
    /// How long it ran, from the shell's marks for its start and end.
    pub took: Option<std::time::Duration>,
}

impl CommandEnding {
    /// Its status, if it failed.
    #[must_use]
    pub fn failed(self) -> Option<i32> {
        self.status.filter(|&status| status != 0)
    }
}

/// How many prompts' worth of command endings a pane remembers.
const REMEMBERED_COMMANDS: usize = 10_000;

/// A program asking to be noticed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Attention {
    Bell,
    /// A desktop notification, with what it says.
    Notification(String),
}

/// A program saying how far along it is, with the OSC 9;4 progress report
/// that Ghostty shows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Progress {
    /// This far, in percent.
    Normal(u8),
    /// Failed, this far along.
    Error(u8),
    /// Busy, without saying how far.
    Indeterminate,
    /// Waiting, this far along.
    Paused(u8),
    /// No longer saying.
    Removed,
}

impl Progress {
    /// `4 ; state ; percent`, with `state` 0 taking the progress away.
    fn parse(report: &str) -> Option<Self> {
        let mut parts = report.split(';');
        if parts.next()? != "4" {
            return None;
        }
        let state = parts.next().unwrap_or("0");
        let percent = parts
            .next()
            .and_then(|percent| percent.trim().parse::<u16>().ok())
            .map(|percent| u8::try_from(percent.min(100)).unwrap_or(100));
        Some(match state {
            "0" => Self::Removed,
            "1" => Self::Normal(percent.unwrap_or(0)),
            "2" => Self::Error(percent.unwrap_or(100)),
            "3" => Self::Indeterminate,
            "4" => Self::Paused(percent.unwrap_or(0)),
            _ => return None,
        })
    }
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
    Clipboard,
    ClipboardEscape,
    Other,
    OtherEscape,
}

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;
const MAX_TITLE_BYTES: usize = 256;
const MAX_NOTICE_BYTES: usize = 512;
/// The most of a copy kept, base64 and all: past it, the copy is dropped
/// rather than cut short.
const MAX_CLIPBOARD_BYTES: usize = 8 * 1024 * 1024;

/// OSC 52's answer to a program that asked to read the clipboard `place`.
#[must_use]
pub fn clipboard_reply(place: &str, text: &str) -> Vec<u8> {
    use base64::Engine as _;
    let data = base64::engine::general_purpose::STANDARD.encode(text);
    format!("\x1b]52;{place};{data}\x1b\\").into_bytes()
}

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

    /// Take how commands ended from a scanner that has seen more of the
    /// output than this one, such as the daemon's.
    pub fn seed_command_endings(&mut self, endings: Vec<CommandEnding>) {
        self.commands = endings.into();
    }

    #[must_use]
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// The title the pane had before this one.
    #[must_use]
    pub fn previous_title(&self) -> Option<&str> {
        self.previous_title.as_deref()
    }

    /// What the program has asked to be noticed by since this was last
    /// called. A notification outranks a bell rung alongside it.
    pub fn take_attention(&mut self) -> Option<Attention> {
        self.take_call().map(|(call, _)| call)
    }

    /// The same, with whether the call is only news: a program saying with
    /// OSC 7501 that it is done, with nothing since that wants the user.
    pub fn take_call(&mut self) -> Option<(Attention, bool)> {
        let news = std::mem::take(&mut self.attention_is_news);
        self.attention.take().map(|call| (call, news))
    }

    /// How far along the program last said it was, if it has said since
    /// this was last called.
    pub fn take_progress(&mut self) -> Option<Progress> {
        self.progress.take()
    }

    /// The text a program last copied to the clipboard since this was last
    /// called.
    pub fn take_copied(&mut self) -> Option<String> {
        self.copied.take()
    }

    /// The clipboard a program asked to read since this was last called,
    /// as it named it (`c`, or empty), for [`clipboard_reply`].
    pub fn take_clipboard_read(&mut self) -> Option<String> {
        self.read_asked.take()
    }

    /// How the last command ended, as the shell said with OSC 133;D, unless
    /// another has started since.
    pub fn take_exit_status(&mut self) -> Option<i32> {
        self.exit_status.take()
    }

    /// How the command run from each prompt ended, oldest first, one for
    /// each prompt the shell marked; the newest is the prompt still
    /// waiting. Line them up with the screen's prompts from the newest.
    #[must_use]
    pub fn command_endings(&self) -> &std::collections::VecDeque<CommandEnding> {
        &self.commands
    }

    pub fn scan(&mut self, bytes: &[u8]) {
        let mut rest = bytes;
        while let Some((&byte, after)) = rest.split_first() {
            if self.state == TitleScan::Ground && byte != ESC && byte != BEL {
                // Nearly all of any output is plain text, which changes
                // nothing here: skip straight to the next byte that could.
                match memchr::memchr2(ESC, BEL, after) {
                    Some(at) => rest = &after[at..],
                    None => return,
                }
                continue;
            }
            rest = after;
            self.state = match (self.state, byte) {
                (TitleScan::Title, BEL) | (TitleScan::TitleEscape, b'\\') => {
                    self.commit();
                    TitleScan::Ground
                }
                (TitleScan::Notice(number), BEL) | (TitleScan::NoticeEscape(number), b'\\') => {
                    self.commit_notice(number);
                    TitleScan::Ground
                }
                (TitleScan::Clipboard, BEL) | (TitleScan::ClipboardEscape, b'\\') => {
                    self.commit_copy();
                    TitleScan::Ground
                }
                (TitleScan::Clipboard, ESC) => TitleScan::ClipboardEscape,
                (TitleScan::Clipboard, _) => {
                    // One past the most is kept, so a copy too big to keep
                    // is known to be.
                    if self.buffer.len() <= MAX_CLIPBOARD_BYTES {
                        self.buffer.push(byte);
                    }
                    TitleScan::Clipboard
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
                    // A bell wants the user, whatever was said before it.
                    self.attention_is_news = false;
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
                (TitleScan::Number(52), b';') => {
                    self.buffer.clear();
                    TitleScan::Clipboard
                }
                (TitleScan::Number(number @ (9 | 133 | 777)), b';') => {
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
        let title = (!title.is_empty()).then_some(title);
        if title != self.title {
            self.previous_title = std::mem::replace(&mut self.title, title);
        }
        self.buffer.clear();
    }

    /// OSC 52 ; where ; base64. Which clipboard the program names makes no
    /// difference, since a Mac has the one; a `?` asks to read it, which is
    /// noted for the app to answer or refuse.
    fn commit_copy(&mut self) {
        use base64::Engine as _;
        let buffer = std::mem::take(&mut self.buffer);
        if buffer.len() > MAX_CLIPBOARD_BYTES {
            return;
        }
        let Some(at) = buffer.iter().position(|&byte| byte == b';') else {
            return;
        };
        let data = &buffer[at + 1..];
        if data == b"?" {
            self.read_asked = Some(String::from_utf8_lossy(&buffer[..at]).into_owned());
            return;
        }
        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data)
            && let Ok(text) = String::from_utf8(bytes)
            && !text.is_empty()
        {
            self.copied = Some(text);
        }
    }

    fn commit_notice(&mut self, number: u16) {
        let text = String::from_utf8_lossy(&self.buffer).into_owned();
        self.buffer.clear();
        if number == 133 {
            self.commit_shell_mark(&text);
            return;
        }
        let said = if number == 9 {
            // ConEmu numbers its own OSC 9 extensions (9;4 is progress); only
            // iTerm2's free text is a notification.
            let numbered = text
                .split_once(';')
                .map_or(text.as_str(), |(head, _)| head)
                .bytes()
                .all(|byte| byte.is_ascii_digit());
            if numbered {
                if let Some(progress) = Progress::parse(&text) {
                    self.progress = Some(progress);
                }
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
        self.attention_is_news = false;
        self.attention = Some(if said.is_empty() {
            Attention::Bell
        } else {
            Attention::Notification(said)
        });
    }
}

impl TitleScanner {
    /// Take in what a program said it is doing (OSC 7501), as its progress
    /// and, when it waits on the user, finishes or fails, as a call.
    pub fn note_program_status(&mut self, status: &crate::ProgramStatus) {
        use crate::{ProgramNeed, ProgramState};
        let name = [&status.title, &status.app]
            .into_iter()
            .map(|name| name.trim())
            .find(|name| !name.is_empty())
            .unwrap_or("The program");
        let message = status
            .message
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let said = |otherwise: String| {
            Attention::Notification(if message.is_empty() {
                otherwise
            } else {
                message.clone()
            })
        };
        match status.state {
            ProgramState::Working => {
                self.progress = Some(
                    status
                        .progress
                        .map_or(Progress::Indeterminate, Progress::Normal),
                );
            }
            ProgramState::Blocked => {
                if let Some(percent) = status.progress {
                    self.progress = Some(Progress::Paused(percent));
                }
                self.attention_is_news = false;
                self.attention = Some(said(match status.need {
                    Some(ProgramNeed::Permission) => format!("{name} wants your permission"),
                    Some(ProgramNeed::Question) => format!("{name} has a question"),
                    Some(ProgramNeed::Auth) => format!("{name} needs you to sign in"),
                    None => format!("{name} needs you"),
                }));
            }
            ProgramState::Done => {
                self.progress = Some(Progress::Removed);
                self.attention = Some(said(format!("{name} is done")));
                self.attention_is_news = true;
            }
            ProgramState::Error => {
                self.progress = Some(Progress::Error(status.progress.unwrap_or(100)));
                self.attention_is_news = false;
                self.attention = Some(said(format!("{name} failed")));
            }
            ProgramState::Idle | ProgramState::Clear => self.progress = Some(Progress::Removed),
        }
    }

    /// OSC 133: `C` as a command starts, `D ; status` as it ends. A command
    /// starting forgets how the one before ended.
    ///
    /// Each `A`, a prompt, starts a command's entry, and its `D` says how
    /// it ended. A shell that marks where commands start (zsh's `C`) has
    /// the `D` of an empty line left out; one that does not (bash) says
    /// it again, so it is taken as given.
    fn commit_shell_mark(&mut self, text: &str) {
        let mut parts = text.split(';');
        match parts.next() {
            Some("A") => {
                if self.commands.len() == REMEMBERED_COMMANDS {
                    self.commands.pop_front();
                }
                self.commands.push_back(CommandEnding::default());
                self.command_started = None;
            }
            Some("C") => {
                self.exit_status = None;
                self.command_started = Some(std::time::Instant::now());
                self.marks_starts = true;
            }
            Some("D") => {
                self.exit_status = parts.next().and_then(|status| status.trim().parse().ok());
                let started = self.command_started.take();
                if started.is_some() || !self.marks_starts {
                    let ending = CommandEnding {
                        status: self.exit_status,
                        took: started.map(|started| started.elapsed()),
                    };
                    // The first prompt this pane saw may have come before
                    // Mux was listening, as after a restart.
                    match self.commands.back_mut() {
                        Some(last) => *last = ending,
                        None => self.commands.push_back(ending),
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_skipped_without_losing_what_follows_it() {
        let mut scanner = TitleScanner::default();
        let mut output = vec![b'a'; 10_000];
        output.extend_from_slice(b"\x1b]2;after the text\x07");
        output.extend(vec![b'b'; 10_000]);
        output.push(BEL);
        // In one piece, and in pieces that split the text and the sequence.
        scanner.scan(&output);
        assert_eq!(scanner.title(), Some("after the text"));
        assert_eq!(scanner.take_attention(), Some(Attention::Bell));

        let mut pieces = TitleScanner::default();
        for piece in output.chunks(977) {
            pieces.scan(piece);
        }
        assert_eq!(pieces.title(), Some("after the text"));
        assert_eq!(pieces.take_attention(), Some(Attention::Bell));
    }

    #[test]
    fn title_scanner_reads_bel_and_st_terminated_titles() {
        let mut scanner = TitleScanner::default();
        scanner.scan(b"hello\x1b]0;~/projects/mux\x07world");
        assert_eq!(scanner.title(), Some("~/projects/mux"));
        scanner.scan(b"\x1b]2;nvim layout.rs\x1b\\");
        assert_eq!(scanner.title(), Some("nvim layout.rs"));
        assert_eq!(scanner.previous_title(), Some("~/projects/mux"));
        scanner.scan(b"\x1b]2;nvim layout.rs\x07");
        assert_eq!(
            scanner.previous_title(),
            Some("~/projects/mux"),
            "the same title again is no change"
        );
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

    fn status(state: crate::ProgramState) -> crate::ProgramStatus {
        crate::ProgramStatus {
            state,
            need: None,
            progress: None,
            app: "cargo".to_owned(),
            title: String::new(),
            message: String::new(),
        }
    }

    #[test]
    fn a_program_saying_it_is_done_is_only_news() {
        use crate::ProgramState;
        let mut scanner = TitleScanner::default();
        scanner.note_program_status(&status(ProgramState::Done));
        assert_eq!(
            scanner.take_call(),
            Some((Attention::Notification("cargo is done".to_owned()), true))
        );
        for state in [ProgramState::Blocked, ProgramState::Error] {
            scanner.note_program_status(&status(state));
            assert!(matches!(scanner.take_call(), Some((_, false))));
        }
        // A bell after it wants you, though the words stay.
        scanner.note_program_status(&status(ProgramState::Done));
        scanner.scan(b"\x07");
        assert_eq!(
            scanner.take_call(),
            Some((Attention::Notification("cargo is done".to_owned()), false))
        );
        // So does a notification.
        scanner.note_program_status(&status(ProgramState::Done));
        scanner.scan(b"\x1b]9;look\x07");
        assert_eq!(
            scanner.take_call(),
            Some((Attention::Notification("look".to_owned()), false))
        );
    }

    #[test]
    fn program_status_reports_become_progress_and_calls() {
        use crate::{ProgramNeed, ProgramState};
        let mut scanner = TitleScanner::default();
        scanner.note_program_status(&crate::ProgramStatus {
            progress: Some(40),
            ..status(ProgramState::Working)
        });
        assert_eq!(scanner.take_progress(), Some(Progress::Normal(40)));
        assert_eq!(scanner.take_attention(), None);

        scanner.note_program_status(&status(ProgramState::Working));
        assert_eq!(scanner.take_progress(), Some(Progress::Indeterminate));

        scanner.note_program_status(&crate::ProgramStatus {
            need: Some(ProgramNeed::Permission),
            ..status(ProgramState::Blocked)
        });
        assert_eq!(
            scanner.take_attention(),
            Some(Attention::Notification(
                "cargo wants your permission".to_owned()
            ))
        );

        scanner.note_program_status(&crate::ProgramStatus {
            message: "Apply  these\nchanges?".to_owned(),
            ..status(ProgramState::Blocked)
        });
        assert_eq!(
            scanner.take_attention(),
            Some(Attention::Notification("Apply these changes?".to_owned()))
        );

        scanner.note_program_status(&status(ProgramState::Error));
        assert_eq!(scanner.take_progress(), Some(Progress::Error(100)));
        assert_eq!(
            scanner.take_attention(),
            Some(Attention::Notification("cargo failed".to_owned()))
        );

        scanner.note_program_status(&crate::ProgramStatus {
            title: "Deploy".to_owned(),
            ..status(ProgramState::Done)
        });
        assert_eq!(scanner.take_progress(), Some(Progress::Removed));
        assert_eq!(
            scanner.take_attention(),
            Some(Attention::Notification("Deploy is done".to_owned()))
        );

        scanner.note_program_status(&status(ProgramState::Clear));
        assert_eq!(scanner.take_progress(), Some(Progress::Removed));
        assert_eq!(scanner.take_attention(), None);
    }

    #[test]
    fn progress_reports_say_how_far_along() {
        let mut scanner = TitleScanner::default();
        assert_eq!(scanner.take_progress(), None);
        scanner.scan(b"\x1b]9;4;1;50\x07");
        assert_eq!(scanner.take_progress(), Some(Progress::Normal(50)));
        assert_eq!(scanner.take_progress(), None, "nothing new");
        scanner.scan(b"\x1b]9;4;1;250\x1b\\");
        assert_eq!(scanner.take_progress(), Some(Progress::Normal(100)));
        scanner.scan(b"\x1b]9;4;2\x07");
        assert_eq!(scanner.take_progress(), Some(Progress::Error(100)));
        scanner.scan(b"\x1b]9;4;3;\x07");
        assert_eq!(scanner.take_progress(), Some(Progress::Indeterminate));
        scanner.scan(b"\x1b]9;4;4;30\x07");
        assert_eq!(scanner.take_progress(), Some(Progress::Paused(30)));
        scanner.scan(b"\x1b]9;4;0;0\x07");
        assert_eq!(scanner.take_progress(), Some(Progress::Removed));
        scanner.scan(b"\x1b]9;4;7;1\x07\x1b]9;5\x07");
        assert_eq!(scanner.take_progress(), None, "not progress");
        assert_eq!(scanner.take_attention(), None);
    }

    #[test]
    fn the_shell_says_how_the_last_command_ended() {
        let mut scanner = TitleScanner::default();
        scanner.scan(b"\x1b]133;C\x07out\r\n\x1b]133;D;2\x07\x1b]133;A\x07$ ");
        assert_eq!(scanner.take_exit_status(), Some(2));
        assert_eq!(scanner.take_exit_status(), None);
        scanner.scan(b"\x1b]133;D;0\x1b\\\x1b]133;C\x07");
        assert_eq!(scanner.take_exit_status(), None, "a new command forgets it");
        scanner.scan(b"\x1b]133;D\x07");
        assert_eq!(scanner.take_exit_status(), None);
    }

    #[test]
    fn each_prompt_remembers_how_its_command_ended() {
        let mut scanner = TitleScanner::default();
        // zsh: an empty line's prompt has no `C`, and no ending.
        scanner.scan(b"\x1b]133;A\x07$ \x1b]133;C\x07");
        scanner.scan(b"\x1b]133;D;2\x07\x1b]133;A\x07$ ");
        scanner.scan(b"\x1b]133;A\x07$ \x1b]133;C\x07\x1b]133;D;0\x07\x1b]133;A\x07");
        assert_eq!(
            scanner
                .command_endings()
                .iter()
                .map(|ending| ending.status)
                .collect::<Vec<_>>(),
            [Some(2), None, Some(0), None]
        );
        let first = scanner.command_endings()[0];
        assert!(first.took.is_some(), "timed from its start");
        assert_eq!(first.failed(), Some(2));
        assert_eq!(scanner.command_endings()[2].failed(), None);
        // bash marks no starts: each `D` is taken as given, the first for
        // the prompt that was there before Mux was listening.
        let mut scanner = TitleScanner::default();
        scanner.scan(b"\x1b]133;D;0\x07\x1b]133;A\x07$ ");
        scanner.scan(b"\x1b]133;D;1\x07\x1b]133;A\x07$ ");
        assert_eq!(
            scanner
                .command_endings()
                .iter()
                .map(|ending| ending.status)
                .collect::<Vec<_>>(),
            [Some(0), Some(1), None]
        );
        assert_eq!(
            scanner.command_endings()[1].took,
            None,
            "no start to time from"
        );
    }

    #[test]
    fn a_copy_to_the_clipboard_is_decoded() {
        let mut scanner = TitleScanner::default();
        scanner.scan(b"before\x1b]52;c;aGVsbG8gd29y");
        assert_eq!(scanner.take_copied(), None, "not finished yet");
        scanner.scan(b"bGQ=\x07after");
        assert_eq!(scanner.take_copied().as_deref(), Some("hello world"));
        assert_eq!(scanner.take_copied(), None);
        assert_eq!(scanner.take_attention(), None, "its BEL is no bell");

        scanner.scan(b"\x1b]52;;bXV4\x1b\\");
        assert_eq!(scanner.take_copied().as_deref(), Some("mux"));
        scanner.scan(b"\x1b]52;c;?\x07\x1b]52;c;!!!\x07");
        assert_eq!(scanner.take_copied(), None, "a read, and nonsense");
        assert_eq!(scanner.take_clipboard_read().as_deref(), Some("c"));
        assert_eq!(scanner.take_clipboard_read(), None);
        assert_eq!(
            clipboard_reply("c", "mux"),
            b"\x1b]52;c;bXV4\x1b\\".to_vec()
        );
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
