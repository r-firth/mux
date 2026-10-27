//! Safe ownership wrapper for the pinned libghostty-vt C shim.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use directories::BaseDirs;
use mux_terminal::Rgb;

pub const GHOSTTY_REVISION: &str = "ce787fc18a7782e81585beb63f0d905ccaf69284";

/// The colour subset of Ghostty configuration that affects VT rendering.
/// Unset values leave libghostty's current colour state untouched.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GhosttyTheme {
    pub background: Option<Rgb>,
    pub foreground: Option<Rgb>,
    pub cursor: Option<Rgb>,
    pub palette: [Option<Rgb>; 256],
}

impl Default for GhosttyTheme {
    fn default() -> Self {
        Self {
            background: None,
            foreground: None,
            cursor: None,
            palette: [None; 256],
        }
    }
}

/// The small Ghostty font subset consumed by Mux's native renderer.
///
/// The terminal adapter owns config discovery and parsing, while the renderer
/// remains responsible for resolving a local face and deriving grid metrics.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GhosttyFont {
    pub family: Option<String>,
    pub size: Option<f32>,
}

impl GhosttyFont {
    /// Load the primary regular family and point size from Ghostty's normal
    /// user config locations. Repeated families follow Ghostty's reset/append
    /// semantics; Mux uses the first family and leaves fallback to the native
    /// shaping stack.
    pub fn load_user() -> Result<Self, ThemeError> {
        let Some((_, configs)) = user_config_paths() else {
            return Ok(Self::default());
        };
        let mut font = Self::default();
        let mut families = Vec::new();
        for config in configs {
            let contents = read_config(&config)?;
            parse_font_config(&contents, &mut families, &mut font.size);
        }
        font.family = families.into_iter().next();
        Ok(font)
    }

    #[cfg(test)]
    fn load_from_path(path: &Path) -> Result<Self, ThemeError> {
        let contents = read_config(path)?;
        let mut families = Vec::new();
        let mut size = None;
        parse_font_config(&contents, &mut families, &mut size);
        Ok(Self {
            family: families.into_iter().next(),
            size,
        })
    }
}

impl GhosttyTheme {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.background.is_none()
            && self.foreground.is_none()
            && self.cursor.is_none()
            && self.palette.iter().all(Option::is_none)
    }

    /// Load Ghostty's user colour configuration from its conventional macOS
    /// or XDG location. Unknown Ghostty options are intentionally ignored.
    pub fn load_user() -> Result<Self, ThemeError> {
        let Some((roots, configs)) = user_config_paths() else {
            return Ok(Self::default());
        };
        if configs.is_empty() {
            return Ok(Self::default());
        }
        let mut theme = Self::default();
        let mut visited = HashSet::new();
        for config in configs {
            parse_theme_file(&config, &roots, &mut visited, &mut theme)?;
        }
        Ok(theme)
    }

    #[cfg(test)]
    fn load_from_path(path: &Path, roots: &[PathBuf]) -> Result<Self, ThemeError> {
        let mut theme = Self::default();
        let mut visited = HashSet::new();
        parse_theme_file(path, roots, &mut visited, &mut theme)?;
        Ok(theme)
    }
}

/// Ghostty's own default for `scrollback-limit`, in bytes.
pub const DEFAULT_HISTORY_BYTES: usize = 10_000_000;

/// How much history a pane keeps above its screen, in bytes: the
/// `scrollback-limit` from the user's Ghostty config when it sets one, so a
/// pane remembers as much as their Ghostty does, else Ghostty's default.
#[must_use]
pub fn user_history_limit() -> usize {
    let mut limit = DEFAULT_HISTORY_BYTES;
    if let Some((_, configs)) = user_config_paths() {
        for config in configs {
            if let Ok(contents) = read_config(&config) {
                parse_history_limit(&contents, &mut limit);
            }
        }
    }
    limit
}

fn user_config_paths() -> Option<(Vec<PathBuf>, Vec<PathBuf>)> {
    let base = BaseDirs::new()?;
    let home = base.home_dir();
    let macos_root = home.join("Library/Application Support/com.mitchellh.ghostty");
    // Ghostty uses XDG_CONFIG_HOME on every platform (including macOS), with
    // ~/.config as its fallback. Platform config_dir() is different on macOS.
    let xdg_config_home = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map_or_else(|| home.join(".config"), PathBuf::from);
    let xdg_root = xdg_config_home.join("ghostty");
    let roots = vec![macos_root.clone(), xdg_root.clone()];
    // Ghostty loads XDG first and macOS-specific files afterwards. Support
    // both the current and pre-1.2.3 filenames.
    let configs = [
        xdg_root.join("config.ghostty"),
        xdg_root.join("config"),
        macos_root.join("config.ghostty"),
        macos_root.join("config"),
    ]
    .into_iter()
    .filter(|path| path.is_file())
    .collect();
    Some((roots, configs))
}

