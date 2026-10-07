use std::{
    cell::RefCell,
    collections::HashMap,
    rc::{Rc, Weak},
};

use gpui::{
    App, Bounds, Context, Font, Hsla, IntoElement, ParentElement as _, Pixels, Render, ShapedLine,
    SharedString, StrikethroughStyle, Styled, TextRun, TextSystem, UnderlineStyle, Window, canvas,
    div, fill, font, point, px, size,
};
use mux_terminal::{CellStyle, CellWidth, CursorStyle, RenderCell, RenderFrame, Rgb};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridMetrics {
    pub cell_width: f32,
    pub cell_height: f32,
    pub font_size: f32,
    pub padding_x: f32,
    pub padding_y: f32,
}

/// Colours the window chrome lends the terminal: the slab it sits on, and the
/// tab's ink for the cursor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalChrome {
    pub surface: Hsla,
    pub cursor: Hsla,
}

/// Places marked on the grid, such as what a find turned up. The current one
/// is painted solid with its text in `current_text`; the rest take a wash
/// under their own colours. With `dim`, text off the marks fades toward it
/// so the marks stand out.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TerminalMarks {
    pub spans: Vec<MarkSpan>,
    pub wash: Hsla,
    pub current: Hsla,
    pub current_text: Rgb,
    pub dim: Option<Rgb>,
}

/// A row of the viewport and the columns a mark covers, end exclusive.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MarkSpan {
    pub row: usize,
    pub start: usize,
    pub end: usize,
    pub current: bool,
}

impl TerminalMarks {
    /// The colour a cell's text takes: `current_text` on the current mark,
    /// its own on the others, and faded toward `dim` off them all.
    fn text_at(&self, row: usize, column: usize, own: Rgb) -> Rgb {
        let mut marked = false;
        for span in &self.spans {
            if span.row == row && (span.start..span.end).contains(&column) {
                if span.current {
                    return self.current_text;
                }
                marked = true;
            }
        }
        match self.dim {
            Some(toward) if !marked => faded(own, toward),
            _ => own,
        }
    }
}

/// A colour most of the way to `toward`: still legible, but fallen back.
fn faded(own: Rgb, toward: Rgb) -> Rgb {
    let mix = |own: u8, toward: u8| (f32::from(own) * 0.3 + f32::from(toward) * 0.7).round() as u8;
    Rgb {
        r: mix(own.r, toward.r),
        g: mix(own.g, toward.g),
        b: mix(own.b, toward.b),
    }
}

#[derive(Clone, Copy, Debug)]
pub struct GridPadding {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl GridMetrics {
    pub fn from_font(font_family: &str, font_size: f32, text_system: &TextSystem) -> Self {
        let font_id = text_system.resolve_font(&font(font_family.to_owned()));
        let measured_advance = text_system
            .advance(font_id, px(font_size), '0')
            .ok()
            .map(|advance| f32::from(advance.width))
            .filter(|advance| advance.is_finite() && *advance > 0.0);

        // The PTY, libghostty replica, cursor, backgrounds, and GPUI glyph
        // origins must all share the resolved face's exact advance. Rounding
        // this value (or assuming the usual 0.6em) accumulates visible error
        // across long runs and makes right-aligned prompts drift.
        Self {
            cell_width: measured_advance.unwrap_or(font_size * 0.6),
            cell_height: (font_size * 1.42 * 2.0).round() / 2.0,
            font_size,
            padding_x: 2.0,
            padding_y: 2.0,
        }
    }

