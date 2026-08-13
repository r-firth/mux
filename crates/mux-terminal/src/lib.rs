//! The terminal-emulation boundary shared by the daemon and GUI replicas.
//!
//! `ReplayEngine` is intentionally not a VT emulator. It proves ordered attach
//! semantics while the libghostty adapter is brought in behind `TerminalEngine`.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderDirty {
    Clean,
    Partial,
    Full,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CursorStyle {
    Bar,
    Block,
    Underline,
    HollowBlock,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderCursor {
    pub visible: bool,
    pub blinking: bool,
    pub x: u16,
    pub y: u16,
    pub style: CursorStyle,
    pub color: Rgb,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CellWidth {
    Narrow,
    Wide,
    SpacerTail,
    SpacerHead,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SemanticContent {
    Output,
    Input,
    Prompt,
}

// Terminal rendition attributes are independent SGR toggles. Keeping them as
// named fields makes the Ghostty boundary exhaustive and avoids leaking a
// backend-specific bit layout into the renderer API.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CellStyle {
    pub bold: bool,
    pub italic: bool,
    pub faint: bool,
    pub blink: bool,
    pub inverse: bool,
    pub invisible: bool,
    pub strikethrough: bool,
    pub overline: bool,
    /// Ghostty's stable SGR underline value: 0 none, 1 single, 2 double,
    /// 3 curly, 4 dotted, 5 dashed.
    pub underline: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderCell {
    pub grapheme: String,
    pub foreground: Rgb,
    pub background: Rgb,
    pub underline_color: Rgb,
    pub style: CellStyle,
    pub width: CellWidth,
    pub semantic: SemanticContent,
    pub selected: bool,
    pub hyperlink: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RenderRow {
    pub wrapped: bool,
    pub continuation: bool,
    pub dirty: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderFrame {
    pub cols: u16,
    pub rows: u16,
    pub dirty: RenderDirty,
    pub background: Rgb,
    pub foreground: Rgb,
    pub cursor: Option<RenderCursor>,
    pub scroll: TerminalScrollState,
    pub row_metadata: Vec<RenderRow>,
    /// Row-major cells. There are exactly `cols * rows` entries.
    pub cells: Vec<RenderCell>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TerminalScrollState {
    pub total: u64,
    pub offset: u64,
    pub len: u64,
}

impl TerminalScrollState {
    #[must_use]
    pub fn is_scrolled(self) -> bool {
        self.offset.saturating_add(self.len) < self.total
    }
}

pub trait TerminalRenderer {
    fn render_frame(&mut self) -> Result<RenderFrame, TerminalError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalPoint {
    pub column: u16,
    pub row: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalSelection {
    pub anchor: TerminalPoint,
    pub focus: TerminalPoint,
    pub rectangular: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalKeyAction {
    Release,
    Press,
    Repeat,
}

// These independent modifier states cross the terminal-engine boundary; a
// backend-specific bit layout would couple callers more tightly than names do.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TerminalModifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub super_key: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalKey {
    Unidentified,
    Backquote,
    Backslash,
    BracketLeft,
    BracketRight,
    Comma,
    Digit(u8),
    Equal,
    IntlBackslash,
    IntlRo,
    IntlYen,
    Letter(char),
    Minus,
    Period,
    Quote,
    Semicolon,
    Slash,
    Backspace,
    Enter,
    Tab,
    Space,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Escape,
    Function(u8),
    NumpadEnter,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalKeyEvent {
    pub action: TerminalKeyAction,
    pub key: TerminalKey,
    pub modifiers: TerminalModifiers,
    /// Modifiers used by the keyboard layout to produce `text` rather than
    /// modifiers that should alter the bytes sent to the terminal.
    pub consumed_modifiers: TerminalModifiers,
    pub text: Option<String>,
    pub unshifted_codepoint: Option<char>,
    pub composing: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalViewportScroll {
    Top,
    Bottom,
    Delta(i64),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalMouseAction {
    Press,
    Release,
    Motion,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalMouseButton {
    Left,
    Right,
    Middle,
    Four,
    Five,
    Six,
    Seven,
    Eight,
    Nine,
    Ten,
    Eleven,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalMouseGeometry {
    pub screen_width: u32,
    pub screen_height: u32,
    pub cell_width: u32,
    pub cell_height: u32,
    pub padding_top: u32,
    pub padding_bottom: u32,
    pub padding_right: u32,
    pub padding_left: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalMouseEvent {
    pub action: TerminalMouseAction,
    pub button: Option<TerminalMouseButton>,
    pub modifiers: TerminalModifiers,
    pub x: f32,
    pub y: f32,
    pub geometry: TerminalMouseGeometry,
    pub any_button_pressed: bool,
}

/// GUI-local interactions whose semantics depend on terminal state. The
/// daemon does not own these transient view concerns, but the emulator still
/// decides selection boundaries and bracketed-paste encoding.
pub trait TerminalInteraction {
    fn set_selection(&mut self, selection: Option<TerminalSelection>) -> Result<(), TerminalError>;

    fn selected_text(&self) -> Result<Option<String>, TerminalError>;

    fn encode_paste(&self, text: &str) -> Result<Vec<u8>, TerminalError>;

    fn encode_key(&self, event: &TerminalKeyEvent) -> Result<Vec<u8>, TerminalError>;

    fn scroll_viewport(&mut self, scroll: TerminalViewportScroll) -> Result<(), TerminalError>;

    fn encode_mouse(&mut self, event: &TerminalMouseEvent) -> Result<Vec<u8>, TerminalError>;
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TerminalSize {
    pub cols: u16,
    pub rows: u16,
    pub cell_width_px: u32,
    pub cell_height_px: u32,
}

impl Default for TerminalSize {
    fn default() -> Self {
        Self {
            cols: 80,
            rows: 24,
            cell_width_px: 0,
            cell_height_px: 0,
        }
    }
}

impl TerminalSize {
    pub fn validate(self) -> Result<Self, TerminalError> {
        if self.cols == 0 || self.rows == 0 {
            Err(TerminalError::InvalidSize {
                cols: self.cols,
                rows: self.rows,
            })
        } else {
            Ok(self)
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EngineDescriptor {
    pub name: String,
    pub revision: String,
    pub checkpoint_format: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TerminalCheckpoint {
    pub descriptor: EngineDescriptor,
    /// The first output sequence not included in this checkpoint.
    pub next_sequence: u64,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OutputChunk {
    pub sequence: u64,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TerminalAttachment {
    pub descriptor: EngineDescriptor,
    pub checkpoint: Option<TerminalCheckpoint>,
    pub replay: Vec<OutputChunk>,
    /// The first sequence that was retained. Lower sequences are unavailable.
    pub retained_from_sequence: u64,
    /// The sequence expected for the next output chunk.
    pub next_sequence: u64,
}

pub trait TerminalEngine: Send {
    fn descriptor(&self) -> &EngineDescriptor;

    fn apply_output(&mut self, sequence: u64, bytes: &[u8]) -> Result<(), TerminalError>;

    fn resize(&mut self, size: TerminalSize) -> Result<(), TerminalError>;

    fn attachment(&self) -> Result<TerminalAttachment, TerminalError>;

    /// Returns terminal-generated replies that must be written to the PTY,
    /// such as device-status and cursor-position query responses.
    fn take_pty_responses(&mut self) -> Result<Vec<u8>, TerminalError> {
        Ok(Vec::new())
    }
}

/// A bounded raw-byte replay ledger used by protocol tests and non-product
/// builds. The native product uses the Ghostty engine.
#[derive(Debug)]
pub struct ReplayEngine {
    descriptor: EngineDescriptor,
    size: TerminalSize,
    max_bytes: usize,
    retained_bytes: usize,
    chunks: VecDeque<OutputChunk>,
    next_sequence: u64,
}

impl ReplayEngine {
    #[must_use]
    pub fn new(size: TerminalSize, max_bytes: usize) -> Self {
        Self {
            descriptor: EngineDescriptor {
                name: "mux.raw-replay".to_owned(),
                revision: env!("CARGO_PKG_VERSION").to_owned(),
                checkpoint_format: 0,
            },
            size,
            max_bytes,
            retained_bytes: 0,
            chunks: VecDeque::new(),
            next_sequence: 1,
        }
    }

    fn trim(&mut self) {
        while self.retained_bytes > self.max_bytes && self.chunks.len() > 1 {
            if let Some(chunk) = self.chunks.pop_front() {
                self.retained_bytes = self.retained_bytes.saturating_sub(chunk.bytes.len());
            }
        }
    }
}

impl TerminalEngine for ReplayEngine {
    fn descriptor(&self) -> &EngineDescriptor {
        &self.descriptor
    }

    fn apply_output(&mut self, sequence: u64, bytes: &[u8]) -> Result<(), TerminalError> {
        if sequence != self.next_sequence {
            return Err(TerminalError::OutOfOrder {
                expected: self.next_sequence,
                actual: sequence,
            });
        }

        self.next_sequence += 1;
        if bytes.is_empty() {
            return Ok(());
        }

        self.retained_bytes = self.retained_bytes.saturating_add(bytes.len());
        self.chunks.push_back(OutputChunk {
            sequence,
            bytes: bytes.to_vec(),
        });
        self.trim();
        Ok(())
    }

    fn resize(&mut self, size: TerminalSize) -> Result<(), TerminalError> {
        self.size = size.validate()?;
        Ok(())
    }

    fn attachment(&self) -> Result<TerminalAttachment, TerminalError> {
        let retained_from_sequence = self
            .chunks
            .front()
            .map_or(self.next_sequence, |chunk| chunk.sequence);
        Ok(TerminalAttachment {
            descriptor: self.descriptor.clone(),
            checkpoint: None,
            replay: self.chunks.iter().cloned().collect(),
            retained_from_sequence,
            next_sequence: self.next_sequence,
        })
    }
}

#[derive(Debug, Error)]
pub enum TerminalError {
    #[error("invalid terminal size {cols}x{rows}")]
    InvalidSize { cols: u16, rows: u16 },
    #[error("terminal output was out of order: expected {expected}, received {actual}")]
    OutOfOrder { expected: u64, actual: u64 },
    #[error("terminal engine failure: {0}")]
    Engine(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_is_ordered_and_bounded() {
        let mut engine = ReplayEngine::new(TerminalSize::default(), 5);
        engine.apply_output(1, b"abc").expect("first output");
        engine.apply_output(2, b"def").expect("second output");

        let attachment = engine.attachment().expect("attachment");
        assert_eq!(attachment.retained_from_sequence, 2);
        assert_eq!(attachment.next_sequence, 3);
        assert_eq!(attachment.replay[0].bytes, b"def");
    }

    #[test]
    fn replay_rejects_a_gap() {
        let mut engine = ReplayEngine::new(TerminalSize::default(), 1024);
        assert!(matches!(
            engine.apply_output(2, b"late"),
            Err(TerminalError::OutOfOrder {
                expected: 1,
                actual: 2,
            }),
        ));
    }
}