#[derive(Debug, thiserror::Error)]
pub enum ThemeError {
    #[error("could not read Ghostty configuration {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

fn parse_theme_file(
    path: &Path,
    roots: &[PathBuf],
    visited: &mut HashSet<PathBuf>,
    theme: &mut GhosttyTheme,
) -> Result<(), ThemeError> {
    let identity = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if !visited.insert(identity) {
        return Ok(());
    }
    let contents = read_config(path)?;
    let entries = contents
        .lines()
        .filter_map(parse_config_entry)
        .collect::<Vec<_>>();

    // Resolve themes first so explicit values in the user's config win even
    // when the `theme` line appears after them.
    for (key, value) in &entries {
        if key == "theme"
            && let Some(name) = dark_theme_name(value)
            && let Some(theme_path) = resolve_theme_path(path, &name, roots)
        {
            parse_theme_file(&theme_path, roots, visited, theme)?;
        }
    }
    for (key, value) in entries {
        match key.as_str() {
            "background" => theme.background = parse_rgb(&value),
            "foreground" => theme.foreground = parse_rgb(&value),
            "cursor-color" => theme.cursor = parse_rgb(&value),
            "palette" => {
                if let Some((index, color)) = parse_palette_entry(&value) {
                    theme.palette[usize::from(index)] = Some(color);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn read_config(path: &Path) -> Result<String, ThemeError> {
    fs::read_to_string(path).map_err(|source| ThemeError::Read {
        path: path.to_path_buf(),
        source,
    })
}

fn parse_font_config(contents: &str, families: &mut Vec<String>, size: &mut Option<f32>) {
    for (key, value) in contents.lines().filter_map(parse_config_entry) {
        match key.as_str() {
            "font-family" if value.is_empty() => families.clear(),
            "font-family" => families.push(value),
            "font-size" => {
                *size = value
                    .parse::<f32>()
                    .ok()
                    .filter(|value| value.is_finite() && *value > 0.0);
            }
            _ => {}
        }
    }
}

fn parse_history_limit(contents: &str, limit: &mut usize) {
    for (key, value) in contents.lines().filter_map(parse_config_entry) {
        if key == "scrollback-limit" {
            if value.is_empty() {
                *limit = DEFAULT_HISTORY_BYTES;
            } else if let Ok(bytes) = value.replace('_', "").parse() {
                *limit = bytes;
            }
        }
    }
}

fn parse_config_entry(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (key, value) = line.split_once('=')?;
    Some((key.trim().to_owned(), unquote(value.trim()).to_owned()))
}

fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value)
}

fn dark_theme_name(value: &str) -> Option<String> {
    let value = unquote(value.trim());
    if value.is_empty() {
        return None;
    }
    value
        .split(',')
        .find_map(|part| part.trim().strip_prefix("dark:").map(str::to_owned))
        .or_else(|| value.split(',').next().map(|name| name.trim().to_owned()))
}

fn resolve_theme_path(config: &Path, name: &str, roots: &[PathBuf]) -> Option<PathBuf> {
    let requested = Path::new(name);
    if requested.is_absolute() && requested.is_file() {
        return Some(requested.to_path_buf());
    }
    let mut candidates = Vec::new();
    if let Some(parent) = config.parent() {
        candidates.push(parent.join(requested));
        candidates.push(parent.join("themes").join(requested));
    }
    for root in roots {
        candidates.push(root.join("themes").join(requested));
    }
    #[cfg(target_os = "macos")]
    candidates.push(
        PathBuf::from("/Applications/Ghostty.app/Contents/Resources/ghostty/themes")
            .join(requested),
    );
    candidates.into_iter().find(|path| path.is_file())
}

fn parse_palette_entry(value: &str) -> Option<(u8, Rgb)> {
    let (index, color) = value.split_once('=')?;
    Some((index.trim().parse().ok()?, parse_rgb(color.trim())?))
}

fn parse_rgb(value: &str) -> Option<Rgb> {
    let hex = value.trim().strip_prefix('#').unwrap_or(value.trim());
    if hex.len() != 6 {
        return None;
    }
    Some(Rgb {
        r: u8::from_str_radix(&hex[0..2], 16).ok()?,
        g: u8::from_str_radix(&hex[2..4], 16).ok()?,
        b: u8::from_str_radix(&hex[4..6], 16).ok()?,
    })
}

#[cfg(feature = "link")]
mod png_decode;

#[cfg(feature = "link")]
mod linked {
    use std::collections::HashMap;
    use std::ffi::c_void;
    use std::ptr::{self, NonNull};
    use std::sync::{Arc, Once};
    use std::time::Duration;

    use mux_terminal::{
        CellStyle, CellWidth, CursorStyle, EngineDescriptor, ImageLayer, ImagePixels,
        KITTY_KEYBOARD_RESET_SEQUENCE, MouseShape, ProgramNeed, ProgramState, ProgramStatus,
        RenderCell, RenderCursor, RenderDirty, RenderFrame, RenderImage, RenderRow, Rgb,
        SemanticContent, TerminalAttachment, TerminalCheckpoint, TerminalEngine, TerminalError,
        TerminalInteraction, TerminalKey, TerminalKeyAction, TerminalKeyEvent, TerminalModifiers,
        TerminalMouseAction, TerminalMouseButton, TerminalMouseEvent, TerminalRenderer,
        TerminalScrollState, TerminalSelection, TerminalSelectionAutoscroll,
        TerminalSelectionGestureEvent, TerminalSelectionGestureStatus, TerminalSize,
        TerminalViewportScroll,
    };
    use thiserror::Error;

    use crate::{GHOSTTY_REVISION, GhosttyTheme};

    const SUCCESS: i32 = 0;
    const CELL_BOLD: u16 = 1 << 0;
    const CELL_ITALIC: u16 = 1 << 1;
    const CELL_FAINT: u16 = 1 << 2;
    const CELL_BLINK: u16 = 1 << 3;
    const CELL_INVERSE: u16 = 1 << 4;
    const CELL_INVISIBLE: u16 = 1 << 5;
    const CELL_STRIKETHROUGH: u16 = 1 << 6;
    const CELL_OVERLINE: u16 = 1 << 7;
    const KEY_BUFFER_SIZE: usize = 4_096;
    const MOUSE_BUFFER_SIZE: usize = 128;

    #[derive(Clone, Copy)]
    #[repr(C)]
    struct CRgb {
        r: u8,
        g: u8,
        b: u8,
    }

    #[derive(Clone, Copy, Default)]
    #[repr(C)]
    struct CSelectionGestureStatus {
        has_selection: u8,
        dragged: u8,
        click_count: u8,
        autoscroll: u8,
    }

    #[derive(Clone, Copy)]
    #[repr(C)]
    struct CRenderCell {
        text_offset: u32,
        text_len: u32,
        hyperlink_offset: u32,
        hyperlink_len: u32,
        foreground: CRgb,
        background: CRgb,
        underline_color: CRgb,
        flags: u16,
        underline: u8,
        width: u8,
        semantic: u8,
        selected: u8,
    }

    #[derive(Clone, Copy)]
    #[repr(C)]
    struct CRenderRow {
        wrapped: u8,
        continuation: u8,
        dirty: u8,
        rebuilt: u8,
    }

    #[repr(C)]
    struct CRenderFrame {
        cols: u16,
        rows: u16,
        dirty: u8,
        background: CRgb,
        foreground: CRgb,
        cursor_has_value: u8,
        cursor_visible: u8,
        cursor_blinking: u8,
        cursor_style: u8,
        cursor_x: u16,
        cursor_y: u16,
        cursor_color: CRgb,
        scroll_total: u64,
        scroll_offset: u64,
        scroll_len: u64,
        row_metadata: *mut CRenderRow,
        cells: *mut CRenderCell,
        text: *mut u8,
        text_len: usize,
        redraw_in_ms: u32,
        held: u8,
        images_changed: u8,
    }

    #[derive(Clone, Copy)]
    #[repr(C)]
    struct CImagePlacement {
        image_generation: u64,
        image_id: u32,
        z: i32,
        col: i32,
        row: i32,
        offset_x: u32,
        offset_y: u32,
        width: u32,
        height: u32,
        source_x: u32,
        source_y: u32,
        source_width: u32,
        source_height: u32,
        layer: u8,
    }

    #[repr(C)]
    struct CImagePixels {
        generation: u64,
        data: *const u8,
        len: usize,
        width: u32,
        height: u32,
        format: u8,
    }

    #[repr(C)]
    struct CProgramStatus {
        state: u8,
        kind: u8,
        progress: i8,
        app_len: u8,
        title_len: u8,
        message_len: u16,
        app: [u8; 32],
        title: [u8; 192],
        message: [u8; 512],
    }

    impl Default for CImagePixels {
        fn default() -> Self {
            Self {
                generation: 0,
                data: ptr::null(),
                len: 0,
                width: 0,
                height: 0,
                format: 0,
            }
        }
    }

    impl Default for CRenderFrame {
        fn default() -> Self {
            Self {
                cols: 0,
                rows: 0,
                dirty: 0,
                background: CRgb { r: 0, g: 0, b: 0 },
                foreground: CRgb { r: 0, g: 0, b: 0 },
                cursor_has_value: 0,
                cursor_visible: 0,
                cursor_blinking: 0,
                cursor_style: 0,
                cursor_x: 0,
                cursor_y: 0,
                cursor_color: CRgb { r: 0, g: 0, b: 0 },
                scroll_total: 0,
                scroll_offset: 0,
                scroll_len: 0,
                row_metadata: ptr::null_mut(),
                cells: ptr::null_mut(),
                text: ptr::null_mut(),
                text_len: 0,
                redraw_in_ms: 0,
                held: 0,
                images_changed: 0,
            }
        }
    }

    /// Kitty image pixel formats, as libghostty stores them.
    const IMAGE_FORMAT_RGB: u8 = 0;
    const IMAGE_FORMAT_RGBA: u8 = 1;
    const IMAGE_FORMAT_GRAY_ALPHA: u8 = 3;
    const IMAGE_FORMAT_GRAY: u8 = 4;

    /// libghostty's answer when there is no such value.
    const NO_VALUE: i32 = -4;

    unsafe extern "C" {
        fn mux_ghostty_terminal_new(cols: u16, rows: u16, out_terminal: *mut *mut c_void) -> i32;
        fn mux_ghostty_terminal_restore(
            snapshot: *const u8,
            snapshot_len: usize,
            out_terminal: *mut *mut c_void,
        ) -> i32;
        fn mux_ghostty_terminal_free(terminal: *mut c_void);
        fn mux_ghostty_terminal_apply_theme(
            terminal: *mut c_void,
            background: *const CRgb,
            foreground: *const CRgb,
            cursor: *const CRgb,
            palette_indices: *const u8,
            palette_colors: *const CRgb,
            palette_len: usize,
        ) -> i32;
        fn mux_ghostty_terminal_write(terminal: *mut c_void, bytes: *const u8, len: usize);
        fn mux_ghostty_terminal_kitty_keyboard_flags(
            terminal: *mut c_void,
            out_flags: *mut u8,
        ) -> i32;
        fn mux_ghostty_terminal_resize(
            terminal: *mut c_void,
            cols: u16,
            rows: u16,
            cell_width_px: u32,
            cell_height_px: u32,
        ) -> i32;
        fn mux_ghostty_terminal_set_selection(
            terminal: *mut c_void,
            anchor_x: u16,
            anchor_y: u16,
            focus_x: u16,
            focus_y: u16,
            rectangular: bool,
        ) -> i32;
        fn mux_ghostty_terminal_clear_selection(terminal: *mut c_void) -> i32;
        fn mux_ghostty_terminal_select_rows(terminal: *mut c_void, first: u32, last: u32) -> i32;
        fn mux_ghostty_selection_gesture_new(out_gesture: *mut *mut c_void) -> i32;
        fn mux_ghostty_selection_gesture_free(gesture: *mut c_void, terminal: *mut c_void);
        fn mux_ghostty_selection_gesture_reset(gesture: *mut c_void, terminal: *mut c_void) -> i32;
        fn mux_ghostty_selection_gesture_press(
            gesture: *mut c_void,
            terminal: *mut c_void,
            x: u16,
            y: u16,
            surface_x: f64,
            surface_y: f64,
            time_ns: u64,
            repeat_distance: f64,
            repeat_interval_ns: u64,
            out_status: *mut CSelectionGestureStatus,
        ) -> i32;
        fn mux_ghostty_selection_gesture_drag(
            gesture: *mut c_void,
            terminal: *mut c_void,
            x: u16,
            y: u16,
            surface_x: f64,
            surface_y: f64,
            rectangular: bool,
            columns: u32,
            cell_width: u32,
            padding_left: u32,
            screen_height: u32,
            out_status: *mut CSelectionGestureStatus,
        ) -> i32;
        fn mux_ghostty_selection_gesture_release(
            gesture: *mut c_void,
            terminal: *mut c_void,
            has_point: bool,
            x: u16,
            y: u16,
            out_status: *mut CSelectionGestureStatus,
        ) -> i32;
        fn mux_ghostty_selection_gesture_autoscroll_tick(
            gesture: *mut c_void,
            terminal: *mut c_void,
            viewport_x: u16,
            viewport_y: u16,
            surface_x: f64,
            surface_y: f64,
            rectangular: bool,
            columns: u32,
            cell_width: u32,
            padding_left: u32,
            screen_height: u32,
            out_status: *mut CSelectionGestureStatus,
        ) -> i32;
        fn mux_ghostty_terminal_selected_text(
            terminal: *mut c_void,
            out_bytes: *mut *mut u8,
            out_len: *mut usize,
            out_has_selection: *mut bool,
        ) -> i32;
        fn mux_ghostty_terminal_set_history_limit(terminal: *mut c_void, max_bytes: usize) -> i32;
        fn mux_ghostty_terminal_screen_text(
            terminal: *mut c_void,
            out_bytes: *mut *mut u8,
            out_len: *mut usize,
        ) -> i32;
        fn mux_ghostty_terminal_encode_paste(
            terminal: *mut c_void,
            bytes: *const u8,
            len: usize,
            out_bytes: *mut *mut u8,
            out_len: *mut usize,
        ) -> i32;
        fn mux_ghostty_buffer_free(bytes: *mut u8);
        fn mux_ghostty_terminal_row_text(
            terminal: *mut c_void,
            y: u32,
            out_bytes: *mut *mut u8,
            out_len: *mut usize,
        ) -> i32;
        fn mux_ghostty_terminal_prompt_rows(
            terminal: *mut c_void,
            out_rows: *mut *mut u32,
            out_len: *mut usize,
        ) -> i32;
        fn mux_ghostty_terminal_scroll_viewport(terminal: *mut c_void, tag: u8, value: i64);
        fn mux_ghostty_key_encoder_new(out_encoder: *mut *mut c_void) -> i32;
        fn mux_ghostty_key_encoder_free(encoder: *mut c_void);
        fn mux_ghostty_key_encoder_encode(
            encoder: *mut c_void,
            terminal: *mut c_void,
            action: u8,
            key_tag: u8,
            function_number: u8,
            modifiers: u16,
            consumed_modifiers: u16,
            utf8: *const u8,
            utf8_len: usize,
            unshifted_codepoint: u32,
            composing: bool,
            out_bytes: *mut u8,
            out_capacity: usize,
            out_len: *mut usize,
        ) -> i32;
        fn mux_ghostty_mouse_encoder_new(out_encoder: *mut *mut c_void) -> i32;
        fn mux_ghostty_mouse_encoder_free(encoder: *mut c_void);
        fn mux_ghostty_mouse_encoder_encode(
            encoder: *mut c_void,
            terminal: *mut c_void,
            action: u8,
            button: u8,
            modifiers: u16,
            x: f32,
            y: f32,
            screen_width: u32,
            screen_height: u32,
            cell_width: u32,
            cell_height: u32,
            padding_top: u32,
            padding_bottom: u32,
            padding_right: u32,
            padding_left: u32,
            any_button_pressed: bool,
            out_bytes: *mut u8,
            out_capacity: usize,
            out_len: *mut usize,
        ) -> i32;
        fn mux_ghostty_terminal_snapshot(
            terminal: *mut c_void,
            out_bytes: *mut *mut u8,
            out_len: *mut usize,
        ) -> i32;
        fn mux_ghostty_snapshot_free(bytes: *mut u8, len: usize);
        fn mux_ghostty_renderer_new(out_renderer: *mut *mut c_void) -> i32;
        fn mux_ghostty_renderer_free(renderer: *mut c_void);
        fn mux_ghostty_renderer_frame(
            renderer: *mut c_void,
            terminal: *mut c_void,
            keep_clean_rows: bool,
            out_frame: *mut CRenderFrame,
        ) -> i32;
        fn mux_ghostty_render_frame_free(frame: *mut CRenderFrame);
        fn mux_ghostty_response_collector_new(
            cols: u16,
            rows: u16,
            out_collector: *mut *mut c_void,
        ) -> i32;
        fn mux_ghostty_response_collector_free(collector: *mut c_void);
        fn mux_ghostty_response_collector_set_size(
            collector: *mut c_void,
            cols: u16,
            rows: u16,
            cell_width_px: u32,
            cell_height_px: u32,
        );
        fn mux_ghostty_response_collector_peek(
            collector: *mut c_void,
            out_bytes: *mut *const u8,
            out_len: *mut usize,
        ) -> i32;
        fn mux_ghostty_response_collector_clear(collector: *mut c_void);
        fn mux_ghostty_terminal_enable_responses(
            terminal: *mut c_void,
            collector: *mut c_void,
            renderer: *mut c_void,
        ) -> i32;
        fn mux_ghostty_set_png_decoder(decoder: Option<crate::png_decode::DecodeFn>) -> i32;
        fn mux_ghostty_renderer_release_hold(renderer: *mut c_void);
        fn mux_ghostty_response_collector_take_program_status(
            collector: *mut c_void,
            out_status: *mut CProgramStatus,
        ) -> bool;
        fn mux_ghostty_terminal_mouse_shape(terminal: *mut c_void, out_shape: *mut u8) -> i32;
        fn mux_ghostty_terminal_on_alternate_screen(
            terminal: *mut c_void,
            out_alternate: *mut bool,
        ) -> i32;
        fn mux_ghostty_terminal_compress_step(terminal: *mut c_void, out_pending: *mut bool)
        -> i32;
        fn mux_ghostty_terminal_memory_bytes(terminal: *mut c_void, out_bytes: *mut u64) -> i32;
        fn mux_ghostty_renderer_images(
            renderer: *mut c_void,
            out_placements: *mut *const CImagePlacement,
            out_len: *mut usize,
        ) -> i32;
        fn mux_ghostty_terminal_image_pixels(
            terminal: *mut c_void,
            image_id: u32,
            out_pixels: *mut CImagePixels,
        ) -> i32;
    }

    pub struct GhosttyEngine {
        terminal: NonNull<c_void>,
        renderer: NonNull<c_void>,
        responses: NonNull<c_void>,
        key_encoder: NonNull<c_void>,
        mouse_encoder: NonNull<c_void>,
        selection_gesture: NonNull<c_void>,
        descriptor: EngineDescriptor,
        next_sequence: u64,
        /// The frame the renderer last filled, so the next fill of that same
        /// frame only has to rewrite the rows that changed.
        filled: Option<FilledFrame>,
        /// The images on screen as of the last frame, kept until they move.
        images: Vec<RenderImage>,
        /// The pixels of those images, by image ID, read once per change.
        image_pixels: HashMap<u32, Arc<ImagePixels>>,
    }

    /// Where a filled frame's cells live and its shape. A frame is only
    /// updated in place when it is still exactly this one: a fresh frame, a
    /// copy, or one resized since has its rows rebuilt in full.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct FilledFrame {
        cells: usize,
        len: usize,
        cols: u16,
        rows: u16,
    }

    impl FilledFrame {
        fn of(frame: &RenderFrame) -> Self {
            Self {
                cells: frame.cells.as_ptr().addr(),
                len: frame.cells.len(),
                cols: frame.cols,
                rows: frame.rows,
            }
        }
    }

    // SAFETY: libghostty-vt terminals contain no thread-affine platform
    // handles. The wrapper owns the sole handle and all mutation requires
    // `&mut self`; callers additionally serialize it behind the pane lock.
    unsafe impl Send for GhosttyEngine {}

    impl GhosttyEngine {
        /// Select everything the terminal holds, its history and the screen,
        /// as Cmd+A does in a Mac terminal.
        pub fn select_all(&mut self) -> Result<(), TerminalError> {
            self.select_rows(0, usize::MAX)
        }

        /// Select whole rows, `first` to `last`, counted from the top of the
        /// history as `screen_text`'s lines are.
        pub fn select_rows(&mut self, first: usize, last: usize) -> Result<(), TerminalError> {
            // SAFETY: both handles are exclusively borrowed; resetting first
            // keeps a later click from continuing an earlier click sequence.
            let reset = unsafe {
                mux_ghostty_selection_gesture_reset(
                    self.selection_gesture.as_ptr(),
                    self.terminal.as_ptr(),
                )
            };
            check(reset).map_err(|error| TerminalError::Engine(error.to_string()))?;
            let row = |row: usize| u32::try_from(row).unwrap_or(u32::MAX);
            // SAFETY: the terminal is exclusively borrowed for this call.
            let result = unsafe {
                mux_ghostty_terminal_select_rows(self.terminal.as_ptr(), row(first), row(last))
            };
            check(result).map_err(|error| TerminalError::Engine(error.to_string()))
        }

        pub fn new(size: TerminalSize) -> Result<Self, GhosttyError> {
            let size = size.validate()?;
            let mut terminal = ptr::null_mut();
            // SAFETY: `terminal` is a valid out pointer and dimensions were validated.
            let result =
                unsafe { mux_ghostty_terminal_new(size.cols, size.rows, &raw mut terminal) };
            check(result)?;
            let terminal = NonNull::new(terminal).ok_or(GhosttyError::NullTerminal)?;
            let renderer = match create_renderer() {
                Ok(renderer) => renderer,
                Err(error) => {
                    // SAFETY: the terminal was just created and is solely owned here.
                    unsafe { mux_ghostty_terminal_free(terminal.as_ptr()) };
                    return Err(error);
                }
            };
            let responses = match create_responses(terminal, renderer, size) {
                Ok(responses) => responses,
                Err(error) => {
                    // SAFETY: both handles are solely owned here.
                    unsafe {
                        mux_ghostty_renderer_free(renderer.as_ptr());
                        mux_ghostty_terminal_free(terminal.as_ptr());
                    }
                    return Err(error);
                }
            };
            let key_encoder = match create_key_encoder() {
                Ok(key_encoder) => key_encoder,
                Err(error) => {
                    // SAFETY: all handles are solely owned here.
                    unsafe {
                        mux_ghostty_response_collector_free(responses.as_ptr());
                        mux_ghostty_renderer_free(renderer.as_ptr());
                        mux_ghostty_terminal_free(terminal.as_ptr());
                    }
                    return Err(error);
                }
            };
            let mouse_encoder = match create_mouse_encoder() {
                Ok(mouse_encoder) => mouse_encoder,
                Err(error) => {
                    // SAFETY: all handles are solely owned here.
                    unsafe {
                        mux_ghostty_key_encoder_free(key_encoder.as_ptr());
                        mux_ghostty_response_collector_free(responses.as_ptr());
                        mux_ghostty_renderer_free(renderer.as_ptr());
                        mux_ghostty_terminal_free(terminal.as_ptr());
                    }
                    return Err(error);
                }
            };
            let selection_gesture = match create_selection_gesture() {
                Ok(selection_gesture) => selection_gesture,
                Err(error) => {
                    // SAFETY: all handles are solely owned here.
                    unsafe {
                        mux_ghostty_mouse_encoder_free(mouse_encoder.as_ptr());
                        mux_ghostty_key_encoder_free(key_encoder.as_ptr());
                        mux_ghostty_response_collector_free(responses.as_ptr());
                        mux_ghostty_renderer_free(renderer.as_ptr());
                        mux_ghostty_terminal_free(terminal.as_ptr());
                    }
                    return Err(error);
                }
            };
            Ok(Self {
                terminal,
                renderer,
                responses,
                key_encoder,
                mouse_encoder,
                selection_gesture,
                descriptor: descriptor(),
                next_sequence: 1,
                filled: None,
                images: Vec::new(),
                image_pixels: HashMap::new(),
            })
        }

        pub fn new_with_theme(
            size: TerminalSize,
            theme: &GhosttyTheme,
        ) -> Result<Self, GhosttyError> {
            let mut engine = Self::new(size)?;
            engine.apply_theme(theme)?;
            Ok(engine)
        }

        pub fn apply_theme(&mut self, theme: &GhosttyTheme) -> Result<(), GhosttyError> {
            let background = theme.background.map(CRgb::from);
            let foreground = theme.foreground.map(CRgb::from);
            let cursor = theme.cursor.map(CRgb::from);
            let (indices, colors): (Vec<u8>, Vec<CRgb>) = theme
                .palette
                .iter()
                .enumerate()
                .filter_map(|(index, color)| {
                    color.map(|color| {
                        (
                            u8::try_from(index).expect("palette index"),
                            CRgb::from(color),
                        )
                    })
                })
                .unzip();
            // SAFETY: all optional pointers and palette slices remain valid for
            // the duration of this synchronous call.
            check(unsafe {
                mux_ghostty_terminal_apply_theme(
                    self.terminal.as_ptr(),
                    background.as_ref().map_or(ptr::null(), std::ptr::from_ref),
                    foreground.as_ref().map_or(ptr::null(), std::ptr::from_ref),
                    cursor.as_ref().map_or(ptr::null(), std::ptr::from_ref),
                    indices.as_ptr(),
                    colors.as_ptr(),
                    indices.len(),
                )
            })
        }

        pub fn restore(checkpoint: &TerminalCheckpoint) -> Result<Self, GhosttyError> {
            if checkpoint.descriptor != descriptor() {
                return Err(GhosttyError::IncompatibleCheckpoint);
            }
            let mut terminal = ptr::null_mut();
            // SAFETY: the snapshot slice remains alive for the synchronous decode.
            let result = unsafe {
                mux_ghostty_terminal_restore(
                    checkpoint.payload.as_ptr(),
                    checkpoint.payload.len(),
                    &raw mut terminal,
                )
            };
            check(result)?;
            let terminal = NonNull::new(terminal).ok_or(GhosttyError::NullTerminal)?;
            let renderer = match create_renderer() {
                Ok(renderer) => renderer,
                Err(error) => {
                    // SAFETY: the restored terminal is solely owned here.
                    unsafe { mux_ghostty_terminal_free(terminal.as_ptr()) };
                    return Err(error);
                }
            };
            let responses = match create_responses(terminal, renderer, TerminalSize::default()) {
                Ok(responses) => responses,
                Err(error) => {
                    // SAFETY: both handles are solely owned here.
                    unsafe {
                        mux_ghostty_renderer_free(renderer.as_ptr());
                        mux_ghostty_terminal_free(terminal.as_ptr());
                    }
                    return Err(error);
                }
            };
            let key_encoder = match create_key_encoder() {
                Ok(key_encoder) => key_encoder,
                Err(error) => {
                    // SAFETY: all handles are solely owned here.
                    unsafe {
                        mux_ghostty_response_collector_free(responses.as_ptr());
                        mux_ghostty_renderer_free(renderer.as_ptr());
                        mux_ghostty_terminal_free(terminal.as_ptr());
                    }
                    return Err(error);
                }
            };
            let mouse_encoder = match create_mouse_encoder() {
                Ok(mouse_encoder) => mouse_encoder,
                Err(error) => {
                    // SAFETY: all handles are solely owned here.
                    unsafe {
                        mux_ghostty_key_encoder_free(key_encoder.as_ptr());
                        mux_ghostty_response_collector_free(responses.as_ptr());
                        mux_ghostty_renderer_free(renderer.as_ptr());
                        mux_ghostty_terminal_free(terminal.as_ptr());
                    }
                    return Err(error);
                }
            };
            let selection_gesture = match create_selection_gesture() {
                Ok(selection_gesture) => selection_gesture,
                Err(error) => {
                    // SAFETY: all handles are solely owned here.
                    unsafe {
                        mux_ghostty_mouse_encoder_free(mouse_encoder.as_ptr());
                        mux_ghostty_key_encoder_free(key_encoder.as_ptr());
                        mux_ghostty_response_collector_free(responses.as_ptr());
                        mux_ghostty_renderer_free(renderer.as_ptr());
                        mux_ghostty_terminal_free(terminal.as_ptr());
                    }
                    return Err(error);
                }
            };
            Ok(Self {
                terminal,
                renderer,
                responses,
                key_encoder,
                mouse_encoder,
                selection_gesture,
                descriptor: descriptor(),
                next_sequence: checkpoint.next_sequence,
                filled: None,
                images: Vec::new(),
                image_pixels: HashMap::new(),
            })
        }

        /// Keep up to `max_bytes` of history above the screen. libghostty
        /// prunes a page at a time, so what is kept lands within a page
        /// (about 400 KB) of the limit.
        pub fn set_history_limit(&mut self, max_bytes: usize) -> Result<(), GhosttyError> {
            // SAFETY: the terminal is owned and the limit is passed by value.
            check(unsafe {
                mux_ghostty_terminal_set_history_limit(self.terminal.as_ptr(), max_bytes)
            })
        }

        /// The whole screen as plain text, history first: one line per row,
        /// so a line's index is its row counted from the top of the history.
        /// Rows past the last one with text may be left out.
        pub fn screen_text(&self) -> Result<String, GhosttyError> {
            let mut bytes = ptr::null_mut();
            let mut length = 0_usize;
            // SAFETY: output pointers are valid; the returned allocation is
            // copied before it is released with the matching shim function.
            let result = unsafe {
                mux_ghostty_terminal_screen_text(
                    self.terminal.as_ptr(),
                    &raw mut bytes,
                    &raw mut length,
                )
            };
            check(result)?;
            let text =
                String::from_utf8_lossy(borrowed_slice(bytes.cast_const(), length, "screen text")?)
                    .into_owned();
            // SAFETY: the pointer is the allocation returned by the shim, and
            // freeing null for an empty screen is supported.
            unsafe { mux_ghostty_buffer_free(bytes) };
            Ok(text)
        }

        /// One row's text, counted from the top of the history as
        /// `screen_text`'s lines are, its trailing blanks left off.
        pub fn row_text(&self, row: usize) -> Result<String, GhosttyError> {
            let mut bytes = ptr::null_mut();
            let mut length = 0_usize;
            let row = u32::try_from(row).unwrap_or(u32::MAX);
            // SAFETY: output pointers are valid; the returned allocation is
            // copied before it is released with the matching shim function.
            let result = unsafe {
                mux_ghostty_terminal_row_text(
                    self.terminal.as_ptr(),
                    row,
                    &raw mut bytes,
                    &raw mut length,
                )
            };
            check(result)?;
            let text =
                String::from_utf8_lossy(borrowed_slice(bytes.cast_const(), length, "row text")?)
                    .trim_end()
                    .to_owned();
            // SAFETY: the pointer is the allocation returned by the shim.
            unsafe { mux_ghostty_buffer_free(bytes) };
            Ok(text)
        }

        /// The rows where the shell's prompts begin, as it marked them with
        /// OSC 133, counted from the top of the history as `screen_text`'s
        /// lines are. Empty when the shell marks none.
        pub fn prompt_rows(&self) -> Result<Vec<usize>, GhosttyError> {
            let mut rows = ptr::null_mut();
            let mut length = 0_usize;
            // SAFETY: output pointers are valid; the returned allocation is
            // copied before it is released with the matching shim function.
            let result = unsafe {
                mux_ghostty_terminal_prompt_rows(
                    self.terminal.as_ptr(),
                    &raw mut rows,
                    &raw mut length,
                )
            };
            check(result)?;
            let found = if rows.is_null() || length == 0 {
                Vec::new()
            } else {
                // SAFETY: the shim returned `length` initialised rows.
                unsafe { std::slice::from_raw_parts(rows, length) }
                    .iter()
                    .map(|&row| row as usize)
                    .collect()
            };
            // SAFETY: the pointer is the allocation returned by the shim, and
            // freeing null is supported.
            unsafe { mux_ghostty_buffer_free(rows.cast()) };
            Ok(found)
        }

        fn checkpoint(&self) -> Result<TerminalCheckpoint, GhosttyError> {
            let mut bytes = ptr::null_mut();
            let mut length = 0_usize;
            // SAFETY: output pointers are valid and the returned allocation is
            // copied and released with the matching libghostty allocator.
            let result = unsafe {
                mux_ghostty_terminal_snapshot(
                    self.terminal.as_ptr(),
                    &raw mut bytes,
                    &raw mut length,
                )
            };
            check(result)?;
            let payload = if length == 0 {
                Vec::new()
            } else {
                let bytes = NonNull::new(bytes).ok_or(GhosttyError::NullSnapshot)?;
                // SAFETY: libghostty returned `length` initialized bytes.
                let payload = unsafe {
                    std::slice::from_raw_parts(bytes.as_ptr().cast_const(), length).to_vec()
                };
                // SAFETY: the pointer and length are exactly those returned above.
                unsafe { mux_ghostty_snapshot_free(bytes.as_ptr(), length) };
                payload
            };
            Ok(TerminalCheckpoint {
                descriptor: self.descriptor.clone(),
                next_sequence: self.next_sequence,
                payload,
            })
        }
    }

    impl TerminalEngine for GhosttyEngine {
        fn descriptor(&self) -> &EngineDescriptor {
            &self.descriptor
        }

        fn screen_text(&self) -> Option<String> {
            Self::screen_text(self).ok()
        }

        fn compress_history_step(&mut self) -> bool {
            Self::compress_history_step(self).unwrap_or(false)
        }

        fn next_output_sequence(&self) -> u64 {
            self.next_sequence
        }

        fn apply_output(&mut self, sequence: u64, bytes: &[u8]) -> Result<(), TerminalError> {
            if sequence != self.next_sequence {
                return Err(TerminalError::OutOfOrder {
                    expected: self.next_sequence,
                    actual: sequence,
                });
            }
            // SAFETY: the owned terminal is valid and the byte slice is borrowed
            // only for this synchronous call.
            unsafe {
                mux_ghostty_terminal_write(self.terminal.as_ptr(), bytes.as_ptr(), bytes.len());
            }
            self.next_sequence += 1;
            Ok(())
        }

        fn resize(&mut self, size: TerminalSize) -> Result<(), TerminalError> {
            let size = size.validate()?;
            // SAFETY: the terminal is owned and dimensions were validated.
            let result = unsafe {
                mux_ghostty_terminal_resize(
                    self.terminal.as_ptr(),
                    size.cols,
                    size.rows,
                    size.cell_width_px,
                    size.cell_height_px,
                )
            };
            check(result).map_err(|error| TerminalError::Engine(error.to_string()))?;
            // A held frame has the old size; the new one is drawn at once.
            self.release_hold();
            // SAFETY: the response collector is owned and dimensions are validated.
            unsafe {
                mux_ghostty_response_collector_set_size(
                    self.responses.as_ptr(),
                    size.cols,
                    size.rows,
                    size.cell_width_px,
                    size.cell_height_px,
                );
            }
            Ok(())
        }

        fn attachment(&self) -> Result<TerminalAttachment, TerminalError> {
            let checkpoint = self
                .checkpoint()
                .map_err(|error| TerminalError::Engine(error.to_string()))?;
            Ok(TerminalAttachment {
                descriptor: self.descriptor.clone(),
                checkpoint: Some(checkpoint),
                replay: Vec::new(),
                retained_from_sequence: self.next_sequence,
                next_sequence: self.next_sequence,
            })
        }

        fn take_pty_responses(&mut self) -> Result<Vec<u8>, TerminalError> {
            let mut bytes = ptr::null();
            let mut length = 0_usize;
            // SAFETY: both output pointers are valid for the synchronous call.
            let result = unsafe {
                mux_ghostty_response_collector_peek(
                    self.responses.as_ptr(),
                    &raw mut bytes,
                    &raw mut length,
                )
            };
            check(result).map_err(|error| TerminalError::Engine(error.to_string()))?;
            let response = borrowed_slice(bytes, length, "PTY response")?.to_vec();
            // SAFETY: clearing retains the collector allocation and invalidates no Rust borrow.
            unsafe { mux_ghostty_response_collector_clear(self.responses.as_ptr()) };
            Ok(response)
        }

        fn kitty_keyboard_flags(&self) -> Result<u8, TerminalError> {
            let mut flags = 0_u8;
            // SAFETY: the terminal and output pointer are valid for this
            // synchronous read-only query.
            let result = unsafe {
                mux_ghostty_terminal_kitty_keyboard_flags(self.terminal.as_ptr(), &raw mut flags)
            };
            check(result).map_err(|error| TerminalError::Engine(error.to_string()))?;
            Ok(flags)
        }

        fn reset_kitty_keyboard(&mut self) -> Result<(), TerminalError> {
            // Setting the current progressive-enhancement flags to zero is
            // intentionally out-of-band: the daemon mirrors these same bytes
            // in the ordered output event consumed by GUI replicas.
            // SAFETY: the terminal is owned and the static slice remains valid
            // for this synchronous write.
            unsafe {
                mux_ghostty_terminal_write(
                    self.terminal.as_ptr(),
                    KITTY_KEYBOARD_RESET_SEQUENCE.as_ptr(),
                    KITTY_KEYBOARD_RESET_SEQUENCE.len(),
                );
            }
            Ok(())
        }
    }

    impl TerminalRenderer for GhosttyEngine {
        fn render_frame(&mut self) -> Result<RenderFrame, TerminalError> {
            let mut rendered = RenderFrame {
                cols: 0,
                rows: 0,
                dirty: RenderDirty::Clean,
                background: Rgb::default(),
                foreground: Rgb::default(),
                cursor: None,
                scroll: TerminalScrollState::default(),
                row_metadata: Vec::new(),
                cells: Vec::new(),
                images: Vec::new(),
                held: false,
                redraw_in: None,
            };
            self.render_frame_into(&mut rendered)?;
            Ok(rendered)
        }

        fn render_frame_into(&mut self, rendered: &mut RenderFrame) -> Result<(), TerminalError> {
            // Rows libghostty has not touched since the last fill are left as
            // they are, but only in the frame that fill wrote. Until this fill
            // succeeds, no frame holds the latest rows.
            let keep_clean_rows = self.filled.take() == Some(FilledFrame::of(rendered));
            let mut frame = CRenderFrame::default();
            // SAFETY: both handles are exclusively owned and `frame` is a valid out pointer.
            let result = unsafe {
                mux_ghostty_renderer_frame(
                    self.renderer.as_ptr(),
                    self.terminal.as_ptr(),
                    keep_clean_rows,
                    &raw mut frame,
                )
            };
            check(result).map_err(|error| TerminalError::Engine(error.to_string()))?;
            let guard = RenderFrameGuard(frame);
            convert_frame_into(&guard.0, rendered)?;
            if guard.0.images_changed != 0 {
                self.read_images()?;
            }
            rendered.images.clone_from(&self.images);
            self.filled = Some(FilledFrame::of(rendered));
            Ok(())
        }
    }

    impl GhosttyEngine {
        /// Read again where images are on screen, and the pixels of any that
        /// changed. Pixels are kept only for the images still on screen.
        fn read_images(&mut self) -> Result<(), TerminalError> {
            let mut placements = ptr::null();
            let mut len = 0_usize;
            // SAFETY: the renderer is exclusively owned and both out pointers
            // are valid; the list it returns lives until its next call.
            let result = unsafe {
                mux_ghostty_renderer_images(
                    self.renderer.as_ptr(),
                    &raw mut placements,
                    &raw mut len,
                )
            };
            check(result).map_err(|error| TerminalError::Engine(error.to_string()))?;
            let placements = borrowed_slice(placements, len, "image placements")?;
            let mut pixels = HashMap::with_capacity(self.image_pixels.len());
            self.images.clear();
            for placement in placements {
                let id = placement.image_id;
                let known = pixels
                    .get(&id)
                    .or_else(|| self.image_pixels.get(&id))
                    .filter(|known: &&Arc<ImagePixels>| {
                        known.generation == placement.image_generation
                    })
                    .cloned();
                let image = match known {
                    Some(image) => image,
                    None => match self.image_pixels(id)? {
                        Some(image) => Arc::new(image),
                        // Its pixels are still arriving.
                        None => continue,
                    },
                };
                pixels.insert(id, Arc::clone(&image));
                let Some(layer) = image_layer(placement.layer) else {
                    continue;
                };
                self.images.push(RenderImage {
                    pixels: image,
                    layer,
                    z: placement.z,
                    column: placement.col,
                    row: placement.row,
                    offset_x: placement.offset_x,
                    offset_y: placement.offset_y,
                    width: placement.width,
                    height: placement.height,
                    source_x: placement.source_x,
                    source_y: placement.source_y,
                    source_width: placement.source_width,
                    source_height: placement.source_height,
                });
            }
            self.image_pixels = pixels;
            Ok(())
        }

        /// An image's pixels as RGBA, or `None` while they are still arriving.
        fn image_pixels(&self, id: u32) -> Result<Option<ImagePixels>, TerminalError> {
            let mut pixels = CImagePixels::default();
            // SAFETY: the terminal is borrowed and not written to while the
            // pixels it lends are copied out below.
            let result = unsafe {
                mux_ghostty_terminal_image_pixels(self.terminal.as_ptr(), id, &raw mut pixels)
            };
            if result == NO_VALUE {
                return Ok(None);
            }
            check(result).map_err(|error| TerminalError::Engine(error.to_string()))?;
            let data = borrowed_slice(pixels.data, pixels.len, "image pixels")?;
            let count = usize::try_from(u64::from(pixels.width) * u64::from(pixels.height))
                .map_err(|_| TerminalError::Engine("image too large".to_owned()))?;
            let channels = match pixels.format {
                IMAGE_FORMAT_RGB => 3,
                IMAGE_FORMAT_RGBA => 4,
                IMAGE_FORMAT_GRAY_ALPHA => 2,
                IMAGE_FORMAT_GRAY => 1,
                other => return Err(invalid_render_value("image format", other)),
            };
            if data.len() != count * channels {
                return Err(TerminalError::Engine(format!(
                    "libghostty lent {} bytes for a {}x{} image",
                    data.len(),
                    pixels.width,
                    pixels.height
                )));
            }
            let rgba = match pixels.format {
                IMAGE_FORMAT_RGBA => data.to_vec(),
                IMAGE_FORMAT_RGB => data
                    .chunks_exact(3)
                    .flat_map(|pixel| [pixel[0], pixel[1], pixel[2], u8::MAX])
                    .collect(),
                IMAGE_FORMAT_GRAY_ALPHA => data
                    .chunks_exact(2)
                    .flat_map(|pixel| [pixel[0], pixel[0], pixel[0], pixel[1]])
                    .collect(),
                _ => data
                    .iter()
                    .flat_map(|&gray| [gray, gray, gray, u8::MAX])
                    .collect(),
            };
            Ok(Some(ImagePixels {
                id,
                generation: pixels.generation,
                width: pixels.width,
                height: pixels.height,
                rgba,
            }))
        }

        /// What the program last said it is doing (OSC 7501), if it has
        /// said anything since this was last asked.
        pub fn take_program_status(&mut self) -> Option<ProgramStatus> {
            // SAFETY: an all-zero report is a valid value for the C struct.
            let mut status: CProgramStatus = unsafe { std::mem::zeroed() };
            // SAFETY: the collector is exclusively owned and the out pointer valid.
            let taken = unsafe {
                mux_ghostty_response_collector_take_program_status(
                    self.responses.as_ptr(),
                    &raw mut status,
                )
            };
            if !taken {
                return None;
            }
            let text = |bytes: &[u8], len: usize| {
                String::from_utf8_lossy(&bytes[..len.min(bytes.len())]).into_owned()
            };
            Some(ProgramStatus {
                state: match status.state {
                    0 => ProgramState::Idle,
                    1 => ProgramState::Working,
                    2 => ProgramState::Done,
                    3 => ProgramState::Blocked,
                    4 => ProgramState::Error,
                    5 => ProgramState::Clear,
                    _ => return None,
                },
                need: match status.kind {
                    1 => Some(ProgramNeed::Permission),
                    2 => Some(ProgramNeed::Question),
                    3 => Some(ProgramNeed::Auth),
                    _ => None,
                },
                progress: u8::try_from(status.progress)
                    .ok()
                    .filter(|percent| *percent <= 100),
                app: text(&status.app, status.app_len.into()),
                title: text(&status.title, status.title_len.into()),
                message: text(&status.message, status.message_len.into()),
            })
        }

        /// One bounded step of compressing the terminal's history, for a
        /// pane gone quiet. Returns whether another step has work to do.
        /// History read back later, as by scrolling, is uncompressed again.
        pub fn compress_history_step(&mut self) -> Result<bool, GhosttyError> {
            let mut pending = false;
            // SAFETY: the terminal is owned and borrowed mutably for the call.
            let result = unsafe {
                mux_ghostty_terminal_compress_step(self.terminal.as_ptr(), &raw mut pending)
            };
            check(result)?;
            Ok(pending)
        }

        /// The bytes of memory the terminal holds, history at its compressed
        /// size and images included.
        pub fn memory_bytes(&self) -> Result<u64, GhosttyError> {
            let mut bytes = 0_u64;
            // SAFETY: the terminal is borrowed for this read-only query.
            let result = unsafe {
                mux_ghostty_terminal_memory_bytes(self.terminal.as_ptr(), &raw mut bytes)
            };
            check(result)?;
            Ok(bytes)
        }

        /// Whether a full-screen program, such as an editor or a pager, has
        /// the terminal on its alternate screen.
        #[must_use]
        pub fn on_alternate_screen(&self) -> bool {
            let mut alternate = false;
            // SAFETY: the terminal is borrowed for this read-only query.
            let result = unsafe {
                mux_ghostty_terminal_on_alternate_screen(self.terminal.as_ptr(), &raw mut alternate)
            };
            result == SUCCESS && alternate
        }

        /// The pointer shape the program asked for over its pane (OSC 22).
        #[must_use]
        pub fn mouse_shape(&self) -> MouseShape {
            let mut shape = 8_u8;
            // SAFETY: the terminal is borrowed for this read-only query.
            let result =
                unsafe { mux_ghostty_terminal_mouse_shape(self.terminal.as_ptr(), &raw mut shape) };
            if result != SUCCESS {
                return MouseShape::Text;
            }
            [
                MouseShape::Default,
                MouseShape::ContextMenu,
                MouseShape::Help,
                MouseShape::Pointer,
                MouseShape::Progress,
                MouseShape::Wait,
                MouseShape::Cell,
                MouseShape::Crosshair,
                MouseShape::Text,
                MouseShape::VerticalText,
                MouseShape::Alias,
                MouseShape::Copy,
                MouseShape::Move,
                MouseShape::NoDrop,
                MouseShape::NotAllowed,
                MouseShape::Grab,
                MouseShape::Grabbing,
                MouseShape::AllScroll,
                MouseShape::ColResize,
                MouseShape::RowResize,
                MouseShape::NResize,
                MouseShape::EResize,
                MouseShape::SResize,
                MouseShape::WResize,
                MouseShape::NeResize,
                MouseShape::NwResize,
                MouseShape::SeResize,
                MouseShape::SwResize,
                MouseShape::EwResize,
                MouseShape::NsResize,
                MouseShape::NeswResize,
                MouseShape::NwseResize,
                MouseShape::ZoomIn,
                MouseShape::ZoomOut,
            ]
            .get(usize::from(shape))
            .copied()
            .unwrap_or_default()
        }

        /// Draw what the terminal holds now, even if a program has asked for
        /// the screen to keep still while it draws.
        fn release_hold(&mut self) {
            // SAFETY: the renderer is exclusively owned.
            unsafe { mux_ghostty_renderer_release_hold(self.renderer.as_ptr()) };
        }
    }

    fn image_layer(layer: u8) -> Option<ImageLayer> {
        match layer {
            0 => Some(ImageLayer::BelowBackground),
            1 => Some(ImageLayer::BelowText),
            2 => Some(ImageLayer::AboveText),
            _ => None,
        }
    }

    impl TerminalInteraction for GhosttyEngine {
        fn set_selection(
            &mut self,
            selection: Option<TerminalSelection>,
        ) -> Result<(), TerminalError> {
            // A programmatic selection replaces any in-progress pointer
            // sequence so a later click cannot accidentally continue it as a
            // double- or triple-click.
            let reset = unsafe {
                mux_ghostty_selection_gesture_reset(
                    self.selection_gesture.as_ptr(),
                    self.terminal.as_ptr(),
                )
            };
            check(reset).map_err(|error| TerminalError::Engine(error.to_string()))?;
            // SAFETY: the terminal is exclusively borrowed. The shim resolves
            // and copies viewport points during this synchronous call.
            let result = unsafe {
                selection.map_or_else(
                    || mux_ghostty_terminal_clear_selection(self.terminal.as_ptr()),
                    |selection| {
                        mux_ghostty_terminal_set_selection(
                            self.terminal.as_ptr(),
                            selection.anchor.column,
                            selection.anchor.row,
                            selection.focus.column,
                            selection.focus.row,
                            selection.rectangular,
                        )
                    },
                )
            };
            check(result).map_err(|error| TerminalError::Engine(error.to_string()))
        }

        fn selection_gesture(
            &mut self,
            event: TerminalSelectionGestureEvent,
        ) -> Result<TerminalSelectionGestureStatus, TerminalError> {
            let mut status = CSelectionGestureStatus::default();
            // SAFETY: the controller and terminal are exclusively owned by
            // this engine. All event data is copied during the call.
            let result = unsafe {
                match event {
                    TerminalSelectionGestureEvent::Press {
                        point,
                        position,
                        time_ns,
                        repeat_distance,
                        repeat_interval_ns,
                    } => mux_ghostty_selection_gesture_press(
                        self.selection_gesture.as_ptr(),
                        self.terminal.as_ptr(),
                        point.column,
                        point.row,
                        position.x,
                        position.y,
                        time_ns,
                        repeat_distance,
                        repeat_interval_ns,
                        &raw mut status,
                    ),
                    TerminalSelectionGestureEvent::Drag {
                        point,
                        position,
                        rectangular,
                        geometry,
                    } => mux_ghostty_selection_gesture_drag(
                        self.selection_gesture.as_ptr(),
                        self.terminal.as_ptr(),
                        point.column,
                        point.row,
                        position.x,
                        position.y,
                        rectangular,
                        geometry.columns,
                        geometry.cell_width,
                        geometry.padding_left,
                        geometry.screen_height,
                        &raw mut status,
                    ),
                    TerminalSelectionGestureEvent::Release { point } => {
                        mux_ghostty_selection_gesture_release(
                            self.selection_gesture.as_ptr(),
                            self.terminal.as_ptr(),
                            point.is_some(),
                            point.map_or(0, |point| point.column),
                            point.map_or(0, |point| point.row),
                            &raw mut status,
                        )
                    }
                    TerminalSelectionGestureEvent::AutoscrollTick {
                        viewport,
                        position,
                        rectangular,
                        geometry,
                    } => mux_ghostty_selection_gesture_autoscroll_tick(
                        self.selection_gesture.as_ptr(),
                        self.terminal.as_ptr(),
                        viewport.column,
                        viewport.row,
                        position.x,
                        position.y,
                        rectangular,
                        geometry.columns,
                        geometry.cell_width,
                        geometry.padding_left,
                        geometry.screen_height,
                        &raw mut status,
                    ),
                }
            };
            check(result).map_err(|error| TerminalError::Engine(error.to_string()))?;
            let autoscroll = match status.autoscroll {
                0 => TerminalSelectionAutoscroll::None,
                1 => TerminalSelectionAutoscroll::Up,
                2 => TerminalSelectionAutoscroll::Down,
                value => {
                    return Err(TerminalError::Engine(format!(
                        "libghostty returned invalid selection autoscroll state {value}"
                    )));
                }
            };
            Ok(TerminalSelectionGestureStatus {
                has_selection: status.has_selection != 0,
                dragged: status.dragged != 0,
                click_count: status.click_count,
                autoscroll,
            })
        }

        fn reset_selection_gesture(&mut self) -> Result<(), TerminalError> {
            // SAFETY: both handles are valid and exclusively borrowed.
            let result = unsafe {
                mux_ghostty_selection_gesture_reset(
                    self.selection_gesture.as_ptr(),
                    self.terminal.as_ptr(),
                )
            };
            check(result).map_err(|error| TerminalError::Engine(error.to_string()))
        }

        fn selected_text(&self) -> Result<Option<String>, TerminalError> {
            let mut bytes = ptr::null_mut();
            let mut length = 0_usize;
            let mut has_selection = false;
            // SAFETY: all output pointers are valid. Any returned allocation is
            // owned by this call and released with the matching shim function.
            let result = unsafe {
                mux_ghostty_terminal_selected_text(
                    self.terminal.as_ptr(),
                    &raw mut bytes,
                    &raw mut length,
                    &raw mut has_selection,
                )
            };
            check(result).map_err(|error| TerminalError::Engine(error.to_string()))?;
            if !has_selection {
                return Ok(None);
            }
            let text = borrowed_slice(bytes.cast_const(), length, "selected text")?.to_vec();
            // SAFETY: the pointer is the allocation returned by the shim, and
            // freeing null for an empty selection is supported.
            unsafe { mux_ghostty_buffer_free(bytes) };
            String::from_utf8(text).map(Some).map_err(|error| {
                TerminalError::Engine(format!("selected text was not UTF-8: {error}"))
            })
        }

        fn encode_paste(&self, text: &str) -> Result<Vec<u8>, TerminalError> {
            let mut bytes = ptr::null_mut();
            let mut length = 0_usize;
            // SAFETY: input is borrowed for the synchronous call; returned
            // bytes are copied before their allocation is released.
            let result = unsafe {
                mux_ghostty_terminal_encode_paste(
                    self.terminal.as_ptr(),
                    text.as_ptr(),
                    text.len(),
                    &raw mut bytes,
                    &raw mut length,
                )
            };
            check(result).map_err(|error| TerminalError::Engine(error.to_string()))?;
            let encoded = borrowed_slice(bytes.cast_const(), length, "encoded paste")?.to_vec();
            // SAFETY: the pointer is the allocation returned by the shim.
            unsafe { mux_ghostty_buffer_free(bytes) };
            Ok(encoded)
        }

        fn encode_key(&self, event: &TerminalKeyEvent) -> Result<Vec<u8>, TerminalError> {
            let (key_tag, function_number) = key_tag(event.key);
            let text = event.text.as_deref().unwrap_or_default().as_bytes();
            let mut buffer = [0_u8; KEY_BUFFER_SIZE];
            let mut length = 0_usize;
            // SAFETY: both handles are owned for the duration of this call,
            // input slices are borrowed synchronously, and the stack output
            // buffer is valid for its full reported capacity.
            let result = unsafe {
                mux_ghostty_key_encoder_encode(
                    self.key_encoder.as_ptr(),
                    self.terminal.as_ptr(),
                    key_action(event.action),
                    key_tag,
                    function_number,
                    modifier_bits(event.modifiers),
                    modifier_bits(event.consumed_modifiers),
                    text.as_ptr(),
                    text.len(),
                    event.unshifted_codepoint.map_or(0, u32::from),
                    event.composing,
                    buffer.as_mut_ptr(),
                    buffer.len(),
                    &raw mut length,
                )
            };
            check(result).map_err(|error| TerminalError::Engine(error.to_string()))?;
            if length > buffer.len() {
                return Err(TerminalError::Engine(format!(
                    "libghostty encoded an oversized key event: {length} bytes"
                )));
            }
            Ok(buffer[..length].to_vec())
        }

        fn scroll_viewport(&mut self, scroll: TerminalViewportScroll) -> Result<(), TerminalError> {
            let (tag, value) = match scroll {
                TerminalViewportScroll::Top => (0, 0),
                TerminalViewportScroll::Bottom => (1, 0),
                TerminalViewportScroll::Delta(rows) => (2, rows),
            };
            // SAFETY: the terminal is exclusively borrowed and the shim copies
            // the tagged value during this synchronous call.
            unsafe {
                mux_ghostty_terminal_scroll_viewport(self.terminal.as_ptr(), tag, value);
            }
            // Scrolling shows history as it is, not as a program held it.
            self.release_hold();
            Ok(())
        }

        fn encode_mouse(&mut self, event: &TerminalMouseEvent) -> Result<Vec<u8>, TerminalError> {
            let mut buffer = [0_u8; MOUSE_BUFFER_SIZE];
            let mut length = 0_usize;
            let geometry = event.geometry;
            // SAFETY: all handles and the stack output buffer remain valid for
            // the synchronous call; scalar geometry is copied by the shim.
            let result = unsafe {
                mux_ghostty_mouse_encoder_encode(
                    self.mouse_encoder.as_ptr(),
                    self.terminal.as_ptr(),
                    mouse_action(event.action),
                    event.button.map_or(0, mouse_button),
                    modifier_bits(event.modifiers),
                    event.x,
                    event.y,
                    geometry.screen_width,
                    geometry.screen_height,
                    geometry.cell_width,
                    geometry.cell_height,
                    geometry.padding_top,
                    geometry.padding_bottom,
                    geometry.padding_right,
                    geometry.padding_left,
                    event.any_button_pressed,
                    buffer.as_mut_ptr(),
                    buffer.len(),
                    &raw mut length,
                )
            };
            check(result).map_err(|error| TerminalError::Engine(error.to_string()))?;
            if length > buffer.len() {
                return Err(TerminalError::Engine(format!(
                    "libghostty encoded an oversized mouse event: {length} bytes"
                )));
            }
            Ok(buffer[..length].to_vec())
        }
    }

    impl Drop for GhosttyEngine {
        fn drop(&mut self) {
            // SAFETY: these are the sole owned handles and Drop runs once.
            unsafe {
                mux_ghostty_selection_gesture_free(
                    self.selection_gesture.as_ptr(),
                    self.terminal.as_ptr(),
                );
                mux_ghostty_key_encoder_free(self.key_encoder.as_ptr());
                mux_ghostty_mouse_encoder_free(self.mouse_encoder.as_ptr());
                mux_ghostty_renderer_free(self.renderer.as_ptr());
                mux_ghostty_response_collector_free(self.responses.as_ptr());
                mux_ghostty_terminal_free(self.terminal.as_ptr());
            }
        }
    }

    struct RenderFrameGuard(CRenderFrame);

    impl Drop for RenderFrameGuard {
        fn drop(&mut self) {
            // SAFETY: the frame borrows renderer-owned buffers. Releasing it
            // invalidates only this view; the renderer retains the capacity.
            unsafe { mux_ghostty_render_frame_free(&raw mut self.0) };
        }
    }

    #[derive(Debug, Error)]
    pub enum GhosttyError {
        #[error("libghostty-vt returned error code {0}")]
        Library(i32),
        #[error("libghostty-vt returned a null terminal")]
        NullTerminal,
        #[error("libghostty-vt returned a null snapshot")]
        NullSnapshot,
        #[error("libghostty-vt returned a null renderer")]
        NullRenderer,
        #[error("libghostty-vt returned a null response collector")]
        NullResponseCollector,
        #[error("libghostty-vt returned a null key encoder")]
        NullKeyEncoder,
        #[error("libghostty-vt returned a null mouse encoder")]
        NullMouseEncoder,
        #[error("libghostty-vt returned a null selection gesture")]
        NullSelectionGesture,
        #[error("terminal checkpoint belongs to a different engine build")]
        IncompatibleCheckpoint,
        #[error(transparent)]
        Terminal(#[from] TerminalError),
    }

    fn check(result: i32) -> Result<(), GhosttyError> {
        if result == SUCCESS {
            Ok(())
        } else {
            Err(GhosttyError::Library(result))
        }
    }

    fn create_renderer() -> Result<NonNull<c_void>, GhosttyError> {
        static PNG_DECODER: Once = Once::new();
        // Programs mostly send Kitty images as PNGs, which libghostty leaves
        // to its embedder to decode.
        // SAFETY: the decoder is a plain function that lives as long as the
        // process.
        PNG_DECODER.call_once(|| unsafe {
            let _ = mux_ghostty_set_png_decoder(Some(crate::png_decode::decode_png));
        });
        let mut renderer = ptr::null_mut();
        // SAFETY: `renderer` is a valid out pointer.
        let result = unsafe { mux_ghostty_renderer_new(&raw mut renderer) };
        check(result)?;
        NonNull::new(renderer).ok_or(GhosttyError::NullRenderer)
    }

    fn create_responses(
        terminal: NonNull<c_void>,
        renderer: NonNull<c_void>,
        size: TerminalSize,
    ) -> Result<NonNull<c_void>, GhosttyError> {
        let mut responses = ptr::null_mut();
        // SAFETY: `responses` is a valid out pointer.
        let result =
            unsafe { mux_ghostty_response_collector_new(size.cols, size.rows, &raw mut responses) };
        check(result)?;
        let responses = NonNull::new(responses).ok_or(GhosttyError::NullResponseCollector)?;
        // SAFETY: all three handles are valid and solely owned by the engine
        // being created, which frees the terminal before the other two.
        let result = unsafe {
            mux_ghostty_terminal_enable_responses(
                terminal.as_ptr(),
                responses.as_ptr(),
                renderer.as_ptr(),
            )
        };
        if let Err(error) = check(result) {
            // SAFETY: the collector has not escaped this helper.
            unsafe { mux_ghostty_response_collector_free(responses.as_ptr()) };
            return Err(error);
        }
        Ok(responses)
    }

    fn create_key_encoder() -> Result<NonNull<c_void>, GhosttyError> {
        let mut encoder = ptr::null_mut();
        // SAFETY: `encoder` is a valid out pointer.
        let result = unsafe { mux_ghostty_key_encoder_new(&raw mut encoder) };
        check(result)?;
        NonNull::new(encoder).ok_or(GhosttyError::NullKeyEncoder)
    }

    fn create_mouse_encoder() -> Result<NonNull<c_void>, GhosttyError> {
        let mut encoder = ptr::null_mut();
        // SAFETY: `encoder` is a valid out pointer.
        let result = unsafe { mux_ghostty_mouse_encoder_new(&raw mut encoder) };
        check(result)?;
        NonNull::new(encoder).ok_or(GhosttyError::NullMouseEncoder)
    }

    fn create_selection_gesture() -> Result<NonNull<c_void>, GhosttyError> {
        let mut gesture = ptr::null_mut();
        // SAFETY: `gesture` is a valid out pointer.
        let result = unsafe { mux_ghostty_selection_gesture_new(&raw mut gesture) };
        check(result)?;
        NonNull::new(gesture).ok_or(GhosttyError::NullSelectionGesture)
    }

    const fn key_action(action: TerminalKeyAction) -> u8 {
        match action {
            TerminalKeyAction::Release => 0,
            TerminalKeyAction::Press => 1,
            TerminalKeyAction::Repeat => 2,
        }
    }

    fn key_tag(key: TerminalKey) -> (u8, u8) {
        match key {
            TerminalKey::Backquote => (18, 0),
            TerminalKey::Backslash => (19, 0),
            TerminalKey::BracketLeft => (20, 0),
            TerminalKey::BracketRight => (21, 0),
            TerminalKey::Comma => (22, 0),
            TerminalKey::Digit(number) => (23, number),
            TerminalKey::Equal => (24, 0),
            TerminalKey::Letter(letter) if letter.is_ascii_lowercase() => {
                (25, u8::try_from(letter).unwrap_or(b'a') - b'a')
            }
            TerminalKey::Unidentified | TerminalKey::Letter(_) => (0, 0),
            TerminalKey::Minus => (26, 0),
            TerminalKey::Period => (27, 0),
            TerminalKey::Quote => (28, 0),
            TerminalKey::Semicolon => (29, 0),
            TerminalKey::Slash => (30, 0),
            TerminalKey::IntlBackslash => (31, 0),
            TerminalKey::IntlRo => (32, 0),
            TerminalKey::IntlYen => (33, 0),
            TerminalKey::Backspace => (1, 0),
            TerminalKey::Enter => (2, 0),
            TerminalKey::Tab => (3, 0),
            TerminalKey::Space => (4, 0),
            TerminalKey::Delete => (5, 0),
            TerminalKey::Insert => (6, 0),
            TerminalKey::Home => (7, 0),
            TerminalKey::End => (8, 0),
            TerminalKey::PageUp => (9, 0),
            TerminalKey::PageDown => (10, 0),
            TerminalKey::ArrowUp => (11, 0),
            TerminalKey::ArrowDown => (12, 0),
            TerminalKey::ArrowLeft => (13, 0),
            TerminalKey::ArrowRight => (14, 0),
            TerminalKey::Escape => (15, 0),
            TerminalKey::Function(number) => (16, number),
            TerminalKey::NumpadEnter => (17, 0),
        }
    }

    fn modifier_bits(modifiers: TerminalModifiers) -> u16 {
        u16::from(modifiers.shift)
            | (u16::from(modifiers.control) << 1)
            | (u16::from(modifiers.alt) << 2)
            | (u16::from(modifiers.super_key) << 3)
    }

    const fn mouse_action(action: TerminalMouseAction) -> u8 {
        match action {
            TerminalMouseAction::Press => 0,
            TerminalMouseAction::Release => 1,
            TerminalMouseAction::Motion => 2,
        }
    }

    const fn mouse_button(button: TerminalMouseButton) -> u8 {
        match button {
            TerminalMouseButton::Left => 1,
            TerminalMouseButton::Right => 2,
            TerminalMouseButton::Middle => 3,
            TerminalMouseButton::Four => 4,
            TerminalMouseButton::Five => 5,
            TerminalMouseButton::Six => 6,
            TerminalMouseButton::Seven => 7,
            TerminalMouseButton::Eight => 8,
            TerminalMouseButton::Nine => 9,
            TerminalMouseButton::Ten => 10,
            TerminalMouseButton::Eleven => 11,
        }
    }

    fn convert_frame_into(
        frame: &CRenderFrame,
        rendered: &mut RenderFrame,
    ) -> Result<(), TerminalError> {
        let cell_count = usize::from(frame.cols) * usize::from(frame.rows);
        let rows = borrowed_slice(frame.row_metadata, usize::from(frame.rows), "rows")?;
        let cells = borrowed_slice(frame.cells, cell_count, "cells")?;
        let text = borrowed_slice(frame.text, frame.text_len, "text")?;

        rendered
            .row_metadata
            .resize(usize::from(frame.rows), RenderRow::default());
        for (target, row) in rendered.row_metadata.iter_mut().zip(rows) {
            *target = RenderRow {
                wrapped: row.wrapped != 0,
                continuation: row.continuation != 0,
                dirty: row.dirty != 0,
            };
        }
        rendered.cells.resize_with(cell_count, blank_render_cell);
        let columns = usize::from(frame.cols).max(1);
        for ((targets, cells), row) in rendered
            .cells
            .chunks_mut(columns)
            .zip(cells.chunks(columns))
            .zip(rows)
        {
            if row.rebuilt == 0 {
                continue;
            }
            for (target, cell) in targets.iter_mut().zip(cells) {
                convert_cell_into(cell, text, target)?;
            }
        }
        let cursor = if frame.cursor_has_value == 0 {
            None
        } else {
            Some(RenderCursor {
                visible: frame.cursor_visible != 0,
                blinking: frame.cursor_blinking != 0,
                x: frame.cursor_x,
                y: frame.cursor_y,
                style: match frame.cursor_style {
                    0 => CursorStyle::Bar,
                    1 => CursorStyle::Block,
                    2 => CursorStyle::Underline,
                    3 => CursorStyle::HollowBlock,
                    other => return Err(invalid_render_value("cursor style", other)),
                },
                color: frame.cursor_color.into(),
            })
        };

        rendered.cols = frame.cols;
        rendered.rows = frame.rows;
        rendered.dirty = match frame.dirty {
            0 => RenderDirty::Clean,
            1 => RenderDirty::Partial,
            2 => RenderDirty::Full,
            other => return Err(invalid_render_value("dirty state", other)),
        };
        rendered.background = frame.background.into();
        rendered.foreground = frame.foreground.into();
        rendered.cursor = cursor;
        rendered.scroll = TerminalScrollState {
            total: frame.scroll_total,
            offset: frame.scroll_offset,
            len: frame.scroll_len,
        };
        rendered.held = frame.held != 0;
        rendered.redraw_in =
            (frame.redraw_in_ms != 0).then(|| Duration::from_millis(frame.redraw_in_ms.into()));
        Ok(())
    }

    fn convert_cell_into(
        cell: &CRenderCell,
        text: &[u8],
        rendered: &mut RenderCell,
    ) -> Result<(), TerminalError> {
        let start = usize::try_from(cell.text_offset)
            .map_err(|_| TerminalError::Engine("invalid text offset".to_owned()))?;
        let length = usize::try_from(cell.text_len)
            .map_err(|_| TerminalError::Engine("invalid text length".to_owned()))?;
        let end = start
            .checked_add(length)
            .ok_or_else(|| TerminalError::Engine("render text range overflowed".to_owned()))?;
        let grapheme =
            std::str::from_utf8(text.get(start..end).ok_or_else(|| {
                TerminalError::Engine("render text range was invalid".to_owned())
            })?)
            .map_err(|error| {
                TerminalError::Engine(format!("render text was not UTF-8: {error}"))
            })?;
        let hyperlink = if cell.hyperlink_len == 0 {
            None
        } else {
            let start = usize::try_from(cell.hyperlink_offset)
                .map_err(|_| TerminalError::Engine("invalid hyperlink offset".to_owned()))?;
            let length = usize::try_from(cell.hyperlink_len)
                .map_err(|_| TerminalError::Engine("invalid hyperlink length".to_owned()))?;
            let end = start.checked_add(length).ok_or_else(|| {
                TerminalError::Engine("render hyperlink range overflowed".to_owned())
            })?;
            Some(
                std::str::from_utf8(text.get(start..end).ok_or_else(|| {
                    TerminalError::Engine("render hyperlink range was invalid".to_owned())
                })?)
                .map_err(|error| {
                    TerminalError::Engine(format!("render hyperlink was not UTF-8: {error}"))
                })?,
            )
        };
        let width = match cell.width {
            0 => CellWidth::Narrow,
            1 => CellWidth::Wide,
            2 => CellWidth::SpacerTail,
            3 => CellWidth::SpacerHead,
            other => return Err(invalid_render_value("cell width", other)),
        };
        let semantic = match cell.semantic {
            0 => SemanticContent::Output,
            1 => SemanticContent::Input,
            2 => SemanticContent::Prompt,
            other => return Err(invalid_render_value("semantic content", other)),
        };

        rendered.grapheme.clear();
        rendered.grapheme.push_str(grapheme);
        match (rendered.hyperlink.as_mut(), hyperlink) {
            (Some(target), Some(value)) => {
                target.clear();
                target.push_str(value);
            }
            (_, Some(value)) => rendered.hyperlink = Some(value.to_owned()),
            (_, None) => rendered.hyperlink = None,
        }
        rendered.foreground = cell.foreground.into();
        rendered.background = cell.background.into();
        rendered.underline_color = cell.underline_color.into();
        rendered.style = CellStyle {
            bold: cell.flags & CELL_BOLD != 0,
            italic: cell.flags & CELL_ITALIC != 0,
            faint: cell.flags & CELL_FAINT != 0,
            blink: cell.flags & CELL_BLINK != 0,
            inverse: cell.flags & CELL_INVERSE != 0,
            invisible: cell.flags & CELL_INVISIBLE != 0,
            strikethrough: cell.flags & CELL_STRIKETHROUGH != 0,
            overline: cell.flags & CELL_OVERLINE != 0,
            underline: cell.underline,
        };
        rendered.width = width;
        rendered.semantic = semantic;
        rendered.selected = cell.selected != 0;
        Ok(())
    }

    fn blank_render_cell() -> RenderCell {
        RenderCell {
            grapheme: String::new(),
            foreground: Rgb::default(),
            background: Rgb::default(),
            underline_color: Rgb::default(),
            style: CellStyle::default(),
            width: CellWidth::Narrow,
            semantic: SemanticContent::Output,
            selected: false,
            hyperlink: None,
        }
    }

    fn borrowed_slice<'a, T>(
        pointer: *const T,
        length: usize,
        name: &str,
    ) -> Result<&'a [T], TerminalError> {
        if length == 0 {
            return Ok(&[]);
        }
        let pointer = NonNull::new(pointer.cast_mut()).ok_or_else(|| {
            TerminalError::Engine(format!("libghostty returned null {name} storage"))
        })?;
        // SAFETY: the successful C frame call exposed at least `length`
        // renderer-owned entries, and no next frame call occurs during conversion.
        Ok(unsafe { std::slice::from_raw_parts(pointer.as_ptr(), length) })
    }

    fn invalid_render_value(name: &str, value: u8) -> TerminalError {
        TerminalError::Engine(format!("libghostty returned invalid {name}: {value}"))
    }

    impl From<CRgb> for Rgb {
        fn from(value: CRgb) -> Self {
            Self {
                r: value.r,
                g: value.g,
                b: value.b,
            }
        }
    }

    impl From<Rgb> for CRgb {
        fn from(value: Rgb) -> Self {
            Self {
                r: value.r,
                g: value.g,
                b: value.b,
            }
        }
    }

    fn descriptor() -> EngineDescriptor {
        EngineDescriptor {
            name: "libghostty-vt".to_owned(),
            revision: GHOSTTY_REVISION.to_owned(),
            checkpoint_format: 1,
        }
    }

    pub use GhosttyEngine as Engine;
    pub use GhosttyError as Error;

    #[cfg(test)]
    mod tests {
        use super::*;

        /// A terminal with a cell size, which images need to be placed.
        fn sized_engine(cols: u16, rows: u16) -> GhosttyEngine {
            let size = TerminalSize {
                cols,
                rows,
                cell_width_px: 10,
                cell_height_px: 20,
            };
            let mut engine = GhosttyEngine::new(size).expect("new terminal");
            engine.resize(size).expect("resize");
            engine
        }

        fn first_row_text(frame: &RenderFrame) -> String {
            frame.cells[..usize::from(frame.cols)]
                .iter()
                .map(|cell| cell.grapheme.as_str())
                .collect::<String>()
                .trim_end()
                .to_owned()
        }

        fn base64(bytes: &[u8]) -> String {
            const DIGITS: &[u8] =
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
            let mut encoded = String::new();
            for chunk in bytes.chunks(3) {
                let value = chunk
                    .iter()
                    .enumerate()
                    .fold(0_u32, |value, (index, &byte)| {
                        value | u32::from(byte) << (16 - 8 * index)
                    });
                for index in 0..4 {
                    if index <= chunk.len() {
                        let digit = (value >> (18 - 6 * index)) & 0x3f;
                        encoded.push(char::from(DIGITS[digit as usize]));
                    } else {
                        encoded.push('=');
                    }
                }
            }
            encoded
        }

        /// A Kitty graphics command transmitting and showing image `id`.
        fn kitty_image(id: u32, keys: &str, payload: &[u8]) -> Vec<u8> {
            format!("\x1b_Ga=T,i={id},{keys};{}\x1b\\", base64(payload)).into_bytes()
        }

        #[test]
        fn synchronized_output_keeps_the_finished_frame_until_the_next_is_done() {
            let mut engine = sized_engine(20, 2);
            engine.apply_output(1, b"old").expect("output");
            let frame = engine.render_frame().expect("frame");
            assert_eq!(first_row_text(&frame), "old");
            assert!(!frame.held);

            engine
                .apply_output(2, b"\x1b[?2026h\rnew")
                .expect("held output");
            let held = engine.render_frame().expect("held frame");
            assert!(held.held);
            assert_eq!(first_row_text(&held), "old");
            assert!(
                held.redraw_in
                    .is_some_and(|wait| wait <= Duration::from_secs(1))
            );

            engine.apply_output(3, b"\x1b[?2026l").expect("release");
            let done = engine.render_frame().expect("released frame");
            assert!(!done.held);
            assert_eq!(first_row_text(&done), "new");
        }

        #[test]
        fn a_resize_shows_a_held_screen_as_it_is() {
            let mut engine = sized_engine(20, 2);
            engine.apply_output(1, b"old").expect("output");
            engine.render_frame().expect("frame");
            engine
                .apply_output(2, b"\x1b[?2026h\rnew")
                .expect("held output");
            engine
                .resize(TerminalSize {
                    cols: 30,
                    rows: 3,
                    cell_width_px: 10,
                    cell_height_px: 20,
                })
                .expect("resize");
            let frame = engine.render_frame().expect("frame");
            assert!(!frame.held);
            assert_eq!(frame.cols, 30);
            assert_eq!(first_row_text(&frame), "new");
        }

        #[test]
        fn a_terminal_nobody_draws_does_not_hold() {
            let mut engine = sized_engine(20, 2);
            engine
                .apply_output(1, b"\x1b[?2026hheld")
                .expect("held output");
            // The daemon never draws, so nothing was captured to show.
            let frame = engine.render_frame().expect("frame");
            assert!(!frame.held);
            assert_eq!(first_row_text(&frame), "held");
        }

        #[test]
        fn a_program_says_what_it_is_doing_and_can_ask_if_that_is_heard() {
            let mut engine = sized_engine(20, 2);
            engine.apply_output(1, b"\x1b]7501;?\x1b\\").expect("query");
            let reply = engine.take_pty_responses().expect("responses");
            assert!(String::from_utf8_lossy(&reply).starts_with("\x1b]7501;?"));

            // "Apply?" in base64.
            engine
                .apply_output(
                    2,
                    b"\x1b]7501;state=blocked:kind=permission:app=terraform:msg=QXBwbHk/\x1b\\",
                )
                .expect("report");
            let status = engine.take_program_status().expect("status");
            assert_eq!(status.state, ProgramState::Blocked);
            assert_eq!(status.need, Some(ProgramNeed::Permission));
            assert_eq!(status.app, "terraform");
            assert_eq!(status.message, "Apply?");
            assert_eq!(status.progress, None);
            assert!(engine.take_program_status().is_none());

            // A report about one of its records, not itself, is left out.
            engine
                .apply_output(3, b"\x1b]7501;state=working:id=build:progress=5\x07")
                .expect("record");
            assert!(engine.take_program_status().is_none());
        }

        #[test]
        fn quiet_history_compresses_and_still_reads_back() {
            let mut engine = sized_engine(80, 24);
            let mut output = Vec::new();
            for line in 0..5_000 {
                output.extend_from_slice(format!("line {line} of repeated history\r\n").as_bytes());
            }
            engine.apply_output(1, &output).expect("output");
            // The scrollback limit has already let the oldest lines go, so
            // what has to survive is whatever history there was.
            let history = engine.screen_text().expect("history before");
            assert!(history.contains("line 4999 of repeated history"));
            let before = engine.memory_bytes().expect("memory before");
            let mut steps = 0;
            while engine.compress_history_step().expect("compress") {
                steps += 1;
                assert!(steps < 100_000, "compression never finished");
            }
            let after = engine.memory_bytes().expect("memory after");
            assert!(after <= before, "{after} bytes after, {before} before");
            assert_eq!(engine.screen_text().expect("history after"), history);
        }

        #[test]
        fn a_full_screen_program_is_seen_on_the_alternate_screen() {
            let mut engine = sized_engine(20, 2);
            assert!(!engine.on_alternate_screen());
            engine.apply_output(1, b"\x1b[?1049h").expect("enter");
            assert!(engine.on_alternate_screen());
            engine.apply_output(2, b"\x1b[?1049l").expect("leave");
            assert!(!engine.on_alternate_screen());
        }

        #[test]
        fn a_program_picks_the_pointer_shape() {
            let mut engine = sized_engine(20, 2);
            assert_eq!(engine.mouse_shape(), MouseShape::Text);
            engine
                .apply_output(1, b"\x1b]22;pointer\x07")
                .expect("shape");
            assert_eq!(engine.mouse_shape(), MouseShape::Pointer);
            engine.apply_output(2, b"\x1b]22;\x07").expect("reset");
            assert_eq!(engine.mouse_shape(), MouseShape::Text);
        }

        #[test]
        fn kitty_images_are_placed_with_their_pixels() {
            let mut engine = sized_engine(20, 4);
            engine
                .apply_output(1, &kitty_image(7, "f=24,s=2,v=1", &[255, 0, 0, 0, 255, 0]))
                .expect("image");
            let frame = engine.render_frame().expect("frame");
            assert_eq!(frame.images.len(), 1);
            let image = &frame.images[0];
            assert_eq!(image.pixels.id, 7);
            assert_eq!((image.pixels.width, image.pixels.height), (2, 1));
            assert_eq!(image.pixels.rgba, [255, 0, 0, 255, 0, 255, 0, 255]);
            assert_eq!((image.column, image.row), (0, 0));
            assert_eq!((image.source_width, image.source_height), (2, 1));
            assert!(image.width > 0 && image.height > 0);

            // Text alone leaves the images, and their pixels, as they were.
            engine.apply_output(2, b"text").expect("text");
            let next = engine.render_frame().expect("next frame");
            assert!(Arc::ptr_eq(&next.images[0].pixels, &image.pixels));

            engine
                .apply_output(3, b"\x1b_Ga=d,d=A\x1b\\")
                .expect("delete");
            assert!(engine.render_frame().expect("frame").images.is_empty());
        }

        #[test]
        fn kitty_images_sent_as_pngs_are_decoded() {
            let mut png_bytes = Vec::new();
            {
                let mut encoder = png::Encoder::new(&mut png_bytes, 1, 1);
                encoder.set_color(png::ColorType::Rgba);
                encoder.set_depth(png::BitDepth::Eight);
                let mut writer = encoder.write_header().expect("header");
                writer.write_image_data(&[1, 2, 3, 4]).expect("pixels");
            }
            let mut engine = sized_engine(20, 4);
            engine
                .apply_output(1, &kitty_image(3, "f=100", &png_bytes))
                .expect("image");
            let frame = engine.render_frame().expect("frame");
            assert_eq!(frame.images.len(), 1);
            assert_eq!(frame.images[0].pixels.rgba, [1, 2, 3, 4]);
        }

        #[test]
        fn kitty_images_come_back_with_a_checkpoint() {
            let mut engine = sized_engine(20, 4);
            engine
                .apply_output(1, &kitty_image(5, "f=32,s=1,v=1", &[9, 8, 7, 6]))
                .expect("image");
            let checkpoint = engine.checkpoint().expect("checkpoint");
            let mut restored = GhosttyEngine::restore(&checkpoint).expect("restore");
            restored
                .resize(TerminalSize {
                    cols: 20,
                    rows: 4,
                    cell_width_px: 10,
                    cell_height_px: 20,
                })
                .expect("resize");
            let frame = restored.render_frame().expect("frame");
            assert_eq!(frame.images.len(), 1);
            assert_eq!(frame.images[0].pixels.rgba, [9, 8, 7, 6]);
        }

        #[test]
        fn kitty_images_are_not_read_from_files() {
            let mut engine = sized_engine(20, 4);
            let path = std::env::temp_dir().join("mux-kitty-image-medium-test.rgba");
            fs_write(&path, &[1, 2, 3, 4]);
            let command = format!(
                "\x1b_Ga=T,i=9,f=32,s=1,v=1,t=f;{}\x1b\\",
                base64(path.to_string_lossy().as_bytes())
            );
            engine.apply_output(1, command.as_bytes()).expect("image");
            let responses = engine.take_pty_responses().expect("responses");
            let _ = std::fs::remove_file(&path);
            assert!(engine.render_frame().expect("frame").images.is_empty());
            // The program hears it failed, and can send the pixels inline.
            assert!(!String::from_utf8_lossy(&responses).contains(";OK"));
        }

        fn fs_write(path: &std::path::Path, bytes: &[u8]) {
            std::fs::write(path, bytes).expect("write image file");
        }

        #[test]
        fn checkpoint_round_trip_preserves_sequence_and_accepts_more_output() {
            let mut engine = GhosttyEngine::new(TerminalSize::default()).expect("new terminal");
            engine
                .apply_output(1, b"first\r\n\x1b[32mgreen\x1b[0m")
                .expect("initial output");
            let attachment = engine.attachment().expect("attachment");
            let checkpoint = attachment.checkpoint.expect("checkpoint");
            assert!(!checkpoint.payload.is_empty());

            let mut restored = GhosttyEngine::restore(&checkpoint).expect("restore terminal");
            restored
                .apply_output(2, b"\r\nsecond")
                .expect("continued output");
            let restored_attachment = restored.attachment().expect("restored attachment");
            assert_eq!(restored_attachment.next_sequence, 3);
        }

        #[test]
        fn checkpoint_handles_an_unfinished_escape_sequence() {
            let mut engine = GhosttyEngine::new(TerminalSize::default()).expect("new terminal");
            engine.apply_output(1, b"\x1b[38;2").expect("partial CSI");
            let checkpoint = engine
                .attachment()
                .expect("partial sequence attachment")
                .checkpoint
                .expect("checkpoint");
            let mut restored = GhosttyEngine::restore(&checkpoint).expect("restore terminal");
            restored
                .apply_output(2, b";255;0;0mred")
                .expect("complete CSI after restore");
        }

        #[test]
        fn render_frame_preserves_unicode_color_and_style() {
            let mut engine = GhosttyEngine::new(TerminalSize {
                cols: 20,
                rows: 4,
                ..TerminalSize::default()
            })
            .expect("new terminal");
            engine
                .apply_output(
                    1,
                    "plain 🦀\r\n\x1b[1;38;2;12;34;56mstyled\x1b[0m".as_bytes(),
                )
                .expect("styled output");

            let frame = engine.render_frame().expect("render frame");
            assert_eq!(frame.cols, 20);
            assert_eq!(frame.rows, 4);
            assert_eq!(frame.cells.len(), 80);
            assert!(frame.cells.iter().any(|cell| cell.grapheme == "🦀"));
            let styled = frame
                .cells
                .iter()
                .find(|cell| cell.grapheme == "s")
                .expect("styled cell");
            assert!(styled.style.bold);
            assert_eq!(
                styled.foreground,
                Rgb {
                    r: 12,
                    g: 34,
                    b: 56
                }
            );
        }

        #[test]
        fn render_frame_into_reuses_viewport_and_grapheme_storage() {
            let mut engine = GhosttyEngine::new(TerminalSize {
                cols: 80,
                rows: 24,
                ..TerminalSize::default()
            })
            .expect("new terminal");
            engine.apply_output(1, b"x").expect("initial output");
            let mut frame = engine.render_frame().expect("initial frame");
            let rows = frame.row_metadata.as_ptr();
            let cells = frame.cells.as_ptr();
            let first_grapheme = frame.cells[0].grapheme.as_ptr();

            engine.apply_output(2, b"y").expect("terminal output");
            engine.render_frame_into(&mut frame).expect("reused frame");

            assert_eq!(frame.row_metadata.as_ptr(), rows);
            assert_eq!(frame.cells.as_ptr(), cells);
            assert_eq!(frame.cells[0].grapheme.as_ptr(), first_grapheme);
            assert_eq!(frame.cells[0].grapheme, "x");
        }

        fn row_text(frame: &RenderFrame, row: usize) -> String {
            let columns = usize::from(frame.cols);
            frame.cells[row * columns..(row + 1) * columns]
                .iter()
                .map(|cell| cell.grapheme.as_str())
                .collect::<String>()
        }

        #[test]
        fn refilling_a_frame_rewrites_only_the_rows_that_changed() {
            let mut engine = GhosttyEngine::new(TerminalSize {
                cols: 20,
                rows: 6,
                ..TerminalSize::default()
            })
            .expect("new terminal");
            engine
                .apply_output(1, b"one\r\ntwo\r\nthree")
                .expect("initial output");
            let mut frame = engine.render_frame().expect("initial frame");
            // A row nothing writes to is left exactly as the frame had it.
            frame.cells[0].grapheme = "Z".to_owned();

            engine
                .apply_output(2, b"\x1b[2;1Hfour")
                .expect("rewrite the second row");
            engine
                .render_frame_into(&mut frame)
                .expect("refilled frame");

            assert_eq!(row_text(&frame, 0), "Zne");
            assert_eq!(row_text(&frame, 1), "four");
            assert_eq!(row_text(&frame, 2), "three");
            assert!(frame.row_metadata[1].dirty);
            assert!(!frame.row_metadata[0].dirty);
        }

        #[test]
        fn a_refilled_frame_matches_a_fresh_one() {
            use std::fmt::Write as _;

            let mut engine = GhosttyEngine::new(TerminalSize {
                cols: 20,
                rows: 6,
                ..TerminalSize::default()
            })
            .expect("new terminal");
            engine
                .apply_output(1, b"one\r\ntwo")
                .expect("initial output");
            let mut frame = engine.render_frame().expect("initial frame");

            // Typing, then a theme, as a restored pane is given one.
            engine
                .apply_output(2, b"\x1b[31mred")
                .expect("coloured text");
            engine.render_frame_into(&mut frame).expect("after typing");
            assert_eq!(frame.cells, engine.render_frame().expect("fresh").cells);

            engine
                .apply_theme(&GhosttyTheme {
                    background: Some(Rgb {
                        r: 0x12,
                        g: 0x34,
                        b: 0x56,
                    }),
                    foreground: Some(Rgb {
                        r: 0xee,
                        g: 0xee,
                        b: 0xee,
                    }),
                    ..GhosttyTheme::default()
                })
                .expect("new background");
            engine
                .render_frame_into(&mut frame)
                .expect("after background");
            let fresh = engine.render_frame().expect("fresh");
            assert_eq!(
                frame.cells[0].background,
                Rgb {
                    r: 0x12,
                    g: 0x34,
                    b: 0x56
                }
            );
            assert_eq!(frame.cells, fresh.cells);

            // A selection, and looking back into history, change rows that
            // no output touched.
            engine
                .set_selection(Some(TerminalSelection {
                    anchor: mux_terminal::TerminalPoint { column: 0, row: 0 },
                    focus: mux_terminal::TerminalPoint { column: 2, row: 1 },
                    rectangular: false,
                }))
                .expect("select");
            engine.render_frame_into(&mut frame).expect("after select");
            assert!(frame.cells[0].selected);
            assert_eq!(frame.cells, engine.render_frame().expect("fresh").cells);
            engine.set_selection(None).expect("clear selection");
            engine.render_frame_into(&mut frame).expect("after clear");
            assert_eq!(frame.cells, engine.render_frame().expect("fresh").cells);

            let mut lines = String::new();
            for number in 1..=20 {
                write!(lines, "\r\n{number}").expect("write test output");
            }
            engine
                .apply_output(3, lines.as_bytes())
                .expect("scroll into history");
            engine.render_frame_into(&mut frame).expect("at the bottom");
            engine
                .scroll_viewport(TerminalViewportScroll::Delta(-3))
                .expect("look back");
            engine.render_frame_into(&mut frame).expect("looking back");
            assert_eq!(frame.cells, engine.render_frame().expect("fresh").cells);
        }

        #[test]
        fn a_frame_the_last_fill_did_not_write_is_rebuilt_in_full() {
            let mut engine = GhosttyEngine::new(TerminalSize {
                cols: 20,
                rows: 6,
                ..TerminalSize::default()
            })
            .expect("new terminal");
            engine
                .apply_output(1, b"one\r\ntwo")
                .expect("initial output");
            let mut frame = engine.render_frame().expect("initial frame");
            let mut copy = frame.clone();

            engine.apply_output(2, b"\r\nthree").expect("more output");
            engine.render_frame_into(&mut copy).expect("copy refilled");
            engine
                .apply_output(3, b"\r\nfour")
                .expect("still more output");
            engine
                .render_frame_into(&mut frame)
                .expect("original refilled");

            assert_eq!(row_text(&copy, 2), "three");
            assert_eq!(row_text(&frame, 3), "four");
            assert_eq!(frame.cells, engine.render_frame().expect("fresh").cells);
        }

        #[test]
        fn configured_palette_and_plain_bold_match_ghostty_policy() {
            let mut theme = GhosttyTheme {
                background: Some(Rgb {
                    r: 0x1e,
                    g: 0x1e,
                    b: 0x2e,
                }),
                foreground: Some(Rgb {
                    r: 0xcd,
                    g: 0xd6,
                    b: 0xf4,
                }),
                cursor: None,
                ..GhosttyTheme::default()
            };
            let red = Rgb {
                r: 0xf3,
                g: 0x8b,
                b: 0xa8,
            };
            let bright_red = Rgb {
                r: 0xff,
                g: 0x00,
                b: 0x00,
            };
            theme.palette[1] = Some(red);
            theme.palette[9] = Some(bright_red);
            let mut engine = GhosttyEngine::new_with_theme(
                TerminalSize {
                    cols: 8,
                    rows: 2,
                    ..TerminalSize::default()
                },
                &theme,
            )
            .expect("themed terminal");
            engine
                .apply_output(1, b"\x1b[31mR\x1b[1mB")
                .expect("palette output");

            let frame = engine.render_frame().expect("render frame");
            assert_eq!(frame.background, theme.background.expect("background"));
            assert_eq!(frame.foreground, theme.foreground.expect("foreground"));
            assert_eq!(frame.cells[0].foreground, red);
            assert_eq!(frame.cells[1].foreground, red);
            assert!(frame.cells[1].style.bold);
        }

        #[test]
        fn terminal_queries_generate_pty_responses() {
            let mut engine = GhosttyEngine::new(TerminalSize {
                cols: 90,
                rows: 30,
                ..TerminalSize::default()
            })
            .expect("new terminal");
            engine
                .apply_output(1, b"\x1b[6n\x1b[c\x1b[>c\x1b[>q")
                .expect("terminal queries");
            let responses = engine.take_pty_responses().expect("PTY responses");
            assert!(
                responses
                    .windows(b"\x1b[1;1R".len())
                    .any(|window| window == b"\x1b[1;1R")
            );
            assert!(responses.windows(3).any(|window| window == b"\x1b[?"));
            assert!(responses.windows(3).any(|window| window == b"\x1b[>"));
            assert!(
                responses
                    .windows(env!("CARGO_PKG_VERSION").len())
                    .any(|window| window == env!("CARGO_PKG_VERSION").as_bytes())
            );
            assert!(engine.take_pty_responses().expect("drained").is_empty());
        }

        #[test]
        fn prompts_the_shell_marks_are_found_by_row_history_included() {
            let mut engine = GhosttyEngine::new(TerminalSize {
                cols: 20,
                rows: 4,
                ..TerminalSize::default()
            })
            .expect("new terminal");
            engine
                .apply_output(
                    1,
                    b"\x1b]133;A\x07$ ls\r\n\x1b]133;C\x07a\r\nb\r\nc\r\n\
                      \x1b]133;D;0\x07\x1b]133;A\x07$ pwd\r\n/\r\n\
                      \x1b]133;A\x07$ ",
                )
                .expect("terminal output");
            assert_eq!(engine.prompt_rows().expect("prompt rows"), vec![0, 4, 6]);
            assert_eq!(engine.row_text(4).expect("row text"), "$ pwd");
            assert_eq!(engine.row_text(1).expect("row text"), "a");
        }

        #[test]
        fn screen_text_has_a_line_for_every_row_history_included() {
            let mut engine = GhosttyEngine::new(TerminalSize {
                cols: 12,
                rows: 4,
                ..TerminalSize::default()
            })
            .expect("new terminal");
            // A blank first row, a blank row between, a row that wraps, and
            // enough rows to push the top of it into the history.
            engine
                .apply_output(
                    1,
                    b"\r\none\r\n\r\nthree  x\r\nabcdefghijklmnop\r\nlast\r\n\r\n",
                )
                .expect("terminal output");
            let text = engine.screen_text().expect("screen text");
            let lines: Vec<&str> = text.lines().collect();
            assert_eq!(
                &lines[..7],
                &["", "one", "", "three  x", "abcdefghijkl", "mnop", "last"]
            );
            let frame = engine.render_frame().expect("frame");
            assert!(lines.len() <= usize::try_from(frame.scroll.total).expect("rows"));
        }

        #[test]
        fn a_raised_history_limit_keeps_rows_the_default_lets_go() {
            let size = TerminalSize {
                cols: 80,
                rows: 24,
                ..TerminalSize::default()
            };
            let output = (0..12_000)
                .flat_map(|line| format!("line {line}\r\n").into_bytes())
                .collect::<Vec<_>>();

            let mut default = GhosttyEngine::new(size).expect("default terminal");
            default.apply_output(1, &output).expect("output");
            let kept_by_default = default.render_frame().expect("frame").scroll.total;

            // The window's copy of a pane is restored from a checkpoint, so
            // the limit has to hold on a restored terminal.
            let checkpoint = GhosttyEngine::new(size)
                .expect("terminal")
                .checkpoint()
                .expect("checkpoint");
            let mut raised = GhosttyEngine::restore(&checkpoint).expect("restored terminal");
            raised.set_history_limit(10_000_000).expect("history limit");
            raised.apply_output(1, &output).expect("output");
            let text = raised.screen_text().expect("screen text");

            assert!(
                kept_by_default < 2_000,
                "the default kept {kept_by_default} rows"
            );
            assert_eq!(text.lines().next(), Some("line 0"));
            assert!(text.lines().any(|line| line == "line 11999"));
        }

        #[test]
        fn ghostty_owns_selection_rendering_and_copy_text() {
            let mut engine = GhosttyEngine::new(TerminalSize {
                cols: 20,
                rows: 4,
                ..TerminalSize::default()
            })
            .expect("new terminal");
            engine
                .apply_output(1, b"hello world")
                .expect("terminal output");
            engine
                .set_selection(Some(TerminalSelection {
                    anchor: mux_terminal::TerminalPoint { column: 0, row: 0 },
                    focus: mux_terminal::TerminalPoint { column: 4, row: 0 },
                    rectangular: false,
                }))
                .expect("set selection");

            let frame = engine.render_frame().expect("selected frame");
            assert!(frame.cells[..5].iter().all(|cell| cell.selected));
            assert!(!frame.cells[5].selected);
            assert_eq!(
                engine.selected_text().expect("copy text").as_deref(),
                Some("hello")
            );

            engine.set_selection(None).expect("clear selection");
            assert_eq!(engine.selected_text().expect("no copy text"), None);
        }

        #[test]
        fn ghostty_gesture_owns_click_count_word_line_drag_and_autoscroll() {
            let mut engine = GhosttyEngine::new(TerminalSize {
                cols: 20,
                rows: 4,
                cell_width_px: 10,
                cell_height_px: 20,
            })
            .expect("new terminal");
            engine
                .apply_output(1, b"hello world")
                .expect("terminal output");
            let geometry = mux_terminal::TerminalSelectionGeometry {
                columns: 20,
                cell_width: 10,
                padding_left: 2,
                screen_height: 80,
            };
            let position = mux_terminal::TerminalSurfacePosition { x: 16.0, y: 10.0 };
            let point = mux_terminal::TerminalPoint { column: 1, row: 0 };
            let press = |time_ns| TerminalSelectionGestureEvent::Press {
                point,
                position,
                time_ns,
                repeat_distance: 10.0,
                repeat_interval_ns: 500_000_000,
            };

            let first = engine.selection_gesture(press(1)).expect("single press");
            assert_eq!(first.click_count, 1);
            assert!(!first.has_selection);
            engine
                .selection_gesture(TerminalSelectionGestureEvent::Release { point: Some(point) })
                .expect("single release");

            let second = engine
                .selection_gesture(press(100_000_000))
                .expect("double press");
            assert_eq!(second.click_count, 2);
            assert!(second.has_selection);
            assert_eq!(
                engine.selected_text().expect("word selection").as_deref(),
                Some("hello")
            );
            engine
                .selection_gesture(TerminalSelectionGestureEvent::Release { point: Some(point) })
                .expect("double release");

            let third = engine
                .selection_gesture(press(200_000_000))
                .expect("triple press");
            assert_eq!(third.click_count, 3);
            assert_eq!(
                engine.selected_text().expect("line selection").as_deref(),
                Some("hello world")
            );

            engine.reset_selection_gesture().expect("reset sequence");
            engine
                .selection_gesture(TerminalSelectionGestureEvent::Press {
                    point: mux_terminal::TerminalPoint { column: 0, row: 0 },
                    position: mux_terminal::TerminalSurfacePosition { x: 4.0, y: 10.0 },
                    time_ns: 1_000_000_000,
                    repeat_distance: 10.0,
                    repeat_interval_ns: 500_000_000,
                })
                .expect("drag press");
            let drag = engine
                .selection_gesture(TerminalSelectionGestureEvent::Drag {
                    point: mux_terminal::TerminalPoint { column: 4, row: 0 },
                    position: mux_terminal::TerminalSurfacePosition { x: 49.0, y: 80.0 },
                    rectangular: false,
                    geometry,
                })
                .expect("cell drag");
            assert!(drag.dragged);
            assert!(drag.has_selection);
            assert_eq!(drag.autoscroll, TerminalSelectionAutoscroll::Down);
            assert_eq!(
                engine.selected_text().expect("drag selection").as_deref(),
                Some("hello")
            );
        }

        #[test]
        fn render_cells_expose_their_osc_8_target() {
            let mut engine = GhosttyEngine::new(TerminalSize {
                cols: 20,
                rows: 2,
                ..TerminalSize::default()
            })
            .expect("new terminal");
            engine
                .apply_output(
                    1,
                    b"plain \x1b]8;;https://example.com/docs\x1b\\link\x1b]8;;\x1b\\",
                )
                .expect("hyperlink output");

            let frame = engine.render_frame().expect("render frame");
            assert!(frame.cells[..6].iter().all(|cell| cell.hyperlink.is_none()));
            assert!(
                frame.cells[6..10]
                    .iter()
                    .all(|cell| { cell.hyperlink.as_deref() == Some("https://example.com/docs") })
            );
            assert!(frame.cells[10].hyperlink.is_none());
        }

        #[test]
        fn paste_encoding_tracks_bracketed_paste_mode() {
            let mut engine = GhosttyEngine::new(TerminalSize::default()).expect("new terminal");
            assert_eq!(engine.encode_paste("a\nb").expect("plain paste"), b"a\rb");
            engine
                .apply_output(1, b"\x1b[?2004h")
                .expect("enable bracketed paste");
            assert_eq!(
                engine.encode_paste("a\nb").expect("bracketed paste"),
                b"\x1b[200~a\nb\x1b[201~"
            );
        }

        #[test]
        fn key_encoding_handles_control_characters_and_function_keys() {
            let engine = GhosttyEngine::new(TerminalSize::default()).expect("new terminal");
            assert_eq!(
                engine
                    .encode_key(&key_event(
                        TerminalKey::Tab,
                        None,
                        Some('\t'),
                        TerminalModifiers::default(),
                    ))
                    .expect("encode Tab"),
                b"\t"
            );
            assert_eq!(
                engine
                    .encode_key(&key_event(
                        TerminalKey::Unidentified,
                        Some("c"),
                        Some('c'),
                        TerminalModifiers {
                            control: true,
                            ..TerminalModifiers::default()
                        },
                    ))
                    .expect("encode control-c"),
                vec![0x03]
            );
            assert_eq!(
                engine
                    .encode_key(&key_event(
                        TerminalKey::Function(5),
                        None,
                        None,
                        TerminalModifiers::default(),
                    ))
                    .expect("encode F5"),
                b"\x1b[15~"
            );
        }

        #[test]
        fn key_encoding_tracks_application_cursor_mode() {
            let mut engine = GhosttyEngine::new(TerminalSize::default()).expect("new terminal");
            let up = key_event(
                TerminalKey::ArrowUp,
                None,
                None,
                TerminalModifiers::default(),
            );
            assert_eq!(
                engine.encode_key(&up).expect("encode normal cursor up"),
                b"\x1b[A"
            );

            engine
                .apply_output(1, b"\x1b[?1h")
                .expect("enable application cursor mode");
            assert_eq!(
                engine
                    .encode_key(&up)
                    .expect("encode application cursor up"),
                b"\x1bOA"
            );

            let release = TerminalKeyEvent {
                action: TerminalKeyAction::Release,
                ..up
            };
            assert!(
                engine
                    .encode_key(&release)
                    .expect("encode legacy key release")
                    .is_empty()
            );
        }

        #[test]
        fn kitty_keyboard_reset_clears_stale_release_reporting_without_advancing_output() {
            let mut engine = GhosttyEngine::new(TerminalSize::default()).expect("new terminal");
            engine
                .apply_output(1, b"\x1b[>3u")
                .expect("enable Kitty release reporting");
            assert_eq!(engine.kitty_keyboard_flags().expect("keyboard flags"), 3);

            let release = TerminalKeyEvent {
                action: TerminalKeyAction::Release,
                key: TerminalKey::Letter('a'),
                modifiers: TerminalModifiers::default(),
                consumed_modifiers: TerminalModifiers::default(),
                unshifted_codepoint: Some('a'),
                text: None,
                composing: false,
            };
            assert_eq!(
                engine.encode_key(&release).expect("Kitty key release"),
                b"\x1b[97;1:3u"
            );
            let next_sequence = engine.next_output_sequence();

            engine
                .reset_kitty_keyboard()
                .expect("reset Kitty keyboard mode");

            assert_eq!(engine.kitty_keyboard_flags().expect("reset flags"), 0);
            assert_eq!(engine.next_output_sequence(), next_sequence);
            assert!(
                engine
                    .encode_key(&release)
                    .expect("legacy key release")
                    .is_empty()
            );
        }

        #[test]
        fn shifted_punctuation_survives_legacy_and_kitty_keyboard_modes() {
            let mut engine = GhosttyEngine::new(TerminalSize::default()).expect("new terminal");
            let mut colon = key_event(
                TerminalKey::Semicolon,
                Some(":"),
                Some(';'),
                TerminalModifiers {
                    shift: true,
                    ..TerminalModifiers::default()
                },
            );
            colon.consumed_modifiers.shift = true;
            assert_eq!(engine.encode_key(&colon).expect("legacy colon"), b":");

            engine
                .apply_output(1, b"\x1b[>1u")
                .expect("enable kitty disambiguation");
            assert_eq!(engine.encode_key(&colon).expect("kitty colon"), b":");
        }

        #[test]
        fn viewport_scrolling_uses_ghostty_scrollback_state() {
            use std::fmt::Write as _;

            let mut engine = GhosttyEngine::new(TerminalSize {
                cols: 20,
                rows: 3,
                ..TerminalSize::default()
            })
            .expect("new terminal");
            let mut output = String::new();
            for number in 1..=12 {
                write!(output, "line-{number:02}\r\n").expect("build terminal output");
            }
            engine
                .apply_output(1, output.as_bytes())
                .expect("scrolling output");

            let bottom = engine.render_frame().expect("bottom frame");
            assert!(bottom.scroll.total > bottom.scroll.len);
            assert!(!bottom.scroll.is_scrolled());

            engine
                .scroll_viewport(TerminalViewportScroll::Delta(-4))
                .expect("scroll into history");
            let history = engine.render_frame().expect("history frame");
            assert!(history.scroll.is_scrolled());
            assert!(history.scroll.offset < bottom.scroll.offset);
            assert_ne!(frame_text(&history), frame_text(&bottom));

            engine
                .scroll_viewport(TerminalViewportScroll::Bottom)
                .expect("return to bottom");
            let restored = engine.render_frame().expect("restored frame");
            assert!(!restored.scroll.is_scrolled());
            assert_eq!(frame_text(&restored), frame_text(&bottom));
        }

        #[test]
        fn mouse_encoding_tracks_terminal_reporting_modes() {
            let mut engine = GhosttyEngine::new(TerminalSize::default()).expect("new terminal");
            let mut event = TerminalMouseEvent {
                action: TerminalMouseAction::Press,
                button: Some(TerminalMouseButton::Left),
                modifiers: TerminalModifiers::default(),
                x: 8.0,
                y: 7.0,
                geometry: mux_terminal::TerminalMouseGeometry {
                    screen_width: 800,
                    screen_height: 600,
                    cell_width: 8,
                    cell_height: 20,
                    padding_top: 7,
                    padding_bottom: 7,
                    padding_right: 8,
                    padding_left: 8,
                },
                any_button_pressed: true,
            };
            assert!(
                engine
                    .encode_mouse(&event)
                    .expect("disabled mouse mode")
                    .is_empty()
            );

            engine
                .apply_output(1, b"\x1b[?1000h\x1b[?1006h")
                .expect("enable SGR mouse reporting");
            assert_eq!(
                engine.encode_mouse(&event).expect("mouse press"),
                b"\x1b[<0;1;1M"
            );
            event.action = TerminalMouseAction::Release;
            event.any_button_pressed = false;
            assert_eq!(
                engine.encode_mouse(&event).expect("mouse release"),
                b"\x1b[<0;1;1m"
            );
            event.action = TerminalMouseAction::Press;
            event.button = Some(TerminalMouseButton::Four);
            assert_eq!(
                engine.encode_mouse(&event).expect("mouse wheel up"),
                b"\x1b[<64;1;1M"
            );

            event.action = TerminalMouseAction::Motion;
            event.button = None;
            assert!(
                engine
                    .encode_mouse(&event)
                    .expect("motion outside any-event mode")
                    .is_empty()
            );
            engine
                .apply_output(2, b"\x1b[?1003h")
                .expect("enable any-event mouse tracking");
            assert_eq!(
                engine.encode_mouse(&event).expect("unpressed mouse motion"),
                b"\x1b[<35;1;1M"
            );
        }

        #[test]
        fn sustained_output_parses_at_terminal_speed() {
            use std::fmt::Write as _;

            let mut engine = GhosttyEngine::new(TerminalSize {
                cols: 100,
                rows: 40,
                cell_width_px: 8,
                cell_height_px: 20,
            })
            .expect("new terminal");
            let mut output = String::new();
            for number in 1..=20_000 {
                write!(output, "{number}\r\n").expect("write test output");
            }
            let started = std::time::Instant::now();
            for (index, chunk) in output.as_bytes().chunks(1_024).enumerate() {
                engine
                    .apply_output(index as u64 + 1, chunk)
                    .expect("terminal output");
            }
            let elapsed = started.elapsed();
            assert!(
                elapsed < std::time::Duration::from_millis(500),
                "parsed {} bytes in {elapsed:?}",
                output.len()
            );
        }

        fn key_event(
            key: TerminalKey,
            text: Option<&str>,
            unshifted_codepoint: Option<char>,
            modifiers: TerminalModifiers,
        ) -> TerminalKeyEvent {
            TerminalKeyEvent {
                action: TerminalKeyAction::Press,
                key,
                modifiers,
                consumed_modifiers: TerminalModifiers::default(),
                text: text.map(str::to_owned),
                unshifted_codepoint,
                composing: false,
            }
        }

        fn frame_text(frame: &RenderFrame) -> String {
            frame
                .cells
                .iter()
                .map(|cell| cell.grapheme.as_str())
                .collect()
        }
    }
}

#[cfg(feature = "link")]
pub use linked::{Engine as GhosttyEngine, Error as GhosttyError};

#[cfg(test)]
mod theme_tests {
    use super::*;

    #[test]
    fn ghostty_theme_and_explicit_overrides_are_resolved() {
        let directory = tempfile::tempdir().expect("tempdir");
        let themes = directory.path().join("themes");
        fs::create_dir(&themes).expect("themes directory");
        fs::write(
            themes.join("calm.conf"),
            "background = 101020\nforeground = #c0c0d0\npalette = 4=#1122ee\n",
        )
        .expect("theme");
        let config = directory.path().join("config");
        fs::write(
            &config,
            "foreground = ffffff\ntheme = calm.conf\npalette = 4=#abcdef\n",
        )
        .expect("config");

        let theme = GhosttyTheme::load_from_path(&config, &[directory.path().to_path_buf()])
            .expect("load theme");
        assert_eq!(theme.background, parse_rgb("101020"));
        assert_eq!(theme.foreground, parse_rgb("ffffff"));
        assert_eq!(theme.palette[4], parse_rgb("abcdef"));
    }

    #[test]
    fn history_limit_follows_the_last_scrollback_limit() {
        let mut limit = DEFAULT_HISTORY_BYTES;
        parse_history_limit("font-size = 13\n", &mut limit);
        assert_eq!(limit, DEFAULT_HISTORY_BYTES);
        parse_history_limit(
            "scrollback-limit = 50_000_000\n# scrollback-limit = 1\n",
            &mut limit,
        );
        assert_eq!(limit, 50_000_000);
        parse_history_limit("scrollback-limit = lots\n", &mut limit);
        assert_eq!(limit, 50_000_000);
        parse_history_limit("scrollback-limit = 0\n", &mut limit);
        assert_eq!(limit, 0);
        parse_history_limit("scrollback-limit =\n", &mut limit);
        assert_eq!(limit, DEFAULT_HISTORY_BYTES);
    }

    #[test]
    fn dark_theme_variant_is_selected() {
        assert_eq!(
            dark_theme_name("light:day.conf,dark:night.conf").as_deref(),
            Some("night.conf"),
        );
    }

    #[test]
    fn primary_font_and_size_follow_ghostty_reset_semantics() {
        let directory = tempfile::tempdir().expect("tempdir");
        let config = directory.path().join("config");
        fs::write(
            &config,
            "font-family = Old Primary\nfont-family = Old Fallback\nfont-family = \"\"\nfont-family = \"Jetbrains Mono\"\nfont-family = Symbols\nfont-size = \"16\"\n",
        )
        .expect("config");

        let font = GhosttyFont::load_from_path(&config).expect("load font");
        assert_eq!(font.family.as_deref(), Some("Jetbrains Mono"));
        assert_eq!(font.size, Some(16.0));
    }
}