    pub fn balanced_padding(self, width: f32, height: f32, columns: u16, rows: u16) -> GridPadding {
        let (left, right) = balanced_axis_padding(
            width,
            self.cell_width * f32::from(columns),
            self.cell_width,
            self.padding_x,
        );
        let (top, bottom) = balanced_axis_padding(
            height,
            self.cell_height * f32::from(rows),
            self.cell_height,
            self.padding_y,
        );
        GridPadding {
            top,
            right,
            bottom,
            left,
        }
    }
}

fn balanced_axis_padding(surface: f32, grid: f32, cell: f32, fallback: f32) -> (f32, f32) {
    let remainder = surface - grid;
    if remainder >= fallback * 2.0 && remainder < cell + fallback * 2.0 {
        let balanced = remainder / 2.0;
        (balanced, balanced)
    } else {
        (fallback, (remainder - fallback).max(fallback))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RunStyle {
    foreground: Rgb,
    style: CellStyle,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ShapedRunKey {
    text: SharedString,
    foreground: u32,
    rendition: u8,
    underline: u8,
}

struct CachedShapedRun {
    line: ShapedLine,
    last_used: u64,
}

/// Per-pane text shaping cache. Terminal scrolls normally move existing rows
/// rather than changing their contents, so keeping the current and previous
/// viewport's runs avoids reshaping almost every glyph on every wheel frame.
/// Older runs are evicted to keep unique terminal output from growing this
/// cache with the size of scrollback.
#[derive(Default)]
pub struct TerminalRenderCache {
    font_key: Option<(String, u32)>,
    generation: u64,
    shaped_runs: HashMap<ShapedRunKey, CachedShapedRun>,
}

impl TerminalRenderCache {
    fn begin_frame(&mut self, font_family: &str, font_size: f32) {
        let font_size = font_size.to_bits();
        if !self
            .font_key
            .as_ref()
            .is_some_and(|(family, size)| family == font_family && *size == font_size)
        {
            self.shaped_runs.clear();
            self.font_key = Some((font_family.to_owned(), font_size));
        }
        if self.generation == u64::MAX {
            self.shaped_runs.clear();
            self.generation = 0;
        }
        self.generation += 1;
    }

    fn shape_run(
        &mut self,
        text: String,
        run_style: RunStyle,
        font_family: &str,
        metrics: GridMetrics,
        window: &mut Window,
    ) -> ShapedLine {
        let foreground = run_style.foreground;
        let style = run_style.style;
        let key = ShapedRunKey {
            text: text.into(),
            foreground: (u32::from(foreground.r) << 16)
                | (u32::from(foreground.g) << 8)
                | u32::from(foreground.b),
            rendition: u8::from(style.bold)
                | (u8::from(style.italic) << 1)
                | (u8::from(style.faint) << 2)
                | (u8::from(style.strikethrough) << 3),
            underline: style.underline,
        };
        if let Some(cached) = self.shaped_runs.get_mut(&key) {
            cached.last_used = self.generation;
            return cached.line.clone();
        }

        let mut run_font = font(font_family.to_owned());
        apply_font_style(&mut run_font, style);
        let color = terminal_color(foreground).alpha(if style.faint { 0.58 } else { 1.0 });
        let underline = (style.underline != 0).then_some(UnderlineStyle {
            color: Some(color),
            thickness: px(1.0),
            wavy: style.underline == 3,
        });
        let strikethrough = style.strikethrough.then_some(StrikethroughStyle {
            color: Some(color),
            thickness: px(1.0),
        });
        let text_run = TextRun {
            len: key.text.len(),
            font: run_font,
            color,
            background_color: None,
            underline,
            strikethrough,
        };
        let line = window.text_system().shape_line(
            key.text.clone(),
            px(metrics.font_size),
            &[text_run],
            None,
        );
        self.shaped_runs.insert(
            key,
            CachedShapedRun {
                line: line.clone(),
                last_used: self.generation,
            },
        );
        line
    }

    fn finish_frame(&mut self) {
        let oldest = self.generation.saturating_sub(1);
        self.shaped_runs
            .retain(|_, cached| cached.last_used >= oldest);
    }
}

struct PreparedRun {
    column: usize,
    row: usize,
    line: ShapedLine,
}

struct PreparedTerminal {
    runs: Vec<PreparedRun>,
}

/// Everything a pane's terminal is drawn from, but the frame itself, which
/// `frame_serial` stands for.
#[derive(Clone, Debug, PartialEq)]
pub struct TerminalProps {
    pub frame_serial: u64,
    pub font_family: String,
    pub metrics: GridMetrics,
    pub focused: bool,
    pub chrome: TerminalChrome,
    pub marks: Rc<TerminalMarks>,
    /// Not drawn here, but a parent's opacity is baked into what is drawn,
    /// so a fading pane cannot reuse what it drew a moment ago.
    pub opacity: f32,
}

/// A pane's terminal as a view of its own. GPUI keeps what a view drew until
/// the view changes, so a pane is given a new view whenever its props do,
/// and the other panes are not drawn again when one of them prints.
pub struct TerminalView {
    props: TerminalProps,
    // Weak, so the pane can still update its frame in place.
    frame: Weak<RenderFrame>,
    cache: Rc<RefCell<TerminalRenderCache>>,
}

impl TerminalView {
    pub fn new(
        props: TerminalProps,
        frame: &Rc<RenderFrame>,
        cache: Rc<RefCell<TerminalRenderCache>>,
    ) -> Self {
        Self {
            props,
            frame: Rc::downgrade(frame),
            cache,
        }
    }

    pub const fn props(&self) -> &TerminalProps {
        &self.props
    }
}

impl Render for TerminalView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let props = &self.props;
        // A frame updated since is drawn by the pane's next view instead.
        div()
            .size_full()
            .children(self.frame.upgrade().map(|frame| {
                terminal_canvas(
                    frame,
                    Rc::clone(&self.cache),
                    props.font_family.clone(),
                    props.metrics,
                    props.focused,
                    props.chrome,
                    Rc::clone(&props.marks),
                )
            }))
    }
}

pub fn terminal_canvas(
    frame: Rc<RenderFrame>,
    cache: Rc<RefCell<TerminalRenderCache>>,
    font_family: String,
    metrics: GridMetrics,
    focused: bool,
    chrome: TerminalChrome,
    marks: Rc<TerminalMarks>,
) -> impl IntoElement {
    let prepaint_frame = Rc::clone(&frame);
    let paint_frame = frame;
    let prepaint_marks = Rc::clone(&marks);
    canvas(
        move |_, window, _| {
            prepare_runs(
                &prepaint_frame,
                &cache,
                &font_family,
                metrics,
                &prepaint_marks,
                window,
            )
        },
        move |bounds, prepared, window, cx| {
            paint_terminal(
                bounds,
                &paint_frame,
                prepared,
                metrics,
                focused,
                chrome,
                &marks,
                window,
                cx,
            );
        },
    )
    .size_full()
}

fn prepare_runs(
    frame: &RenderFrame,
    cache: &Rc<RefCell<TerminalRenderCache>>,
    font_family: &str,
    metrics: GridMetrics,
    marks: &TerminalMarks,
    window: &mut Window,
) -> PreparedTerminal {
    let columns = usize::from(frame.cols);
    let mut runs = Vec::new();
    let mut cache = cache.borrow_mut();
    cache.begin_frame(font_family, metrics.font_size);

    for row in 0..usize::from(frame.rows) {
        let mut column = 0;
        while column < columns {
            let cell = &frame.cells[row * columns + column];
            if !is_visible_cell(cell) {
                column += 1;
                continue;
            }

            let start = column;
            let foreground_at = |column: usize, cell: &RenderCell| {
                marks.text_at(row, column, effective_foreground(cell))
            };
            let run_style = RunStyle {
                foreground: foreground_at(column, cell),
                style: cell.style,
            };
            let mut text = String::new();
            push_cell_text(&mut text, cell);
            column += 1;

            // Wide and fallback glyphs are isolated so the next run is always
            // anchored to its libghostty column, independent of shaped width.
            if cell.width == CellWidth::Narrow {
                while column < columns {
                    let next = &frame.cells[row * columns + column];
                    let next_style = RunStyle {
                        foreground: foreground_at(column, next),
                        style: next.style,
                    };
                    if next.width != CellWidth::Narrow
                        || !is_visible_cell(next)
                        || next_style != run_style
                    {
                        break;
                    }
                    push_cell_text(&mut text, next);
                    column += 1;
                }
            }

            let line = cache.shape_run(text, run_style, font_family, metrics, window);
            runs.push(PreparedRun {
                column: start,
                row,
                line,
            });
        }
    }
    cache.finish_frame();

    PreparedTerminal { runs }
}

#[allow(clippy::too_many_arguments)] // One flat paint pass is clearer than a bundle struct.
fn paint_terminal(
    bounds: Bounds<Pixels>,
    frame: &RenderFrame,
    prepared: PreparedTerminal,
    metrics: GridMetrics,
    focused: bool,
    chrome: TerminalChrome,
    marks: &TerminalMarks,
    window: &mut Window,
    cx: &mut App,
) {
    // Default-background cells take the slab colour so the terminal reads as
    // part of its pane; explicit backgrounds still paint below.
    window.paint_quad(fill(bounds, chrome.surface));
    let columns = usize::from(frame.cols);
    let padding = metrics.balanced_padding(
        f32::from(bounds.size.width),
        f32::from(bounds.size.height),
        frame.cols,
        frame.rows,
    );
    let origin = point(
        bounds.origin.x + px(padding.left),
        bounds.origin.y + px(padding.top),
    );

    // A full-screen program usually paints every cell, so each run of one
    // fill along a row is one quad rather than one per cell.
    let fill_of = |cell: &RenderCell| {
        let background = effective_background(cell);
        (background != frame.background || cell.selected).then_some((background, cell.selected))
    };
    for row in 0..usize::from(frame.rows) {
        let cells = &frame.cells[row * columns..(row + 1) * columns];
        let mut column = 0;
        while column < columns {
            let Some(run) = fill_of(&cells[column]) else {
                column += 1;
                continue;
            };
            let start = column;
            column += 1;
            while column < columns && fill_of(&cells[column]) == Some(run) {
                column += 1;
            }
            let (background, selected) = run;
            let color = if selected {
                selection_color(background)
            } else {
                terminal_color(background)
            };
            // Both edges from the grid, so neighbouring runs meet exactly.
            window.paint_quad(fill(
                Bounds::from_corners(
                    point(
                        origin.x + px(metrics.cell_width) * start,
                        origin.y + px(metrics.cell_height) * row,
                    ),
                    point(
                        origin.x + px(metrics.cell_width) * column,
                        origin.y + px(metrics.cell_height) * (row + 1),
                    ),
                ),
                color,
            ));
        }
    }

    paint_marks(origin, metrics, marks, window);

    for run in prepared.runs {
        let position = point(
            origin.x + px(metrics.cell_width) * run.column,
            origin.y + px(metrics.cell_height) * run.row,
        );
        let _ = run.line.paint(
            position,
            px(metrics.cell_height),
            gpui::TextAlign::Left,
            None,
            window,
            cx,
        );
    }

    if focused
        && let Some(cursor) = frame.cursor
        && cursor.visible
        && cursor.x < frame.cols
        && cursor.y < frame.rows
    {
        let cell_bounds = Bounds::new(
            point(
                origin.x + px(metrics.cell_width) * usize::from(cursor.x),
                origin.y + px(metrics.cell_height) * usize::from(cursor.y),
            ),
            size(px(metrics.cell_width), px(metrics.cell_height)),
        );
        let cursor_color = chrome.cursor.alpha(0.88);
        let cursor_bounds = match cursor.style {
            CursorStyle::Block => cell_bounds,
            CursorStyle::HollowBlock => {
                window.paint_quad(gpui::outline(
                    cell_bounds,
                    cursor_color,
                    gpui::BorderStyle::Solid,
                ));
                return;
            }
            CursorStyle::Bar => {
                Bounds::new(cell_bounds.origin, size(px(2.0), cell_bounds.size.height))
            }
            CursorStyle::Underline => Bounds::new(
                point(cell_bounds.origin.x, cell_bounds.bottom() - px(2.0)),
                size(cell_bounds.size.width, px(2.0)),
            ),
        };
        window.paint_quad(fill(cursor_bounds, cursor_color));
    }
}

/// Marks sit between the backgrounds and the text, a hair inside their row
/// so marks on neighbouring rows read as separate slips.
fn paint_marks(
    origin: gpui::Point<Pixels>,
    metrics: GridMetrics,
    marks: &TerminalMarks,
    window: &mut Window,
) {
    for span in &marks.spans {
        let bounds = Bounds::new(
            point(
                origin.x + px(metrics.cell_width) * span.start - px(1.0),
                origin.y + px(metrics.cell_height) * span.row + px(1.0),
            ),
            size(
                px(metrics.cell_width) * (span.end - span.start) + px(2.0),
                px(metrics.cell_height) - px(2.0),
            ),
        );
        let color = if span.current {
            marks.current
        } else {
            marks.wash
        };
        window.paint_quad(fill(bounds, color).corner_radii(px(3.0)));
    }
}

fn is_visible_cell(cell: &RenderCell) -> bool {
    !cell.style.invisible
        && !cell.grapheme.is_empty()
        && !matches!(cell.width, CellWidth::SpacerTail | CellWidth::SpacerHead)
}

fn push_cell_text(output: &mut String, cell: &RenderCell) {
    if cell.grapheme.is_empty() {
        output.push(' ');
    } else {
        output.push_str(&cell.grapheme);
    }
}

fn apply_font_style(font: &mut Font, style: CellStyle) {
    if style.bold {
        font.weight = gpui::FontWeight::BOLD;
    }
    if style.italic {
        font.style = gpui::FontStyle::Italic;
    }
}

fn effective_foreground(cell: &RenderCell) -> Rgb {
    if cell.style.inverse {
        cell.background
    } else {
        cell.foreground
    }
}

fn effective_background(cell: &RenderCell) -> Rgb {
    if cell.style.inverse {
        cell.foreground
    } else {
        cell.background
    }
}

fn terminal_color(color: Rgb) -> Hsla {
    gpui::rgb((u32::from(color.r) << 16) | (u32::from(color.g) << 8) | u32::from(color.b)).into()
}

fn selection_color(background: Rgb) -> Hsla {
    let base = terminal_color(background);
    // A warm translucent selection remains legible across arbitrary terminal
    // themes without replacing the application's actual ANSI colours.
    base.blend(gpui::rgba(0xf29a_6b4f).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balances_only_the_normal_sub_cell_remainder() {
        assert_eq!(balanced_axis_padding(100.0, 90.0, 10.0, 2.0), (5.0, 5.0));
        assert_eq!(balanced_axis_padding(100.0, 80.0, 10.0, 2.0), (2.0, 18.0));
    }
}
