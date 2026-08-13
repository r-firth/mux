use std::{cell::RefCell, collections::HashMap, rc::Rc};

use gpui::{
    App, Bounds, Font, Hsla, IntoElement, Pixels, ShapedLine, SharedString, StrikethroughStyle,
    Styled, TextRun, TextSystem, UnderlineStyle, Window, canvas, fill, font, point, px, size,
};
use mux_terminal::{CellStyle, CellWidth, CursorStyle, RenderCell, RenderFrame, Rgb};

#[derive(Clone, Copy, Debug)]
pub struct GridMetrics {
    pub cell_width: f32,
    pub cell_height: f32,
    pub font_size: f32,
    pub padding_x: f32,
    pub padding_y: f32,
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

pub fn terminal_canvas(
    frame: Rc<RenderFrame>,
    cache: Rc<RefCell<TerminalRenderCache>>,
    font_family: String,
    metrics: GridMetrics,
    focused: bool,
) -> impl IntoElement {
    let prepaint_frame = Rc::clone(&frame);
    let paint_frame = frame;
    canvas(
        move |_, window, _| prepare_runs(&prepaint_frame, &cache, &font_family, metrics, window),
        move |bounds, prepared, window, cx| {
            paint_terminal(bounds, &paint_frame, prepared, metrics, focused, window, cx);
        },
    )
    .size_full()
}

fn prepare_runs(
    frame: &RenderFrame,
    cache: &Rc<RefCell<TerminalRenderCache>>,
    font_family: &str,
    metrics: GridMetrics,
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
            let run_style = RunStyle {
                foreground: effective_foreground(cell),
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
                        foreground: effective_foreground(next),
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

fn paint_terminal(
    bounds: Bounds<Pixels>,
    frame: &RenderFrame,
    prepared: PreparedTerminal,
    metrics: GridMetrics,
    focused: bool,
    window: &mut Window,
    cx: &mut App,
) {
    window.paint_quad(fill(bounds, terminal_color(frame.background)));
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

    for row in 0..usize::from(frame.rows) {
        for column in 0..columns {
            let cell = &frame.cells[row * columns + column];
            let background = effective_background(cell);
            if background != frame.background || cell.selected {
                let color = if cell.selected {
                    selection_color(background)
                } else {
                    terminal_color(background)
                };
                window.paint_quad(fill(
                    Bounds::new(
                        point(
                            origin.x + px(metrics.cell_width) * column,
                            origin.y + px(metrics.cell_height) * row,
                        ),
                        size(px(metrics.cell_width), px(metrics.cell_height)),
                    ),
                    color,
                ));
            }
        }
    }

    for run in prepared.runs {
        let position = point(
            origin.x + px(metrics.cell_width) * run.column,
            origin.y + px(metrics.cell_height) * run.row,
        );
        let _ = run
            .line
            .paint(position, px(metrics.cell_height), window, cx);
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
        let cursor_color = terminal_color(cursor.color).alpha(0.88);
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
    // A cool translucent selection remains legible across arbitrary terminal
    // themes without replacing the application's actual ANSI colours.
    base.blend(gpui::rgba(0x68bd_e84f).into())
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
