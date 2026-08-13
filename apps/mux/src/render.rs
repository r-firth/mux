// The renderer intentionally converts bounded desktop-pixel/grid coordinates
// to wgpu's f32 coordinate space after clamping where sign could be ambiguous.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use bytemuck::{Pod, Zeroable};
use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, Style,
    SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport, Weight, Wrap,
};
use mux_acp::{
    AgentConfigCategory, AgentConfigValue, AgentMessageRole, AgentProfile, AgentSessionSnapshot,
    AgentSessionStatus, AgentSlashCommand, AgentTimelineItem, PlanStatus, ToolStatus,
};
use mux_protocol::SessionSummary;
use mux_terminal::{
    CellStyle, CellWidth, RenderCell, RenderDirty, RenderFrame, Rgb, TerminalMouseGeometry,
    TerminalPoint, TerminalSelectionGeometry, TerminalSurfacePosition,
};
use mux_terminal_ghostty::GhosttyFont;
use mux_workspace::{InputMode, PaneId, Session};
use tracing::info;
use unicode_width::UnicodeWidthStr;
use wgpu::{
    BlendState, BufferDescriptor, BufferUsages, ColorTargetState, ColorWrites,
    CommandEncoderDescriptor, CompositeAlphaMode, DeviceDescriptor, FragmentState, Instance,
    InstanceDescriptor, LoadOp, MultisampleState, Operations, PipelineCompilationOptions,
    PresentMode, PrimitiveState, RenderPassColorAttachment, RenderPassDescriptor, RenderPipeline,
    RenderPipelineDescriptor, RequestAdapterOptions, StoreOp, Surface, SurfaceConfiguration,
    TextureFormat, TextureUsages, TextureViewDescriptor, VertexState,
};
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::window::{CursorIcon, Window};

use crate::layout::{
    PANE_PADDING_X, PANE_PADDING_Y, PaneGeometry, Rect, TAB_BAR_HEIGHT, WorkspaceGeometry,
};

const TERMINAL_FONT_FAMILY: &str = "JetBrainsMono Nerd Font Mono";
const FONT_SIZE: f32 = 14.0;
// JetBrains Mono's advance is exactly 0.6 em. Grid geometry and shaped text
// must use the same value or the cursor and colored cell backgrounds drift.
const CELL_WIDTH: f32 = FONT_SIZE * 0.6;
const CELL_HEIGHT: f32 = 20.0;

const WINDOW_BACKGROUND: [f32; 4] = [0.035, 0.039, 0.047, 1.0];
const TAB_BACKGROUND: [f32; 4] = [0.047, 0.052, 0.062, 1.0];
const TAB_ACTIVE: [f32; 4] = [0.082, 0.090, 0.105, 1.0];
const BORDER: [f32; 4] = [0.15, 0.16, 0.18, 1.0];
const FOCUS: [f32; 4] = [0.35, 0.63, 0.96, 0.82];
const SELECTION: [f32; 4] = [0.24, 0.43, 0.68, 0.72];
const MODE_BACKGROUND: [f32; 4] = [0.065, 0.072, 0.086, 0.98];
const SCROLL_THUMB: [f32; 4] = [0.50, 0.57, 0.68, 0.62];
const OVERLAY_SCRIM: [f32; 4] = [0.0, 0.0, 0.0, 0.58];
const OVERLAY_BACKGROUND: [f32; 4] = [0.075, 0.082, 0.098, 0.99];
const OVERLAY_SELECTED: [f32; 4] = [0.13, 0.22, 0.34, 1.0];
const AGENT_BACKGROUND: [f32; 4] = [0.018, 0.021, 0.028, 0.995];
const AGENT_COMPOSER: [f32; 4] = [0.032, 0.037, 0.048, 1.0];
const AGENT_ACCENT: [f32; 4] = [0.40, 0.72, 0.98, 1.0];

#[derive(Clone, Copy, Debug)]
pub struct TerminalSelectionPointer {
    pub point: Option<TerminalPoint>,
    pub clamped_point: TerminalPoint,
    pub position: TerminalSurfacePosition,
    pub geometry: TerminalSelectionGeometry,
}
const AGENT_PERMISSION: [f32; 4] = [0.075, 0.053, 0.022, 1.0];

#[derive(Clone, Copy)]
pub struct SessionSwitcherView<'a> {
    pub entries: &'a [SessionSummary],
    pub selected: usize,
    pub pending_kill: Option<mux_workspace::SessionId>,
}

#[derive(Clone, Copy)]
pub struct AgentSurfaceView<'a> {
    pub entries: &'a [AgentSessionSnapshot],
    pub selected: usize,
    pub draft: &'a str,
    pub loading: bool,
    pub progress: f32,
    pub launcher: Option<AgentLauncherView<'a>>,
    pub context_label: &'a str,
    pub notice: Option<&'a str>,
    pub timeline_scroll: usize,
    pub command_suggestions: &'a [AgentSlashCommand],
    pub command_selection: usize,
}

#[derive(Clone, Copy)]
pub struct AgentLauncherView<'a> {
    pub profiles: &'a [AgentProfile],
    pub selected: usize,
    pub cwd_override: Option<&'a Path>,
}

#[derive(Clone, Copy)]
pub struct TextPromptView<'a> {
    pub label: &'a str,
    pub draft: &'a str,
}

#[derive(Clone, Copy)]
pub struct UiState<'a> {
    pub mode: InputMode,
    pub message: Option<&'a str>,
    pub session_switcher: Option<SessionSwitcherView<'a>>,
    pub text_prompt: Option<TextPromptView<'a>>,
    pub agent_surface: Option<AgentSurfaceView<'a>>,
    pub ime_preedit: Option<&'a str>,
    pub hovered_hyperlink: Option<(PaneId, &'a str)>,
    pub cursor_blink_visible: bool,
}

/// Logical composer height for up to five visible wrapped input lines.
#[must_use]
pub fn agent_composer_height(draft: &str) -> f32 {
    let lines = draft
        .split('\n')
        .map(|line| UnicodeWidthStr::width(line).div_ceil(52).max(1))
        .sum::<usize>()
        .clamp(1, 5);
    64.0 + (lines.saturating_sub(1) as f32 * 18.0)
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    position: [f32; 2],
    color: [f32; 4],
}

struct PaneText {
    rows: Vec<TerminalRow>,
    geometry: PaneGeometry,
    cols: u16,
    row_count: u16,
}

#[derive(Default)]
struct TerminalRow {
    runs: Vec<TerminalTextRun>,
}

struct TerminalTextRun {
    buffer: Buffer,
    column: u16,
    offset_x: f32,
}

#[derive(Clone, Copy)]
struct TerminalGridMetrics {
    font_size: f32,
    cell_width: f32,
    cell_height: f32,
}

struct ResolvedTerminalFont {
    family: String,
    size: f32,
    cell_width: f32,
    cell_height: f32,
}

impl ResolvedTerminalFont {
    fn from_ghostty(font_system: &mut FontSystem, requested: &GhosttyFont) -> Self {
        let family = resolve_terminal_font_family(font_system, requested.family.as_deref());
        let size = requested.size.unwrap_or(FONT_SIZE).clamp(6.0, 72.0);
        let cell_height = size * (CELL_HEIGHT / FONT_SIZE);
        let cell_width = measure_terminal_cell_width(font_system, &family, size, cell_height)
            .unwrap_or(size * (CELL_WIDTH / FONT_SIZE));
        info!(
            family,
            size, cell_width, cell_height, "resolved terminal font and grid metrics"
        );
        Self {
            family,
            size,
            cell_width,
            cell_height,
        }
    }
}

struct ChromeText {
    buffer: Buffer,
    rect: Rect,
    color: Color,
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: Surface<'static>,
    config: SurfaceConfiguration,
    rect_pipeline: RenderPipeline,
    font_system: FontSystem,
    swash_cache: SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    text_pipeline: TextRenderer,
    overlay_text_pipeline: TextRenderer,
    pane_text: HashMap<PaneId, PaneText>,
    chrome_text: Vec<ChromeText>,
    overlay_text: Vec<ChromeText>,
    rect_vertices: Vec<Vertex>,
    overlay_rect_vertices: Vec<Vertex>,
    rect_buffer: Option<wgpu::Buffer>,
    rect_buffer_capacity: u64,
    overlay_rect_buffer: Option<wgpu::Buffer>,
    overlay_rect_buffer_capacity: u64,
    terminal_font: ResolvedTerminalFont,
    scale_factor: f32,
    window: Arc<Window>,
}

impl Renderer {
    pub async fn new(window: Arc<Window>, requested_font: GhosttyFont) -> Result<Self> {
        window.set_ime_allowed(true);
        let size = window.inner_size();
        let instance = Instance::new(&InstanceDescriptor::default());
        let surface = instance
            .create_surface(Arc::clone(&window))
            .context("create GPU window surface")?;
        let adapter = instance
            .request_adapter(&RequestAdapterOptions {
                compatible_surface: Some(&surface),
                ..RequestAdapterOptions::default()
            })
            .await
            .context("find a compatible graphics adapter")?;
        let (device, queue) = adapter
            .request_device(&DeviceDescriptor {
                label: Some("mux graphics device"),
                ..DeviceDescriptor::default()
            })
            .await
            .context("create graphics device")?;
        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(TextureFormat::is_srgb)
            .unwrap_or(capabilities.formats[0]);
        let present_mode = if capabilities.present_modes.contains(&PresentMode::AutoVsync) {
            PresentMode::AutoVsync
        } else {
            PresentMode::Fifo
        };
        let config = SurfaceConfiguration {
            usage: TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode,
            alpha_mode: CompositeAlphaMode::Opaque,
            view_formats: Vec::new(),
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let cache = Cache::new(&device);
        let viewport = Viewport::new(&device, &cache);
        let mut atlas = TextAtlas::new(&device, &queue, &cache, format);
        let text_pipeline =
            TextRenderer::new(&mut atlas, &device, MultisampleState::default(), None);
        let overlay_text_pipeline =
            TextRenderer::new(&mut atlas, &device, MultisampleState::default(), None);
        let rect_pipeline = create_rect_pipeline(&device, format);

        let mut font_system = FontSystem::new();
        load_terminal_fonts(&mut font_system);
        let terminal_font = ResolvedTerminalFont::from_ghostty(&mut font_system, &requested_font);

        Ok(Self {
            device,
            queue,
            surface,
            config,
            rect_pipeline,
            font_system,
            swash_cache: SwashCache::new(),
            viewport,
            atlas,
            text_pipeline,
            overlay_text_pipeline,
            pane_text: HashMap::new(),
            chrome_text: Vec::new(),
            overlay_text: Vec::new(),
            rect_vertices: Vec::new(),
            overlay_rect_vertices: Vec::new(),
            rect_buffer: None,
            rect_buffer_capacity: 0,
            overlay_rect_buffer: None,
            overlay_rect_buffer_capacity: 0,
            terminal_font,
            scale_factor: window.scale_factor() as f32,
            window,
        })
    }

    pub fn resize(&mut self, width: u32, height: u32, scale_factor: f64) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.scale_factor = scale_factor as f32;
        self.surface.configure(&self.device, &self.config);
    }

    #[must_use]
    pub fn terminal_size(&self, geometry: PaneGeometry) -> mux_terminal::TerminalSize {
        let geometry = PaneGeometry {
            pane_id: geometry.pane_id,
            rect: scale_rect(geometry.rect, self.scale_factor),
            focused: geometry.focused,
        };
        let cell_width = self.cell_width();
        let cell_height = self.cell_height();
        let content_width =
            (geometry.rect.width - 2.0 * PANE_PADDING_X * self.scale_factor).max(cell_width);
        let content_height =
            (geometry.rect.height - 2.0 * PANE_PADDING_Y * self.scale_factor).max(cell_height);
        mux_terminal::TerminalSize {
            cols: (content_width / cell_width)
                .floor()
                .clamp(1.0, f32::from(u16::MAX)) as u16,
            rows: (content_height / cell_height)
                .floor()
                .clamp(1.0, f32::from(u16::MAX)) as u16,
            cell_width_px: cell_width.round() as u32,
            cell_height_px: cell_height.round() as u32,
        }
    }

    #[must_use]
    pub fn terminal_point_at(
        &self,
        geometry: PaneGeometry,
        physical_x: f32,
        physical_y: f32,
    ) -> Option<TerminalPoint> {
        let size = self.terminal_size(geometry);
        let geometry = scale_rect(geometry.rect, self.scale_factor);
        terminal_point_in_grid(
            physical_x,
            physical_y,
            (
                geometry.x + PANE_PADDING_X * self.scale_factor,
                geometry.y + PANE_PADDING_Y * self.scale_factor,
            ),
            (self.cell_width(), self.cell_height()),
            size.cols,
            size.rows,
        )
    }

    #[must_use]
    pub fn terminal_mouse_geometry(
        &self,
        geometry: PaneGeometry,
        physical_x: f32,
        physical_y: f32,
    ) -> (TerminalMouseGeometry, f32, f32) {
        let rect = scale_rect(geometry.rect, self.scale_factor);
        let horizontal_padding = (PANE_PADDING_X * self.scale_factor).round() as u32;
        let vertical_padding = (PANE_PADDING_Y * self.scale_factor).round() as u32;
        (
            TerminalMouseGeometry {
                screen_width: rect.width.round().max(1.0) as u32,
                screen_height: rect.height.round().max(1.0) as u32,
                cell_width: self.cell_width().round().max(1.0) as u32,
                cell_height: self.cell_height().round().max(1.0) as u32,
                padding_top: vertical_padding,
                padding_bottom: vertical_padding,
                padding_right: horizontal_padding,
                padding_left: horizontal_padding,
            },
            physical_x - rect.x,
            physical_y - rect.y,
        )
    }

    #[must_use]
    pub fn terminal_selection_pointer(
        &self,
        geometry: PaneGeometry,
        physical_x: f32,
        physical_y: f32,
    ) -> TerminalSelectionPointer {
        let size = self.terminal_size(geometry);
        let rect = scale_rect(geometry.rect, self.scale_factor);
        let cell_width = self.cell_width();
        let cell_height = self.cell_height();
        let padding_left = PANE_PADDING_X * self.scale_factor;
        let padding_top = PANE_PADDING_Y * self.scale_factor;
        let relative_grid_x = physical_x - rect.x - padding_left;
        let relative_grid_y = physical_y - rect.y - padding_top;
        let clamped_column = (relative_grid_x / cell_width)
            .floor()
            .clamp(0.0, f32::from(size.cols.saturating_sub(1))) as u16;
        let clamped_row = (relative_grid_y / cell_height)
            .floor()
            .clamp(0.0, f32::from(size.rows.saturating_sub(1))) as u16;
        TerminalSelectionPointer {
            point: terminal_point_in_grid(
                physical_x,
                physical_y,
                (rect.x + padding_left, rect.y + padding_top),
                (cell_width, cell_height),
                size.cols,
                size.rows,
            ),
            clamped_point: TerminalPoint {
                column: clamped_column,
                row: clamped_row,
            },
            position: TerminalSurfacePosition {
                x: f64::from(physical_x - rect.x),
                y: f64::from(physical_y - rect.y),
            },
            geometry: TerminalSelectionGeometry {
                columns: u32::from(size.cols),
                cell_width: cell_width.round().max(1.0) as u32,
                padding_left: padding_left.round().max(0.0) as u32,
                screen_height: rect.height.round().max(1.0) as u32,
            },
        }
    }

    #[allow(clippy::too_many_lines)]
    pub fn sync(
        &mut self,
        session: &Session,
        geometry: &WorkspaceGeometry,
        frames: &HashMap<PaneId, &RenderFrame>,
        changed_panes: &HashSet<PaneId>,
        ui: &UiState<'_>,
    ) {
        self.rect_vertices.clear();
        self.chrome_text.clear();
        self.overlay_rect_vertices.clear();
        self.overlay_text.clear();
        push_rect(
            &mut self.rect_vertices,
            Rect {
                x: 0.0,
                y: 0.0,
                width: self.config.width as f32,
                height: self.config.height as f32,
            },
            WINDOW_BACKGROUND,
            self.config.width,
            self.config.height,
        );
        push_rect(
            &mut self.rect_vertices,
            Rect {
                x: 0.0,
                y: 0.0,
                width: self.config.width as f32,
                height: TAB_BAR_HEIGHT * self.scale_factor,
            },
            TAB_BACKGROUND,
            self.config.width,
            self.config.height,
        );

        for tab in &geometry.tabs {
            let rect = scale_rect(tab.rect, self.scale_factor);
            if tab.active {
                push_rect(
                    &mut self.rect_vertices,
                    rect,
                    TAB_ACTIVE,
                    self.config.width,
                    self.config.height,
                );
            }
            self.chrome_text.push(make_text(
                &mut self.font_system,
                &tab.title,
                Rect {
                    x: rect.x + 8.0 * self.scale_factor,
                    y: rect.y + self.scale_factor,
                    width: rect.width - 16.0 * self.scale_factor,
                    height: rect.height - 2.0 * self.scale_factor,
                },
                12.0 * self.scale_factor,
                Color::rgb(218, 222, 231),
                Family::SansSerif,
                Weight::MEDIUM,
            ));
        }

        self.pane_text.retain(|pane_id, _| {
            geometry
                .panes
                .iter()
                .any(|geometry| geometry.pane_id == *pane_id)
        });
        let font_size = self.font_size();
        let cell_width = self.cell_width();
        let cell_height = self.cell_height();
        let terminal_metrics = TerminalGridMetrics {
            font_size,
            cell_width,
            cell_height,
        };
        for pane in &geometry.panes {
            let scaled_geometry = PaneGeometry {
                pane_id: pane.pane_id,
                rect: scale_rect(pane.rect, self.scale_factor),
                focused: pane.focused,
            };
            if let Some(frame) = frames.get(&pane.pane_id) {
                let hovered_hyperlink = ui
                    .hovered_hyperlink
                    .and_then(|(pane_id, uri)| (pane_id == pane.pane_id).then_some(uri));
                self.add_terminal_rectangles(
                    scaled_geometry,
                    frame,
                    hovered_hyperlink,
                    ui.cursor_blink_visible,
                );
                let previous = self.pane_text.remove(&pane.pane_id);
                let rebuild_all = previous.as_ref().is_none_or(|text| {
                    text.geometry.rect != scaled_geometry.rect
                        || text.cols != frame.cols
                        || text.row_count != frame.rows
                }) || matches!(frame.dirty, RenderDirty::Full);
                let mut rows = previous.map_or_else(Vec::new, |text| text.rows);
                resize_row_buffers(
                    &mut self.font_system,
                    &mut rows,
                    usize::from(frame.rows),
                    font_size,
                    cell_height,
                );
                if rebuild_all || changed_panes.contains(&pane.pane_id) {
                    update_terminal_rows(
                        &mut self.font_system,
                        &mut rows,
                        frame,
                        &self.terminal_font.family,
                        terminal_metrics,
                        rebuild_all,
                    );
                }
                self.pane_text.insert(
                    pane.pane_id,
                    PaneText {
                        rows,
                        geometry: scaled_geometry,
                        cols: frame.cols,
                        row_count: frame.rows,
                    },
                );
            }
        }

        if let Some(mode_bar) = geometry.mode_bar {
            let rect = scale_rect(mode_bar, self.scale_factor);
            push_rect(
                &mut self.overlay_rect_vertices,
                rect,
                MODE_BACKGROUND,
                self.config.width,
                self.config.height,
            );
            let label = match ui.mode {
                InputMode::Pane => {
                    "PANE   h j k l focus · d down · r right · n new · f zoom · x close"
                }
                InputMode::Tab => "TAB    h j k l switch · 1–9 select · n new · r rename · x close",
                InputMode::Session => "SESSION   w sessions · d detach · Esc return",
                InputMode::Resize => "RESIZE   h j k l / arrows grow · Esc return",
                InputMode::Normal => "",
            };
            self.overlay_text.push(make_text(
                &mut self.font_system,
                label,
                Rect {
                    x: 12.0 * self.scale_factor,
                    y: rect.y + 4.0 * self.scale_factor,
                    width: rect.width - 24.0 * self.scale_factor,
                    height: rect.height - 8.0 * self.scale_factor,
                },
                12.0 * self.scale_factor,
                Color::rgb(190, 205, 228),
                Family::SansSerif,
                Weight::MEDIUM,
            ));
        }

        if let Some(message) = ui.message
            && ui.agent_surface.is_none()
        {
            self.add_toast(message);
        }

        if let Some(switcher) = ui.session_switcher {
            self.add_session_switcher(switcher);
        }

        if let Some(prompt) = ui.text_prompt {
            self.add_text_prompt(prompt);
        }

        if let Some(agent) = ui.agent_surface {
            self.add_agent_surface(agent);
        }

        self.update_ime(
            geometry,
            frames,
            ui.agent_surface,
            ui.text_prompt,
            ui.ime_preedit,
        );

        let _ = session;
    }

    fn update_ime(
        &mut self,
        geometry: &WorkspaceGeometry,
        frames: &HashMap<PaneId, &RenderFrame>,
        agent_surface: Option<AgentSurfaceView<'_>>,
        text_prompt: Option<TextPromptView<'_>>,
        preedit: Option<&str>,
    ) {
        let cursor = if let Some(prompt) = text_prompt {
            Some(self.text_prompt_cursor_rect(prompt))
        } else {
            agent_surface.map_or_else(
                || self.focused_terminal_cursor_rect(geometry, frames),
                |surface| Some(self.agent_composer_cursor_rect(surface)),
            )
        };
        let Some(cursor) = cursor else {
            return;
        };
        self.window.set_ime_cursor_area(
            PhysicalPosition::new(f64::from(cursor.x), f64::from(cursor.y)),
            PhysicalSize::new(
                cursor.width.max(1.0).round() as u32,
                cursor.height.max(1.0).round() as u32,
            ),
        );
        if let Some(preedit) = preedit.filter(|value| !value.is_empty()) {
            self.add_ime_preedit(cursor, preedit);
        }
    }

    fn focused_terminal_cursor_rect(
        &self,
        geometry: &WorkspaceGeometry,
        frames: &HashMap<PaneId, &RenderFrame>,
    ) -> Option<Rect> {
        let pane = geometry.panes.iter().find(|pane| pane.focused)?;
        let cursor = frames.get(&pane.pane_id)?.cursor?;
        let pane = scale_rect(pane.rect, self.scale_factor);
        Some(Rect {
            x: pane.x
                + PANE_PADDING_X * self.scale_factor
                + f32::from(cursor.x) * self.cell_width(),
            y: pane.y
                + PANE_PADDING_Y * self.scale_factor
                + f32::from(cursor.y) * self.cell_height(),
            width: self.cell_width(),
            height: self.cell_height(),
        })
    }

    fn agent_composer_cursor_rect(&self, surface: AgentSurfaceView<'_>) -> Rect {
        let scale = self.scale_factor;
        let window_width = self.config.width as f32;
        let window_height = self.config.height as f32;
        let progress = 1.0 - (1.0 - surface.progress.clamp(0.0, 1.0)).powi(3);
        let panel_width = (480.0 * scale).min(window_width * 0.62);
        Rect {
            x: window_width - panel_width * progress + 30.0 * scale,
            y: window_height - 54.0 * scale,
            width: 2.0 * scale,
            height: 20.0 * scale,
        }
    }

    fn add_ime_preedit(&mut self, cursor: Rect, preedit: &str) {
        let scale = self.scale_factor;
        let font_size = self.font_size();
        let estimated_width = (preedit.chars().count().max(1) as f32 * self.cell_width()
            + 10.0 * scale)
            .min(self.config.width as f32 - 16.0 * scale);
        let height = self.cell_height().max(22.0 * scale);
        let x = cursor
            .x
            .min((self.config.width as f32 - estimated_width - 8.0 * scale).max(8.0 * scale));
        let y = if cursor.y + height <= self.config.height as f32 {
            cursor.y
        } else {
            (cursor.y - height).max(TAB_BAR_HEIGHT * scale)
        };
        let rect = Rect {
            x,
            y,
            width: estimated_width,
            height,
        };
        push_rect(
            &mut self.overlay_rect_vertices,
            rect,
            [0.055, 0.061, 0.074, 0.99],
            self.config.width,
            self.config.height,
        );
        push_rect(
            &mut self.overlay_rect_vertices,
            Rect {
                y: rect.y + rect.height - 2.0 * scale,
                height: 2.0 * scale,
                ..rect
            },
            AGENT_ACCENT,
            self.config.width,
            self.config.height,
        );
        self.overlay_text.push(make_text(
            &mut self.font_system,
            preedit,
            Rect {
                x: rect.x + 5.0 * scale,
                y: rect.y,
                width: rect.width - 10.0 * scale,
                height: rect.height - 2.0 * scale,
            },
            font_size,
            Color::rgb(235, 239, 247),
            Family::Name(&self.terminal_font.family),
            Weight::NORMAL,
        ));
    }

    fn add_toast(&mut self, message: &str) {
        let scale = self.scale_factor;
        let width = (420.0 * scale).min(self.config.width as f32 - 28.0 * scale);
        let rect = Rect {
            x: self.config.width as f32 - width - 14.0 * scale,
            y: (TAB_BAR_HEIGHT + 12.0) * scale,
            width,
            height: 38.0 * scale,
        };
        push_rect(
            &mut self.overlay_rect_vertices,
            rect,
            [0.09, 0.10, 0.12, 0.97],
            self.config.width,
            self.config.height,
        );
        push_rect(
            &mut self.overlay_rect_vertices,
            Rect {
                width: 2.0 * scale,
                ..rect
            },
            AGENT_ACCENT,
            self.config.width,
            self.config.height,
        );
        self.overlay_text.push(make_text(
            &mut self.font_system,
            message,
            Rect {
                x: rect.x + 12.0 * scale,
                y: rect.y + 7.0 * scale,
                width: rect.width - 20.0 * scale,
                height: 24.0 * scale,
            },
            12.0 * scale,
            Color::rgb(224, 230, 241),
            Family::SansSerif,
            Weight::MEDIUM,
        ));
    }

    #[allow(clippy::too_many_lines)]
    fn add_agent_surface(&mut self, view: AgentSurfaceView<'_>) {
        let scale = self.scale_factor;
        let window_width = self.config.width as f32;
        let window_height = self.config.height as f32;
        let progress = 1.0 - (1.0 - view.progress.clamp(0.0, 1.0)).powi(3);
        let panel_width = (480.0 * scale).min(window_width * 0.62);
        let panel = Rect {
            x: window_width - panel_width * progress,
            y: TAB_BAR_HEIGHT * scale,
            width: panel_width,
            height: window_height - TAB_BAR_HEIGHT * scale,
        };
        push_rect(
            &mut self.overlay_rect_vertices,
            Rect {
                x: 0.0,
                y: panel.y,
                width: panel.x.max(0.0),
                height: panel.height,
            },
            [0.0, 0.0, 0.0, 0.18 * progress],
            self.config.width,
            self.config.height,
        );
        push_rect(
            &mut self.overlay_rect_vertices,
            panel,
            AGENT_BACKGROUND,
            self.config.width,
            self.config.height,
        );
        push_rect(
            &mut self.overlay_rect_vertices,
            Rect {
                width: scale.max(1.0),
                ..panel
            },
            [0.22, 0.25, 0.30, progress],
            self.config.width,
            self.config.height,
        );

        let content_x = panel.x + 18.0 * scale;
        let content_width = panel.width - 36.0 * scale;
        self.overlay_text.push(make_text(
            &mut self.font_system,
            "Agents",
            Rect {
                x: content_x,
                y: panel.y + 13.0 * scale,
                width: content_width - 90.0 * scale,
                height: 24.0 * scale,
            },
            15.0 * scale,
            Color::rgb(235, 238, 245),
            Family::SansSerif,
            Weight::SEMIBOLD,
        ));
        self.overlay_text.push(make_text(
            &mut self.font_system,
            "⇧⌘A  close",
            Rect {
                x: panel.x + panel.width - 100.0 * scale,
                y: panel.y + 15.0 * scale,
                width: 82.0 * scale,
                height: 20.0 * scale,
            },
            10.5 * scale,
            Color::rgb(135, 146, 166),
            Family::SansSerif,
            Weight::NORMAL,
        ));

        if let Some(launcher) = view.launcher {
            self.add_agent_launcher(panel, launcher, view.draft, view.loading, view.notice);
            return;
        }

        let Some(agent) = view.entries.get(view.selected) else {
            self.add_empty_agent_surface(panel, view.loading);
            return;
        };
        self.add_agent_header(agent, panel, view.selected, view.entries.len());
        let body_top = if let Some(notice) = view.notice {
            self.add_agent_notice(panel, notice, panel.y + 91.0 * scale);
            130.0
        } else {
            94.0
        };

        let permission = agent.pending_permission();
        let composer_height = permission.map_or_else(
            || agent_composer_height(view.draft),
            |value| 58.0 + value.options.len().min(4) as f32 * 26.0,
        ) * scale;
        let composer = Rect {
            x: content_x,
            y: panel.y + panel.height - composer_height - 16.0 * scale,
            width: content_width,
            height: composer_height,
        };
        if let Some(permission) = permission {
            self.add_agent_permission(permission, composer);
        } else {
            self.add_agent_composer(
                view.draft,
                agent.status,
                &agent.name,
                view.context_label,
                composer,
            );
        }
        let command_height = if view.command_suggestions.is_empty() {
            0.0
        } else {
            (28.0 + view.command_suggestions.len() as f32 * 30.0) * scale
        };
        let command_gap = if command_height > 0.0 {
            8.0 * scale
        } else {
            0.0
        };
        self.add_agent_timeline(
            agent,
            Rect {
                x: content_x,
                y: panel.y + body_top * scale,
                width: content_width,
                height: (composer.y
                    - command_height
                    - command_gap
                    - panel.y
                    - (body_top + 12.0) * scale)
                    .max(1.0),
            },
            view.timeline_scroll,
        );
        if command_height > 0.0 {
            self.add_agent_command_palette(
                view.command_suggestions,
                view.command_selection,
                Rect {
                    x: content_x,
                    y: composer.y - command_height - command_gap,
                    width: content_width,
                    height: command_height,
                },
            );
        }
    }

    fn add_agent_command_palette(
        &mut self,
        commands: &[AgentSlashCommand],
        selected: usize,
        rect: Rect,
    ) {
        let scale = self.scale_factor;
        push_rect(
            &mut self.overlay_rect_vertices,
            rect,
            [0.040, 0.047, 0.061, 1.0],
            self.config.width,
            self.config.height,
        );
        self.overlay_text.push(make_text(
            &mut self.font_system,
            "Commands  ·  ↑↓ choose  ·  Tab complete",
            Rect {
                x: rect.x + 10.0 * scale,
                y: rect.y + 5.0 * scale,
                width: rect.width - 20.0 * scale,
                height: 18.0 * scale,
            },
            9.5 * scale,
            Color::rgb(126, 143, 167),
            Family::SansSerif,
            Weight::MEDIUM,
        ));
        for (index, command) in commands.iter().enumerate() {
            let row = Rect {
                x: rect.x + 5.0 * scale,
                y: rect.y + (25.0 + index as f32 * 30.0) * scale,
                width: rect.width - 10.0 * scale,
                height: 27.0 * scale,
            };
            if index == selected {
                push_rect(
                    &mut self.overlay_rect_vertices,
                    row,
                    [0.080, 0.133, 0.198, 1.0],
                    self.config.width,
                    self.config.height,
                );
            }
            self.overlay_text.push(make_text(
                &mut self.font_system,
                &format!("/{}", command.name),
                Rect {
                    x: row.x + 8.0 * scale,
                    y: row.y + 4.0 * scale,
                    width: 122.0 * scale,
                    height: 19.0 * scale,
                },
                10.8 * scale,
                Color::rgb(189, 216, 244),
                Family::Monospace,
                Weight::MEDIUM,
            ));
            self.overlay_text.push(make_text(
                &mut self.font_system,
                &head_chars(&command.description, 48),
                Rect {
                    x: row.x + 132.0 * scale,
                    y: row.y + 4.0 * scale,
                    width: row.width - 140.0 * scale,
                    height: 19.0 * scale,
                },
                10.3 * scale,
                Color::rgb(150, 162, 181),
                Family::SansSerif,
                Weight::NORMAL,
            ));
        }
    }

    fn add_empty_agent_surface(&mut self, panel: Rect, loading: bool) {
        let scale = self.scale_factor;
        let label = if loading {
            "Looking for agent sessions…"
        } else {
            "Codex is ready when you are."
        };
        self.overlay_text.push(make_text(
            &mut self.font_system,
            label,
            Rect {
                x: panel.x + 22.0 * scale,
                y: panel.y + 88.0 * scale,
                width: panel.width - 44.0 * scale,
                height: 28.0 * scale,
            },
            14.0 * scale,
            Color::rgb(198, 205, 219),
            Family::SansSerif,
            Weight::MEDIUM,
        ));
        if !loading {
            self.overlay_text.push(make_text(
                &mut self.font_system,
                "Press Enter to start a persistent ACP session.",
                Rect {
                    x: panel.x + 22.0 * scale,
                    y: panel.y + 120.0 * scale,
                    width: panel.width - 44.0 * scale,
                    height: 24.0 * scale,
                },
                12.0 * scale,
                Color::rgb(142, 153, 174),
                Family::SansSerif,
                Weight::NORMAL,
            ));
        }
    }

    fn add_agent_launcher(
        &mut self,
        panel: Rect,
        launcher: AgentLauncherView<'_>,
        draft: &str,
        loading: bool,
        notice: Option<&str>,
    ) {
        let scale = self.scale_factor;
        let content_x = panel.x + 18.0 * scale;
        let content_width = panel.width - 36.0 * scale;
        self.overlay_text.push(make_text(
            &mut self.font_system,
            "New agent session",
            Rect {
                x: content_x,
                y: panel.y + 48.0 * scale,
                width: content_width,
                height: 24.0 * scale,
            },
            13.0 * scale,
            Color::rgb(220, 226, 237),
            Family::SansSerif,
            Weight::SEMIBOLD,
        ));
        self.overlay_text.push(make_text(
            &mut self.font_system,
            "↑↓ choose  ·  Enter start  ·  /agents sessions",
            Rect {
                x: content_x,
                y: panel.y + 72.0 * scale,
                width: content_width,
                height: 20.0 * scale,
            },
            10.5 * scale,
            Color::rgb(126, 138, 158),
            Family::SansSerif,
            Weight::NORMAL,
        ));

        self.add_agent_profile_rows(panel, launcher.profiles, launcher.selected, notice);
        self.add_agent_launcher_footer(panel, launcher.cwd_override, draft, loading);
    }

    fn add_agent_profile_rows(
        &mut self,
        panel: Rect,
        profiles: &[AgentProfile],
        selected: usize,
        notice: Option<&str>,
    ) {
        let scale = self.scale_factor;
        let content_x = panel.x + 18.0 * scale;
        let content_width = panel.width - 36.0 * scale;
        let row_height = 68.0 * scale;
        let mut y = if let Some(notice) = notice {
            self.add_agent_notice(panel, notice, panel.y + 94.0 * scale);
            panel.y + 134.0 * scale
        } else {
            panel.y + 104.0 * scale
        };
        for (index, profile) in profiles.iter().take(5).enumerate() {
            let row = Rect {
                x: content_x,
                y,
                width: content_width,
                height: 58.0 * scale,
            };
            if index == selected {
                push_rect(
                    &mut self.overlay_rect_vertices,
                    row,
                    [0.045, 0.075, 0.115, 1.0],
                    self.config.width,
                    self.config.height,
                );
                push_rect(
                    &mut self.overlay_rect_vertices,
                    Rect {
                        width: 2.0 * scale,
                        ..row
                    },
                    AGENT_ACCENT,
                    self.config.width,
                    self.config.height,
                );
            }
            self.overlay_text.push(make_text(
                &mut self.font_system,
                &profile.name,
                Rect {
                    x: row.x + 12.0 * scale,
                    y: row.y + 8.0 * scale,
                    width: row.width - 24.0 * scale,
                    height: 21.0 * scale,
                },
                12.0 * scale,
                Color::rgb(225, 230, 240),
                Family::SansSerif,
                Weight::MEDIUM,
            ));
            self.overlay_text.push(make_text(
                &mut self.font_system,
                &profile.description,
                Rect {
                    x: row.x + 12.0 * scale,
                    y: row.y + 31.0 * scale,
                    width: row.width - 24.0 * scale,
                    height: 18.0 * scale,
                },
                10.5 * scale,
                Color::rgb(135, 146, 166),
                Family::SansSerif,
                Weight::NORMAL,
            ));
            y += row_height;
        }
    }

    fn add_agent_launcher_footer(
        &mut self,
        panel: Rect,
        cwd_override: Option<&Path>,
        draft: &str,
        loading: bool,
    ) {
        let scale = self.scale_factor;
        let content_x = panel.x + 18.0 * scale;
        let content_width = panel.width - 36.0 * scale;
        let cwd = cwd_override.map_or_else(
            || "focused pane · live cwd".to_owned(),
            |path| path.display().to_string(),
        );
        let footer = Rect {
            x: content_x,
            y: panel.y + panel.height - 92.0 * scale,
            width: content_width,
            height: 72.0 * scale,
        };
        push_rect(
            &mut self.overlay_rect_vertices,
            footer,
            AGENT_COMPOSER,
            self.config.width,
            self.config.height,
        );
        self.overlay_text.push(make_text(
            &mut self.font_system,
            &format!("Working directory  ·  {cwd}"),
            Rect {
                x: footer.x + 12.0 * scale,
                y: footer.y + 9.0 * scale,
                width: footer.width - 24.0 * scale,
                height: 20.0 * scale,
            },
            10.5 * scale,
            Color::rgb(151, 164, 185),
            Family::SansSerif,
            Weight::MEDIUM,
        ));
        let input = if loading {
            "Starting persistent ACP session…"
        } else if draft.is_empty() {
            "Enter to start  ·  /cwd <path> to change"
        } else {
            draft
        };
        self.overlay_text.push(make_text(
            &mut self.font_system,
            input,
            Rect {
                x: footer.x + 12.0 * scale,
                y: footer.y + 38.0 * scale,
                width: footer.width - if loading { 24.0 } else { 100.0 } * scale,
                height: 21.0 * scale,
            },
            11.5 * scale,
            if draft.is_empty() {
                Color::rgb(124, 136, 156)
            } else {
                Color::rgb(226, 231, 240)
            },
            Family::SansSerif,
            Weight::NORMAL,
        ));
        if !loading {
            let button = Rect {
                x: footer.x + footer.width - 76.0 * scale,
                y: footer.y + 34.0 * scale,
                width: 64.0 * scale,
                height: 30.0 * scale,
            };
            push_rect(
                &mut self.overlay_rect_vertices,
                button,
                [0.105, 0.265, 0.420, 1.0],
                self.config.width,
                self.config.height,
            );
            self.overlay_text.push(make_text(
                &mut self.font_system,
                "Start",
                Rect {
                    x: button.x + 15.0 * scale,
                    y: button.y + 5.0 * scale,
                    width: button.width - 24.0 * scale,
                    height: 20.0 * scale,
                },
                11.0 * scale,
                Color::rgb(226, 237, 249),
                Family::SansSerif,
                Weight::SEMIBOLD,
            ));
        }
    }

    fn add_agent_notice(&mut self, panel: Rect, notice: &str, y: f32) {
        let scale = self.scale_factor;
        let rect = Rect {
            x: panel.x + 18.0 * scale,
            y,
            width: panel.width - 36.0 * scale,
            height: 30.0 * scale,
        };
        push_rect(
            &mut self.overlay_rect_vertices,
            rect,
            [0.037, 0.050, 0.069, 1.0],
            self.config.width,
            self.config.height,
        );
        push_rect(
            &mut self.overlay_rect_vertices,
            Rect {
                width: 2.0 * scale,
                ..rect
            },
            [0.28, 0.55, 0.82, 0.95],
            self.config.width,
            self.config.height,
        );
        self.overlay_text.push(make_text(
            &mut self.font_system,
            notice,
            Rect {
                x: rect.x + 10.0 * scale,
                y: rect.y + 6.0 * scale,
                width: rect.width - 18.0 * scale,
                height: 19.0 * scale,
            },
            10.5 * scale,
            Color::rgb(175, 193, 217),
            Family::SansSerif,
            Weight::MEDIUM,
        ));
    }

    fn add_agent_header(
        &mut self,
        agent: &AgentSessionSnapshot,
        panel: Rect,
        selected: usize,
        session_count: usize,
    ) {
        let scale = self.scale_factor;
        let (status, color) = match agent.status {
            AgentSessionStatus::Starting => ("connecting", Color::rgb(213, 176, 102)),
            AgentSessionStatus::WaitingForAuthentication => {
                ("sign in required", Color::rgb(245, 192, 102))
            }
            AgentSessionStatus::Authenticating => ("signing in", Color::rgb(107, 181, 245)),
            AgentSessionStatus::Idle => ("ready", Color::rgb(115, 207, 151)),
            AgentSessionStatus::Working => ("working", Color::rgb(107, 181, 245)),
            AgentSessionStatus::WaitingForPermission => {
                ("needs permission", Color::rgb(245, 192, 102))
            }
            AgentSessionStatus::Failed => ("attention", Color::rgb(245, 137, 137)),
            AgentSessionStatus::Closed => ("closed", Color::rgb(143, 151, 168)),
        };
        let title = agent.agent_name.as_deref().unwrap_or(&agent.name);
        let title = format!("{title}  ·  {} of {session_count}", selected + 1);
        self.overlay_text.push(make_text(
            &mut self.font_system,
            &title,
            Rect {
                x: panel.x + 18.0 * scale,
                y: panel.y + 43.0 * scale,
                width: panel.width - 150.0 * scale,
                height: 21.0 * scale,
            },
            12.5 * scale,
            Color::rgb(209, 216, 229),
            Family::SansSerif,
            Weight::MEDIUM,
        ));
        self.overlay_text.push(make_text(
            &mut self.font_system,
            status,
            Rect {
                x: panel.x + panel.width - 128.0 * scale,
                y: panel.y + 44.0 * scale,
                width: 110.0 * scale,
                height: 20.0 * scale,
            },
            10.5 * scale,
            color,
            Family::SansSerif,
            Weight::MEDIUM,
        ));
        let mut details = vec![tail_chars(&agent.cwd.display().to_string(), 44)];
        if let Some(mode) = &agent.current_mode {
            details.push(mode.clone());
        }
        for option in &agent.config_options {
            if matches!(
                option.category,
                AgentConfigCategory::Model | AgentConfigCategory::ThoughtLevel
            ) {
                let value = match &option.value {
                    AgentConfigValue::Select { current, .. } => current.clone(),
                    AgentConfigValue::Boolean(value) => {
                        if *value { "on" } else { "off" }.to_owned()
                    }
                };
                details.push(value);
            }
        }
        self.overlay_text.push(make_text(
            &mut self.font_system,
            &details
                .into_iter()
                .take(4)
                .collect::<Vec<_>>()
                .join("  ·  "),
            Rect {
                x: panel.x + 18.0 * scale,
                y: panel.y + 67.0 * scale,
                width: panel.width - 36.0 * scale,
                height: 18.0 * scale,
            },
            10.0 * scale,
            Color::rgb(124, 137, 158),
            Family::SansSerif,
            Weight::NORMAL,
        ));
    }

    fn add_agent_composer(
        &mut self,
        draft: &str,
        status: AgentSessionStatus,
        agent_name: &str,
        context_label: &str,
        rect: Rect,
    ) {
        let scale = self.scale_factor;
        push_rect(
            &mut self.overlay_rect_vertices,
            rect,
            AGENT_COMPOSER,
            self.config.width,
            self.config.height,
        );
        let (text, color) = if draft.is_empty() {
            let placeholder = match status {
                AgentSessionStatus::Working => "Agent is working…  Ctrl+C to stop",
                AgentSessionStatus::WaitingForAuthentication => {
                    "Run /login to sign in with this agent"
                }
                AgentSessionStatus::Authenticating => "Complete sign in to continue…",
                _ => "Ask the agent…",
            };
            (placeholder, Color::rgb(126, 137, 157))
        } else {
            (draft, Color::rgb(229, 233, 241))
        };
        self.overlay_text.push(make_text(
            &mut self.font_system,
            &format!("{agent_name}  ·  {context_label}  ·  /help"),
            Rect {
                x: rect.x + 12.0 * scale,
                y: rect.y + 7.0 * scale,
                width: rect.width - 24.0 * scale,
                height: 17.0 * scale,
            },
            9.5 * scale,
            Color::rgb(117, 132, 154),
            Family::SansSerif,
            Weight::MEDIUM,
        ));
        self.overlay_text.push(make_wrapped_text(
            &mut self.font_system,
            text,
            Rect {
                x: rect.x + 12.0 * scale,
                y: rect.y + 26.0 * scale,
                width: rect.width - 24.0 * scale,
                height: rect.height - 34.0 * scale,
            },
            12.5 * scale,
            18.0 * scale,
            color,
            Family::SansSerif,
            Weight::NORMAL,
        ));
        if !matches!(
            status,
            AgentSessionStatus::Starting
                | AgentSessionStatus::Authenticating
                | AgentSessionStatus::Closed
        ) {
            let button = Rect {
                x: rect.x + rect.width - 62.0 * scale,
                y: rect.y + 26.0 * scale,
                width: 50.0 * scale,
                height: 29.0 * scale,
            };
            push_rect(
                &mut self.overlay_rect_vertices,
                button,
                [0.075, 0.155, 0.245, 1.0],
                self.config.width,
                self.config.height,
            );
            self.overlay_text.push(make_text(
                &mut self.font_system,
                "Send",
                Rect {
                    x: button.x + 10.0 * scale,
                    y: button.y + 5.0 * scale,
                    width: button.width - 18.0 * scale,
                    height: 19.0 * scale,
                },
                10.5 * scale,
                Color::rgb(203, 222, 242),
                Family::SansSerif,
                Weight::SEMIBOLD,
            ));
        }
    }

    fn add_agent_permission(&mut self, permission: &mux_acp::AgentPermission, rect: Rect) {
        let scale = self.scale_factor;
        push_rect(
            &mut self.overlay_rect_vertices,
            rect,
            AGENT_PERMISSION,
            self.config.width,
            self.config.height,
        );
        self.overlay_text.push(make_text(
            &mut self.font_system,
            &format!("Permission  ·  {}", permission.title),
            Rect {
                x: rect.x + 12.0 * scale,
                y: rect.y + 8.0 * scale,
                width: rect.width - 24.0 * scale,
                height: 22.0 * scale,
            },
            12.0 * scale,
            Color::rgb(244, 215, 153),
            Family::SansSerif,
            Weight::SEMIBOLD,
        ));
        for (index, option) in permission.options.iter().take(4).enumerate() {
            self.overlay_text.push(make_text(
                &mut self.font_system,
                &format!("{}  {}", index + 1, option.label),
                Rect {
                    x: rect.x + 14.0 * scale,
                    y: rect.y + (36.0 + index as f32 * 26.0) * scale,
                    width: rect.width - 28.0 * scale,
                    height: 21.0 * scale,
                },
                11.5 * scale,
                Color::rgb(222, 225, 232),
                Family::SansSerif,
                Weight::MEDIUM,
            ));
        }
    }

    fn add_agent_timeline(
        &mut self,
        agent: &AgentSessionSnapshot,
        body: Rect,
        timeline_scroll: usize,
    ) {
        let scale = self.scale_factor;
        let approximate_characters = (body.width / (7.1 * scale)).max(24.0) as usize;
        let mut bottom = body.y + body.height;
        if timeline_scroll > 0 {
            self.overlay_text.push(make_text(
                &mut self.font_system,
                &format!("{timeline_scroll} newer  ·  End to return"),
                Rect {
                    x: body.x,
                    y: body.y,
                    width: body.width,
                    height: 18.0 * scale,
                },
                9.5 * scale,
                Color::rgb(112, 130, 155),
                Family::SansSerif,
                Weight::MEDIUM,
            ));
        }
        for item in agent.timeline.iter().rev().skip(timeline_scroll).take(24) {
            let (text, color, weight) = timeline_text(item);
            let text = tail_chars(&text, 1_600);
            let line_count = text
                .lines()
                .map(|line| line.chars().count().div_ceil(approximate_characters).max(1))
                .sum::<usize>()
                .clamp(1, 12);
            let height = (line_count as f32 * 18.0 + 10.0) * scale;
            let y = bottom - height;
            if y < body.y {
                break;
            }
            self.overlay_text.push(make_wrapped_text(
                &mut self.font_system,
                &text,
                Rect {
                    x: body.x,
                    y,
                    width: body.width,
                    height: height - 4.0 * scale,
                },
                11.8 * scale,
                18.0 * scale,
                color,
                Family::SansSerif,
                weight,
            ));
            bottom = y - 8.0 * scale;
        }
    }

    fn add_session_switcher(&mut self, switcher: SessionSwitcherView<'_>) {
        let scale = self.scale_factor;
        let window = Rect {
            x: 0.0,
            y: 0.0,
            width: self.config.width as f32,
            height: self.config.height as f32,
        };
        push_rect(
            &mut self.overlay_rect_vertices,
            window,
            OVERLAY_SCRIM,
            self.config.width,
            self.config.height,
        );
        let row_height = 36.0 * scale;
        let panel_width = (460.0 * scale).min(window.width - 40.0 * scale);
        let visible_rows = switcher.entries.len().clamp(1, 9) as f32;
        let panel_height = 82.0 * scale + row_height * visible_rows;
        let panel = Rect {
            x: ((window.width - panel_width) / 2.0).round(),
            y: ((window.height - panel_height) / 2.0).round(),
            width: panel_width,
            height: panel_height,
        };
        push_rect(
            &mut self.overlay_rect_vertices,
            panel,
            OVERLAY_BACKGROUND,
            self.config.width,
            self.config.height,
        );
        self.overlay_text.push(make_text(
            &mut self.font_system,
            "Sessions",
            Rect {
                x: panel.x + 16.0 * scale,
                y: panel.y + 11.0 * scale,
                width: panel.width - 32.0 * scale,
                height: 28.0 * scale,
            },
            15.0 * scale,
            Color::rgb(230, 234, 243),
            Family::SansSerif,
            Weight::SEMIBOLD,
        ));
        self.add_session_switcher_help(panel, switcher.pending_kill.is_some());
        if switcher.entries.is_empty() {
            self.overlay_text.push(make_text(
                &mut self.font_system,
                "Loading sessions…",
                Rect {
                    x: panel.x + 16.0 * scale,
                    y: panel.y + 51.0 * scale,
                    width: panel.width - 32.0 * scale,
                    height: 24.0 * scale,
                },
                13.0 * scale,
                Color::rgb(174, 183, 200),
                Family::SansSerif,
                Weight::NORMAL,
            ));
            return;
        }
        for (index, entry) in switcher.entries.iter().take(9).enumerate() {
            let row = Rect {
                x: panel.x + 8.0 * scale,
                y: panel.y + 48.0 * scale + index as f32 * row_height,
                width: panel.width - 16.0 * scale,
                height: row_height - 2.0 * scale,
            };
            if index == switcher.selected {
                push_rect(
                    &mut self.overlay_rect_vertices,
                    row,
                    OVERLAY_SELECTED,
                    self.config.width,
                    self.config.height,
                );
            }
            self.add_session_switcher_row(
                index,
                entry,
                row,
                switcher.pending_kill == Some(entry.id),
            );
        }
    }

    fn add_session_switcher_help(&mut self, panel: Rect, pending_kill: bool) {
        let scale = self.scale_factor;
        let help = if pending_kill {
            "Press x again to kill this session · Esc cancel"
        } else {
            "↑↓ choose · Enter attach · n new · r rename · x kill"
        };
        self.overlay_text.push(make_text(
            &mut self.font_system,
            help,
            Rect {
                x: panel.x + 16.0 * scale,
                y: panel.y + panel.height - 26.0 * scale,
                width: panel.width - 32.0 * scale,
                height: 18.0 * scale,
            },
            10.5 * scale,
            if pending_kill {
                Color::rgb(235, 164, 116)
            } else {
                Color::rgb(139, 151, 171)
            },
            Family::SansSerif,
            Weight::NORMAL,
        ));
    }

    fn add_session_switcher_row(
        &mut self,
        index: usize,
        entry: &SessionSummary,
        row: Rect,
        pending_kill: bool,
    ) {
        let scale = self.scale_factor;
        let label = format!("{}  {}", index + 1, entry.name);
        self.overlay_text.push(make_text(
            &mut self.font_system,
            &label,
            Rect {
                x: row.x + 10.0 * scale,
                y: row.y + 5.0 * scale,
                width: row.width - 100.0 * scale,
                height: row.height - 8.0 * scale,
            },
            13.0 * scale,
            if pending_kill {
                Color::rgb(246, 184, 137)
            } else {
                Color::rgb(224, 229, 239)
            },
            Family::SansSerif,
            Weight::MEDIUM,
        ));
        let pane_count = if entry.pane_count == 1 {
            "1 pane".to_owned()
        } else {
            format!("{} panes", entry.pane_count)
        };
        self.overlay_text.push(make_text(
            &mut self.font_system,
            &pane_count,
            Rect {
                x: row.x + row.width - 86.0 * scale,
                y: row.y + 6.0 * scale,
                width: 76.0 * scale,
                height: row.height - 8.0 * scale,
            },
            11.5 * scale,
            Color::rgb(159, 171, 192),
            Family::SansSerif,
            Weight::NORMAL,
        ));
    }

    fn add_text_prompt(&mut self, prompt: TextPromptView<'_>) {
        let scale = self.scale_factor;
        let window = Rect {
            x: 0.0,
            y: 0.0,
            width: self.config.width as f32,
            height: self.config.height as f32,
        };
        push_rect(
            &mut self.overlay_rect_vertices,
            window,
            OVERLAY_SCRIM,
            self.config.width,
            self.config.height,
        );
        let panel = self.text_prompt_panel();
        push_rect(
            &mut self.overlay_rect_vertices,
            panel,
            OVERLAY_BACKGROUND,
            self.config.width,
            self.config.height,
        );
        push_border(
            &mut self.overlay_rect_vertices,
            panel,
            scale.max(1.0),
            [0.18, 0.22, 0.29, 1.0],
            self.config.width,
            self.config.height,
        );
        self.overlay_text.push(make_text(
            &mut self.font_system,
            prompt.label,
            Rect {
                x: panel.x + 16.0 * scale,
                y: panel.y + 11.0 * scale,
                width: panel.width - 32.0 * scale,
                height: 22.0 * scale,
            },
            13.0 * scale,
            Color::rgb(225, 230, 240),
            Family::SansSerif,
            Weight::SEMIBOLD,
        ));
        let input = self.text_prompt_input_rect();
        push_rect(
            &mut self.overlay_rect_vertices,
            input,
            [0.025, 0.030, 0.040, 1.0],
            self.config.width,
            self.config.height,
        );
        push_border(
            &mut self.overlay_rect_vertices,
            input,
            scale.max(1.0),
            [0.22, 0.34, 0.50, 1.0],
            self.config.width,
            self.config.height,
        );
        let (value, color) = if prompt.draft.is_empty() {
            ("Type a name…", Color::rgb(116, 126, 144))
        } else {
            (prompt.draft, Color::rgb(226, 231, 240))
        };
        self.overlay_text.push(make_text(
            &mut self.font_system,
            value,
            Rect {
                x: input.x + 10.0 * scale,
                y: input.y + 6.0 * scale,
                width: input.width - 20.0 * scale,
                height: input.height - 8.0 * scale,
            },
            13.0 * scale,
            color,
            Family::Name(&self.terminal_font.family),
            Weight::NORMAL,
        ));
        let cursor = self.text_prompt_cursor_rect(prompt);
        push_rect(
            &mut self.overlay_rect_vertices,
            cursor,
            AGENT_ACCENT,
            self.config.width,
            self.config.height,
        );
        self.add_text_prompt_help(panel);
    }

    fn add_text_prompt_help(&mut self, panel: Rect) {
        let scale = self.scale_factor;
        self.overlay_text.push(make_text(
            &mut self.font_system,
            "Enter rename  ·  Esc cancel",
            Rect {
                x: panel.x + 16.0 * scale,
                y: panel.y + panel.height - 25.0 * scale,
                width: panel.width - 32.0 * scale,
                height: 18.0 * scale,
            },
            10.5 * scale,
            Color::rgb(133, 144, 164),
            Family::SansSerif,
            Weight::NORMAL,
        ));
    }

    fn text_prompt_panel(&self) -> Rect {
        let scale = self.scale_factor;
        let width = (430.0 * scale).min(self.config.width as f32 - 32.0 * scale);
        let height = 118.0 * scale;
        Rect {
            x: ((self.config.width as f32 - width) / 2.0).round(),
            y: ((self.config.height as f32 - height) * 0.34)
                .max(16.0 * scale)
                .round(),
            width,
            height,
        }
    }

    fn text_prompt_input_rect(&self) -> Rect {
        let scale = self.scale_factor;
        let panel = self.text_prompt_panel();
        Rect {
            x: panel.x + 16.0 * scale,
            y: panel.y + 39.0 * scale,
            width: panel.width - 32.0 * scale,
            height: 36.0 * scale,
        }
    }

    fn text_prompt_cursor_rect(&self, prompt: TextPromptView<'_>) -> Rect {
        let scale = self.scale_factor;
        let input = self.text_prompt_input_rect();
        let offset = prompt.draft.width() as f32 * 7.8 * scale;
        Rect {
            x: (input.x + 9.0 * scale + offset)
                .min(input.x + input.width - 11.0 * scale)
                .round(),
            y: input.y + 8.0 * scale,
            width: scale.max(1.0),
            height: 19.0 * scale,
        }
    }

    #[allow(clippy::too_many_lines)]
    fn add_terminal_rectangles(
        &mut self,
        geometry: PaneGeometry,
        frame: &RenderFrame,
        hovered_hyperlink: Option<&str>,
        cursor_blink_visible: bool,
    ) {
        let cell_width = self.cell_width();
        let cell_height = self.cell_height();
        let content_x = geometry.rect.x + PANE_PADDING_X * self.scale_factor;
        let content_y = geometry.rect.y + PANE_PADDING_Y * self.scale_factor;
        push_rect(
            &mut self.rect_vertices,
            geometry.rect,
            rgb(frame.background, 1.0),
            self.config.width,
            self.config.height,
        );
        for (index, cell) in frame.cells.iter().enumerate() {
            let x = index % usize::from(frame.cols);
            let y = index / usize::from(frame.cols);
            let rect = Rect {
                x: content_x + x as f32 * cell_width,
                y: content_y + y as f32 * cell_height,
                width: cell_width + 0.5,
                height: cell_height + 0.5,
            };
            if cell.background != frame.background {
                push_rect(
                    &mut self.rect_vertices,
                    rect,
                    rgb(cell.background, 1.0),
                    self.config.width,
                    self.config.height,
                );
            }
            if cell.selected {
                push_rect(
                    &mut self.rect_vertices,
                    rect,
                    SELECTION,
                    self.config.width,
                    self.config.height,
                );
            }
            if !matches!(cell.width, CellWidth::SpacerTail | CellWidth::SpacerHead) {
                let decoration_rect = Rect {
                    width: if cell.width == CellWidth::Wide {
                        2.0 * cell_width
                    } else {
                        cell_width
                    },
                    ..rect
                };
                push_cell_decorations(
                    &mut self.rect_vertices,
                    decoration_rect,
                    cell,
                    hovered_hyperlink.is_some_and(|uri| cell.hyperlink.as_deref() == Some(uri)),
                    self.scale_factor,
                    self.config.width,
                    self.config.height,
                );
            }
        }
        push_scroll_indicator(
            &mut self.rect_vertices,
            geometry.rect,
            frame.scroll,
            self.scale_factor,
            self.config.width,
            self.config.height,
        );
        if geometry.focused {
            if let Some(cursor) = frame
                .cursor
                .filter(|cursor| cursor.visible && (!cursor.blinking || cursor_blink_visible))
            {
                let width = match cursor.style {
                    mux_terminal::CursorStyle::Bar => 2.0 * self.scale_factor,
                    _ => cell_width,
                };
                let height = match cursor.style {
                    mux_terminal::CursorStyle::Underline => 2.0 * self.scale_factor,
                    _ => cell_height,
                };
                let y_offset = if matches!(cursor.style, mux_terminal::CursorStyle::Underline) {
                    cell_height - height
                } else {
                    0.0
                };
                push_rect(
                    &mut self.rect_vertices,
                    Rect {
                        x: content_x + f32::from(cursor.x) * cell_width,
                        y: content_y + f32::from(cursor.y) * cell_height + y_offset,
                        width,
                        height,
                    },
                    rgb(cursor.color, 0.72),
                    self.config.width,
                    self.config.height,
                );
            }
            push_border(
                &mut self.rect_vertices,
                geometry.rect,
                self.scale_factor.max(1.0),
                FOCUS,
                self.config.width,
                self.config.height,
            );
        } else {
            push_border(
                &mut self.rect_vertices,
                geometry.rect,
                self.scale_factor.max(1.0),
                BORDER,
                self.config.width,
                self.config.height,
            );
        }
    }

    #[allow(clippy::too_many_lines)]
    pub fn draw(&mut self) -> Result<()> {
        self.viewport.update(
            &self.queue,
            Resolution {
                width: self.config.width,
                height: self.config.height,
            },
        );
        let scale_factor = self.scale_factor;
        let cell_width = self.cell_width();
        let cell_height = self.cell_height();
        let viewport_width = self.config.width;
        let viewport_height = self.config.height;
        let pane_areas = self.pane_text.values().flat_map(|text| {
            text.rows
                .iter()
                .enumerate()
                .flat_map(move |(row_index, row)| {
                    row.runs.iter().map(move |run| TextArea {
                        buffer: &run.buffer,
                        left: text.geometry.rect.x
                            + PANE_PADDING_X * scale_factor
                            + f32::from(run.column) * cell_width
                            + run.offset_x,
                        top: text.geometry.rect.y
                            + PANE_PADDING_Y * scale_factor
                            + row_index as f32 * cell_height,
                        scale: 1.0,
                        bounds: text_bounds(
                            terminal_content_rect(text.geometry.rect, scale_factor),
                            viewport_width,
                            viewport_height,
                        ),
                        default_color: Color::rgb(220, 224, 232),
                        custom_glyphs: &[],
                    })
                })
        });
        let chrome_areas = self.chrome_text.iter().map(|text| TextArea {
            buffer: &text.buffer,
            left: text.rect.x,
            top: text.rect.y,
            scale: 1.0,
            bounds: text_bounds(text.rect, self.config.width, self.config.height),
            default_color: text.color,
            custom_glyphs: &[],
        });
        self.text_pipeline
            .prepare(
                &self.device,
                &self.queue,
                &mut self.font_system,
                &mut self.atlas,
                &self.viewport,
                pane_areas.chain(chrome_areas),
                &mut self.swash_cache,
            )
            .context("prepare terminal text")?;
        let overlay_areas = self.overlay_text.iter().map(|text| TextArea {
            buffer: &text.buffer,
            left: text.rect.x,
            top: text.rect.y,
            scale: 1.0,
            bounds: text_bounds(text.rect, self.config.width, self.config.height),
            default_color: text.color,
            custom_glyphs: &[],
        });
        self.overlay_text_pipeline
            .prepare(
                &self.device,
                &self.queue,
                &mut self.font_system,
                &mut self.atlas,
                &self.viewport,
                overlay_areas,
                &mut self.swash_cache,
            )
            .context("prepare overlay text")?;

        let output = match self.surface.get_current_texture() {
            Ok(output) => output,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
            Err(wgpu::SurfaceError::Timeout) => return Ok(()),
            Err(error) => return Err(error).context("acquire GPU frame"),
        };
        let view = output
            .texture
            .create_view(&TextureViewDescriptor::default());
        self.upload_rect_vertices();
        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some("mux frame encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("mux terminal pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: Operations {
                        load: LoadOp::Clear(wgpu::Color {
                            r: f64::from(WINDOW_BACKGROUND[0]),
                            g: f64::from(WINDOW_BACKGROUND[1]),
                            b: f64::from(WINDOW_BACKGROUND[2]),
                            a: 1.0,
                        }),
                        store: StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            if let Some(vertex_buffer) = &self.rect_buffer {
                pass.set_pipeline(&self.rect_pipeline);
                pass.set_vertex_buffer(0, vertex_buffer.slice(..));
                pass.draw(
                    0..u32::try_from(self.rect_vertices.len()).unwrap_or(u32::MAX),
                    0..1,
                );
            }
            self.text_pipeline
                .render(&self.atlas, &self.viewport, &mut pass)
                .context("render terminal text")?;
            if let Some(vertex_buffer) = &self.overlay_rect_buffer {
                pass.set_pipeline(&self.rect_pipeline);
                pass.set_vertex_buffer(0, vertex_buffer.slice(..));
                pass.draw(
                    0..u32::try_from(self.overlay_rect_vertices.len()).unwrap_or(u32::MAX),
                    0..1,
                );
            }
            self.overlay_text_pipeline
                .render(&self.atlas, &self.viewport, &mut pass)
                .context("render overlay text")?;
        }
        self.queue.submit(Some(encoder.finish()));
        output.present();
        self.atlas.trim();
        Ok(())
    }

    fn upload_rect_vertices(&mut self) {
        upload_vertices(
            &self.device,
            &self.queue,
            &self.rect_vertices,
            &mut self.rect_buffer,
            &mut self.rect_buffer_capacity,
            "mux rectangle vertices",
        );
        upload_vertices(
            &self.device,
            &self.queue,
            &self.overlay_rect_vertices,
            &mut self.overlay_rect_buffer,
            &mut self.overlay_rect_buffer_capacity,
            "mux overlay rectangle vertices",
        );
    }

    pub fn request_redraw(&self) {
        self.window.request_redraw();
    }

    pub fn set_cursor_icon(&self, icon: CursorIcon) {
        self.window.set_cursor(icon);
    }

    pub fn drag_window(&self) -> Result<()> {
        self.window
            .drag_window()
            .context("start native window drag")
    }

    #[must_use]
    pub const fn width(&self) -> u32 {
        self.config.width
    }

    #[must_use]
    pub const fn height(&self) -> u32 {
        self.config.height
    }

    #[must_use]
    pub fn window_scale_factor(&self) -> f32 {
        self.scale_factor
    }

    #[must_use]
    pub fn terminal_cell_height(&self) -> f32 {
        self.cell_height()
    }

    fn font_size(&self) -> f32 {
        self.terminal_font.size * self.scale_factor
    }

    fn cell_width(&self) -> f32 {
        self.terminal_font.cell_width * self.scale_factor
    }

    fn cell_height(&self) -> f32 {
        self.terminal_font.cell_height * self.scale_factor
    }
}

fn load_terminal_fonts(font_system: &mut FontSystem) {
    for font in [
        include_bytes!("../assets/fonts/JetBrainsMonoNerdFontMono-Regular.ttf").as_slice(),
        include_bytes!("../assets/fonts/JetBrainsMonoNerdFontMono-Bold.ttf").as_slice(),
        include_bytes!("../assets/fonts/JetBrainsMonoNerdFontMono-Italic.ttf").as_slice(),
        include_bytes!("../assets/fonts/JetBrainsMonoNerdFontMono-BoldItalic.ttf").as_slice(),
    ] {
        font_system.db_mut().load_font_data(font.to_vec());
    }
}

fn resolve_terminal_font_family(font_system: &FontSystem, requested: Option<&str>) -> String {
    requested
        .filter(|requested| !requested.trim().is_empty())
        .and_then(|requested| {
            font_system.db().faces().find_map(|face| {
                face.families
                    .iter()
                    .find(|(family, _)| family.eq_ignore_ascii_case(requested.trim()))
                    .map(|(family, _)| family.clone())
            })
        })
        .unwrap_or_else(|| TERMINAL_FONT_FAMILY.to_owned())
}

fn measure_terminal_cell_width(
    font_system: &mut FontSystem,
    family: &str,
    font_size: f32,
    cell_height: f32,
) -> Option<f32> {
    const SAMPLE: &str = "0000000000";
    let mut buffer = Buffer::new(font_system, Metrics::new(font_size, cell_height));
    buffer.set_size(font_system, Some(1_000.0), Some(cell_height));
    buffer.set_wrap(font_system, Wrap::None);
    buffer.set_text(
        font_system,
        SAMPLE,
        &Attrs::new().family(Family::Name(family)),
        Shaping::Advanced,
    );
    buffer.shape_until_scroll(font_system, false);
    let width = buffer.layout_runs().next()?.line_w / SAMPLE.len() as f32;
    width
        .is_finite()
        .then_some(width)
        .filter(|width| *width > 0.0)
}

fn resize_row_buffers(
    _font_system: &mut FontSystem,
    rows: &mut Vec<TerminalRow>,
    target: usize,
    _font_size: f32,
    _cell_height: f32,
) {
    rows.truncate(target);
    while rows.len() < target {
        rows.push(TerminalRow::default());
    }
}

fn update_terminal_rows(
    font_system: &mut FontSystem,
    rows: &mut [TerminalRow],
    frame: &RenderFrame,
    font_family: &str,
    metrics: TerminalGridMetrics,
    rebuild_all: bool,
) {
    let cols = usize::from(frame.cols);
    for (row_index, row) in rows.iter_mut().enumerate() {
        let row_dirty = rebuild_all
            || frame
                .row_metadata
                .get(row_index)
                .is_none_or(|metadata| metadata.dirty);
        if !row_dirty {
            continue;
        }
        let start = row_index * cols;
        let end = (start + cols).min(frame.cells.len());
        update_terminal_row(
            font_system,
            row,
            &frame.cells[start..end],
            frame.foreground,
            font_family,
            metrics,
        );
    }
}

fn update_terminal_row(
    font_system: &mut FontSystem,
    row: &mut TerminalRow,
    cells: &[mux_terminal::RenderCell],
    default_foreground: Rgb,
    font_family: &str,
    metrics: TerminalGridMetrics,
) {
    let specs = terminal_run_specs(cells);
    let mut reusable = std::mem::take(&mut row.runs).into_iter();
    row.runs = specs
        .into_iter()
        .map(|spec| {
            let mut buffer = reusable.next().map_or_else(
                || {
                    Buffer::new(
                        font_system,
                        Metrics::new(metrics.font_size, metrics.cell_height),
                    )
                },
                |run| run.buffer,
            );
            let offset_x = shape_terminal_run(
                font_system,
                &mut buffer,
                &spec,
                default_foreground,
                font_family,
                metrics,
            );
            TerminalTextRun {
                buffer,
                column: spec.column,
                offset_x,
            }
        })
        .collect();
}

struct TerminalRunSpec {
    column: u16,
    cell_count: u16,
    center: bool,
    spans: Vec<TerminalSpan>,
}

struct TerminalSpan {
    text: String,
    color: Rgb,
    style: CellStyle,
}

fn terminal_run_specs(cells: &[mux_terminal::RenderCell]) -> Vec<TerminalRunSpec> {
    let mut specs = Vec::new();
    let mut narrow_start = 0_usize;
    let mut narrow_cells = Vec::new();
    for (column, cell) in cells.iter().enumerate() {
        match cell.width {
            CellWidth::Wide => {
                push_narrow_run(&mut specs, narrow_start, &narrow_cells);
                narrow_cells.clear();
                if !cell.style.invisible && !cell.grapheme.is_empty() {
                    specs.push(TerminalRunSpec {
                        column: u16::try_from(column).unwrap_or(u16::MAX),
                        cell_count: 2,
                        center: true,
                        spans: vec![TerminalSpan {
                            text: cell.grapheme.clone(),
                            color: cell.foreground,
                            style: cell.style,
                        }],
                    });
                }
                narrow_start = column.saturating_add(2);
            }
            CellWidth::SpacerTail | CellWidth::SpacerHead => {
                push_narrow_run(&mut specs, narrow_start, &narrow_cells);
                narrow_cells.clear();
                narrow_start = column.saturating_add(1);
            }
            CellWidth::Narrow => {
                if narrow_cells.is_empty() {
                    narrow_start = column;
                }
                narrow_cells.push(cell);
            }
        }
    }
    push_narrow_run(&mut specs, narrow_start, &narrow_cells);
    specs
}

fn push_narrow_run(
    specs: &mut Vec<TerminalRunSpec>,
    start_column: usize,
    cells: &[&mux_terminal::RenderCell],
) {
    let Some(first) = cells.iter().position(|cell| cell_has_text(cell)) else {
        return;
    };
    let last = cells
        .iter()
        .rposition(|cell| cell_has_text(cell))
        .expect("first text cell implies a last text cell");
    let mut spans: Vec<TerminalSpan> = Vec::new();
    for cell in &cells[first..=last] {
        let text = if cell.style.invisible || cell.grapheme.is_empty() {
            " "
        } else {
            &cell.grapheme
        };
        if let Some(span) = spans
            .last_mut()
            .filter(|span| span.color == cell.foreground && span.style == cell.style)
        {
            span.text.push_str(text);
        } else {
            spans.push(TerminalSpan {
                text: text.to_owned(),
                color: cell.foreground,
                style: cell.style,
            });
        }
    }
    specs.push(TerminalRunSpec {
        column: u16::try_from(start_column + first).unwrap_or(u16::MAX),
        cell_count: u16::try_from(last - first + 1).unwrap_or(u16::MAX),
        center: false,
        spans,
    });
}

fn cell_has_text(cell: &mux_terminal::RenderCell) -> bool {
    !cell.style.invisible && !cell.grapheme.is_empty() && cell.grapheme != " "
}

fn shape_terminal_run(
    font_system: &mut FontSystem,
    buffer: &mut Buffer,
    spec: &TerminalRunSpec,
    default_foreground: Rgb,
    font_family: &str,
    metrics: TerminalGridMetrics,
) -> f32 {
    let width = f32::from(spec.cell_count) * metrics.cell_width;
    let default = Attrs::new()
        .family(Family::Name(font_family))
        .color(to_text_color(default_foreground, false));
    buffer.set_metrics_and_size(
        font_system,
        Metrics::new(metrics.font_size, metrics.cell_height),
        Some(width.max(metrics.cell_width)),
        Some(metrics.cell_height),
    );
    buffer.set_monospace_width(font_system, Some(metrics.cell_width));
    buffer.set_wrap(font_system, Wrap::None);
    buffer.set_rich_text(
        font_system,
        spec.spans.iter().map(|span| {
            let mut attrs = Attrs::new()
                .family(Family::Name(font_family))
                .color(to_text_color(span.color, span.style.faint));
            if span.style.bold {
                attrs = attrs.weight(Weight::BOLD);
            }
            if span.style.italic {
                attrs = attrs.style(Style::Italic);
            }
            (span.text.as_str(), attrs)
        }),
        &default,
        Shaping::Advanced,
        None,
    );
    buffer.shape_until_scroll(font_system, false);
    if spec.center {
        let shaped_width = buffer.layout_runs().next().map_or(0.0, |run| run.line_w);
        ((width - shaped_width) * 0.5).max(0.0)
    } else {
        0.0
    }
}

fn make_text(
    font_system: &mut FontSystem,
    text: &str,
    rect: Rect,
    font_size: f32,
    color: Color,
    family: Family<'_>,
    weight: Weight,
) -> ChromeText {
    let mut buffer = Buffer::new(
        font_system,
        Metrics::new(font_size, rect.height.max(font_size)),
    );
    buffer.set_size(
        font_system,
        Some(rect.width.max(1.0)),
        Some(rect.height.max(1.0)),
    );
    buffer.set_wrap(font_system, Wrap::None);
    buffer.set_text(
        font_system,
        text,
        &Attrs::new().family(family).weight(weight).color(color),
        Shaping::Advanced,
    );
    ChromeText {
        buffer,
        rect,
        color,
    }
}

#[allow(clippy::too_many_arguments)]
fn make_wrapped_text(
    font_system: &mut FontSystem,
    text: &str,
    rect: Rect,
    font_size: f32,
    line_height: f32,
    color: Color,
    family: Family<'_>,
    weight: Weight,
) -> ChromeText {
    let mut buffer = Buffer::new(font_system, Metrics::new(font_size, line_height));
    buffer.set_size(
        font_system,
        Some(rect.width.max(1.0)),
        Some(rect.height.max(1.0)),
    );
    buffer.set_wrap(font_system, Wrap::WordOrGlyph);
    buffer.set_text(
        font_system,
        text,
        &Attrs::new().family(family).weight(weight).color(color),
        Shaping::Advanced,
    );
    ChromeText {
        buffer,
        rect,
        color,
    }
}

fn timeline_text(item: &AgentTimelineItem) -> (String, Color, Weight) {
    match item {
        AgentTimelineItem::Message { role, text, .. } => match role {
            AgentMessageRole::User => (
                format!("You\n{text}"),
                Color::rgb(220, 225, 235),
                Weight::MEDIUM,
            ),
            AgentMessageRole::Agent => (
                format!("Codex\n{text}"),
                Color::rgb(205, 217, 235),
                Weight::NORMAL,
            ),
            AgentMessageRole::Thought => (
                format!("Thinking\n{text}"),
                Color::rgb(136, 148, 169),
                Weight::NORMAL,
            ),
        },
        AgentTimelineItem::Tool(tool) => {
            let marker = match tool.status {
                ToolStatus::Pending => "○",
                ToolStatus::Running => "◌",
                ToolStatus::Completed => "✓",
                ToolStatus::Failed => "×",
            };
            let detail = tool
                .detail
                .as_deref()
                .map_or_else(String::new, |detail| format!("\n{detail}"));
            (
                format!("{marker}  {}{detail}", tool.title),
                Color::rgb(164, 177, 199),
                Weight::MEDIUM,
            )
        }
        AgentTimelineItem::Plan(entries) => {
            let lines = entries
                .iter()
                .map(|entry| {
                    let marker = match entry.status {
                        PlanStatus::Pending => "○",
                        PlanStatus::Running => "◌",
                        PlanStatus::Completed => "✓",
                    };
                    format!("{marker}  {}", entry.text)
                })
                .collect::<Vec<_>>()
                .join("\n");
            (
                format!("Plan\n{lines}"),
                Color::rgb(154, 170, 197),
                Weight::NORMAL,
            )
        }
        AgentTimelineItem::Permission(permission) => (
            format!("Permission requested\n{}", permission.title),
            Color::rgb(232, 194, 119),
            Weight::MEDIUM,
        ),
        AgentTimelineItem::Context { label, characters } => (
            format!("Context attached  ·  {label}  ·  {characters} chars"),
            Color::rgb(124, 162, 191),
            Weight::NORMAL,
        ),
        AgentTimelineItem::Error(error) => (
            format!("Agent error\n{error}"),
            Color::rgb(242, 143, 143),
            Weight::NORMAL,
        ),
    }
}

fn tail_chars(text: &str, maximum: usize) -> String {
    let count = text.chars().count();
    if count <= maximum {
        return text.to_owned();
    }
    format!(
        "…{}",
        text.chars().skip(count - maximum).collect::<String>()
    )
}

fn head_chars(text: &str, maximum: usize) -> String {
    if text.chars().count() <= maximum {
        return text.to_owned();
    }
    let visible = maximum.saturating_sub(1);
    format!("{}…", text.chars().take(visible).collect::<String>())
}

fn create_rect_pipeline(device: &wgpu::Device, format: TextureFormat) -> RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("mux rectangle shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("rect.wgsl").into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("mux rectangle pipeline layout"),
        bind_group_layouts: &[],
        push_constant_ranges: &[],
    });
    device.create_render_pipeline(&RenderPipelineDescriptor {
        label: Some("mux rectangle pipeline"),
        layout: Some(&layout),
        vertex: VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4],
            }],
            compilation_options: PipelineCompilationOptions::default(),
        },
        fragment: Some(FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(ColorTargetState {
                format,
                blend: Some(BlendState::ALPHA_BLENDING),
                write_mask: ColorWrites::ALL,
            })],
            compilation_options: PipelineCompilationOptions::default(),
        }),
        primitive: PrimitiveState::default(),
        depth_stencil: None,
        multisample: MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}

fn push_border(
    vertices: &mut Vec<Vertex>,
    rect: Rect,
    width: f32,
    color: [f32; 4],
    viewport_width: u32,
    viewport_height: u32,
) {
    for edge in [
        Rect {
            height: width,
            ..rect
        },
        Rect {
            y: rect.y + rect.height - width,
            height: width,
            ..rect
        },
        Rect { width, ..rect },
        Rect {
            x: rect.x + rect.width - width,
            width,
            ..rect
        },
    ] {
        push_rect(vertices, edge, color, viewport_width, viewport_height);
    }
}

fn push_scroll_indicator(
    vertices: &mut Vec<Vertex>,
    pane: Rect,
    scroll: mux_terminal::TerminalScrollState,
    scale_factor: f32,
    viewport_width: u32,
    viewport_height: u32,
) {
    if !scroll.is_scrolled() || scroll.total <= scroll.len {
        return;
    }
    let inset = 4.0 * scale_factor;
    let track_height = (pane.height - 2.0 * inset).max(1.0);
    let thumb_height = (track_height * scroll.len as f32 / scroll.total as f32)
        .max(24.0 * scale_factor)
        .min(track_height);
    let max_offset = scroll.total.saturating_sub(scroll.len).max(1);
    let progress = (scroll.offset as f32 / max_offset as f32).clamp(0.0, 1.0);
    push_rect(
        vertices,
        Rect {
            x: pane.x + pane.width - 4.0 * scale_factor,
            y: pane.y + inset + (track_height - thumb_height) * progress,
            width: 2.0 * scale_factor,
            height: thumb_height,
        },
        SCROLL_THUMB,
        viewport_width,
        viewport_height,
    );
}

fn push_cell_decorations(
    vertices: &mut Vec<Vertex>,
    cell: Rect,
    render_cell: &RenderCell,
    hyperlink_highlight: bool,
    scale_factor: f32,
    viewport_width: u32,
    viewport_height: u32,
) {
    let stroke = scale_factor.max(1.0);
    let foreground = rgb(
        render_cell.foreground,
        if render_cell.style.faint { 0.55 } else { 1.0 },
    );
    if render_cell.style.overline {
        push_horizontal_cell_line(
            vertices,
            cell,
            cell.y,
            foreground,
            stroke,
            viewport_width,
            viewport_height,
        );
    }
    if render_cell.style.strikethrough {
        push_horizontal_cell_line(
            vertices,
            cell,
            cell.y + (cell.height * 0.56).round(),
            foreground,
            stroke,
            viewport_width,
            viewport_height,
        );
    }
    let underline = rgb(
        render_cell.underline_color,
        if render_cell.style.faint { 0.55 } else { 1.0 },
    );
    let bottom = cell.y + cell.height - stroke;
    match render_cell.style.underline {
        1 => push_horizontal_cell_line(
            vertices,
            cell,
            bottom,
            underline,
            stroke,
            viewport_width,
            viewport_height,
        ),
        2 => {
            for y in [bottom - 3.0 * stroke, bottom] {
                push_horizontal_cell_line(
                    vertices,
                    cell,
                    y,
                    underline,
                    stroke,
                    viewport_width,
                    viewport_height,
                );
            }
        }
        3 => push_patterned_underline(
            vertices,
            cell,
            underline,
            stroke,
            UnderlinePattern::Curly,
            viewport_width,
            viewport_height,
        ),
        4 => push_patterned_underline(
            vertices,
            cell,
            underline,
            stroke,
            UnderlinePattern::Dotted,
            viewport_width,
            viewport_height,
        ),
        5 => push_patterned_underline(
            vertices,
            cell,
            underline,
            stroke,
            UnderlinePattern::Dashed,
            viewport_width,
            viewport_height,
        ),
        _ => {}
    }
    if hyperlink_highlight && render_cell.style.underline == 0 {
        push_hyperlink_underline(
            vertices,
            cell,
            foreground,
            stroke,
            viewport_width,
            viewport_height,
        );
    }
}

fn push_hyperlink_underline(
    vertices: &mut Vec<Vertex>,
    cell: Rect,
    color: [f32; 4],
    stroke: f32,
    viewport_width: u32,
    viewport_height: u32,
) {
    push_horizontal_cell_line(
        vertices,
        cell,
        cell.y + cell.height - stroke,
        color,
        stroke,
        viewport_width,
        viewport_height,
    );
}

fn push_horizontal_cell_line(
    vertices: &mut Vec<Vertex>,
    cell: Rect,
    y: f32,
    color: [f32; 4],
    stroke: f32,
    viewport_width: u32,
    viewport_height: u32,
) {
    push_rect(
        vertices,
        Rect {
            y,
            height: stroke,
            ..cell
        },
        color,
        viewport_width,
        viewport_height,
    );
}

#[derive(Clone, Copy)]
enum UnderlinePattern {
    Curly,
    Dotted,
    Dashed,
}

fn push_patterned_underline(
    vertices: &mut Vec<Vertex>,
    cell: Rect,
    color: [f32; 4],
    stroke: f32,
    pattern: UnderlinePattern,
    viewport_width: u32,
    viewport_height: u32,
) {
    let (segments, duty_cycle) = match pattern {
        UnderlinePattern::Curly => (4_u8, 1.0),
        UnderlinePattern::Dotted => (4, 0.34),
        UnderlinePattern::Dashed => (2, 0.68),
    };
    let segment_width = cell.width / f32::from(segments);
    for index in 0..segments {
        let wave_offset = if matches!(pattern, UnderlinePattern::Curly) && index % 2 == 0 {
            -2.0 * stroke
        } else {
            0.0
        };
        push_rect(
            vertices,
            Rect {
                x: cell.x + f32::from(index) * segment_width,
                y: cell.y + cell.height - stroke + wave_offset,
                width: (segment_width * duty_cycle).max(stroke),
                height: stroke,
            },
            color,
            viewport_width,
            viewport_height,
        );
    }
}

fn push_rect(
    vertices: &mut Vec<Vertex>,
    rect: Rect,
    color: [f32; 4],
    viewport_width: u32,
    viewport_height: u32,
) {
    if rect.width <= 0.0 || rect.height <= 0.0 {
        return;
    }
    let left = rect.x / viewport_width as f32 * 2.0 - 1.0;
    let right = (rect.x + rect.width) / viewport_width as f32 * 2.0 - 1.0;
    let top = 1.0 - rect.y / viewport_height as f32 * 2.0;
    let bottom = 1.0 - (rect.y + rect.height) / viewport_height as f32 * 2.0;
    vertices.extend_from_slice(&[
        Vertex {
            position: [left, top],
            color,
        },
        Vertex {
            position: [left, bottom],
            color,
        },
        Vertex {
            position: [right, bottom],
            color,
        },
        Vertex {
            position: [left, top],
            color,
        },
        Vertex {
            position: [right, bottom],
            color,
        },
        Vertex {
            position: [right, top],
            color,
        },
    ]);
}

fn upload_vertices(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    vertices: &[Vertex],
    buffer: &mut Option<wgpu::Buffer>,
    capacity: &mut u64,
    label: &'static str,
) {
    if vertices.is_empty() {
        return;
    }
    let bytes = bytemuck::cast_slice(vertices);
    let required = bytes.len() as u64;
    if required > *capacity {
        let next_capacity = required.next_power_of_two().max(4_096);
        *buffer = Some(device.create_buffer(&BufferDescriptor {
            label: Some(label),
            size: next_capacity,
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
        *capacity = next_capacity;
    }
    if let Some(buffer) = buffer {
        queue.write_buffer(buffer, 0, bytes);
    }
}

fn text_bounds(rect: Rect, width: u32, height: u32) -> TextBounds {
    TextBounds {
        left: rect.x.max(0.0) as i32,
        top: rect.y.max(0.0) as i32,
        right: (rect.x + rect.width).min(width as f32) as i32,
        bottom: (rect.y + rect.height).min(height as f32) as i32,
    }
}

fn terminal_content_rect(rect: Rect, scale_factor: f32) -> Rect {
    let horizontal_padding = PANE_PADDING_X * scale_factor;
    let vertical_padding = PANE_PADDING_Y * scale_factor;
    Rect {
        x: rect.x + horizontal_padding,
        y: rect.y + vertical_padding,
        width: (rect.width - 2.0 * horizontal_padding).max(1.0),
        height: (rect.height - 2.0 * vertical_padding).max(1.0),
    }
}

fn terminal_point_in_grid(
    x: f32,
    y: f32,
    grid_origin: (f32, f32),
    cell_size: (f32, f32),
    cols: u16,
    rows: u16,
) -> Option<TerminalPoint> {
    let (grid_x, grid_y) = grid_origin;
    let (cell_width, cell_height) = cell_size;
    let relative_x = x - grid_x;
    let relative_y = y - grid_y;
    if relative_x < 0.0
        || relative_y < 0.0
        || relative_x >= f32::from(cols) * cell_width
        || relative_y >= f32::from(rows) * cell_height
    {
        return None;
    }
    Some(TerminalPoint {
        column: (relative_x / cell_width).floor() as u16,
        row: (relative_y / cell_height).floor() as u16,
    })
}

fn scale_rect(rect: Rect, scale: f32) -> Rect {
    Rect {
        x: rect.x * scale,
        y: rect.y * scale,
        width: rect.width * scale,
        height: rect.height * scale,
    }
}

fn rgb(color: Rgb, alpha: f32) -> [f32; 4] {
    [
        srgb_channel_to_linear(color.r),
        srgb_channel_to_linear(color.g),
        srgb_channel_to_linear(color.b),
        alpha,
    ]
}

fn srgb_channel_to_linear(channel: u8) -> f32 {
    let value = f32::from(channel) / 255.0;
    if value <= 0.040_45 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn to_text_color(color: Rgb, faint: bool) -> Color {
    if faint {
        Color::rgba(color.r, color.g, color.b, 140)
    } else {
        Color::rgb(color.r, color.g, color.b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_rgb_is_linearized_for_the_srgb_surface() {
        let color = rgb(
            Rgb {
                r: 0,
                g: 128,
                b: 255,
            },
            0.75,
        );
        assert!(color[0].abs() < f32::EPSILON);
        assert!((color[1] - 0.215_861).abs() < 0.000_001);
        assert!((color[2] - 1.0).abs() < f32::EPSILON);
        assert!((color[3] - 0.75).abs() < f32::EPSILON);
    }

    #[test]
    fn agent_composer_expands_for_wrapped_and_explicit_lines() {
        for (draft, expected) in [
            (String::new(), 64.0),
            ("first\nsecond".to_owned(), 82.0),
            ("x".repeat(53), 82.0),
            ("x\n".repeat(12), 136.0),
        ] {
            assert!((agent_composer_height(&draft) - expected).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn bundled_font_advance_matches_terminal_grid() {
        let mut font_system = FontSystem::new();
        load_terminal_fonts(&mut font_system);
        let mut buffer = Buffer::new(&mut font_system, Metrics::new(FONT_SIZE, CELL_HEIGHT));
        buffer.set_size(&mut font_system, Some(1_000.0), Some(CELL_HEIGHT));
        buffer.set_monospace_width(&mut font_system, Some(CELL_WIDTH));
        buffer.set_wrap(&mut font_system, Wrap::None);
        buffer.set_text(
            &mut font_system,
            "0000000000",
            &Attrs::new().family(Family::Name(TERMINAL_FONT_FAMILY)),
            Shaping::Advanced,
        );
        buffer.shape_until_scroll(&mut font_system, false);
        let width = buffer.layout_runs().next().expect("one shaped row").line_w;
        assert!((width - 10.0 * CELL_WIDTH).abs() < 0.01, "width={width}");
    }

    #[test]
    fn ghostty_font_family_resolution_is_case_insensitive_and_safe() {
        let mut font_system = FontSystem::new();
        load_terminal_fonts(&mut font_system);

        assert_eq!(
            resolve_terminal_font_family(&font_system, Some("jetbrainsmono nerd font mono")),
            TERMINAL_FONT_FAMILY
        );
        assert_eq!(
            resolve_terminal_font_family(&font_system, Some("Definitely Not Installed")),
            TERMINAL_FONT_FAMILY
        );
    }

    #[test]
    fn terminal_grid_uses_the_resolved_face_advance() {
        let mut font_system = FontSystem::new();
        load_terminal_fonts(&mut font_system);
        let measured = measure_terminal_cell_width(
            &mut font_system,
            TERMINAL_FONT_FAMILY,
            FONT_SIZE,
            CELL_HEIGHT,
        )
        .expect("bundled font has a measurable advance");

        assert!((measured - CELL_WIDTH).abs() < 0.01, "measured={measured}");
    }

    #[test]
    fn ligatures_preserve_exact_terminal_columns() {
        let cells = "-> != === <=>"
            .chars()
            .map(|character| render_cell(&character.to_string(), CellWidth::Narrow))
            .collect::<Vec<_>>();
        let specs = terminal_run_specs(&cells);
        assert_eq!(specs.len(), 1);

        let mut font_system = FontSystem::new();
        load_terminal_fonts(&mut font_system);
        let mut buffer = Buffer::new(&mut font_system, Metrics::new(FONT_SIZE, CELL_HEIGHT));
        shape_terminal_run(
            &mut font_system,
            &mut buffer,
            &specs[0],
            Rgb::default(),
            TERMINAL_FONT_FAMILY,
            TerminalGridMetrics {
                font_size: FONT_SIZE,
                cell_width: CELL_WIDTH,
                cell_height: CELL_HEIGHT,
            },
        );
        let width = buffer.layout_runs().next().expect("one shaped row").line_w;
        let expected = cells.len() as f32 * CELL_WIDTH;
        assert!(
            (width - expected).abs() < 0.01,
            "ligature row width={width}, expected={expected}"
        );
    }

    #[test]
    fn unicode_fallback_produces_real_glyphs() {
        let mut font_system = FontSystem::new();
        load_terminal_fonts(&mut font_system);
        let mut buffer = Buffer::new(&mut font_system, Metrics::new(FONT_SIZE, CELL_HEIGHT));
        buffer.set_size(&mut font_system, Some(1_000.0), Some(CELL_HEIGHT));
        buffer.set_monospace_width(&mut font_system, Some(CELL_WIDTH));
        buffer.set_wrap(&mut font_system, Wrap::None);
        buffer.set_text(
            &mut font_system,
            "界🙂",
            &Attrs::new().family(Family::Name(TERMINAL_FONT_FAMILY)),
            Shaping::Advanced,
        );
        buffer.shape_until_scroll(&mut font_system, false);
        let glyphs = buffer
            .layout_runs()
            .next()
            .expect("one shaped row")
            .glyphs
            .to_vec();
        assert_eq!(glyphs.len(), 2);
        assert!(glyphs.iter().all(|glyph| glyph.glyph_id != 0));
    }

    #[test]
    fn wide_fallback_glyphs_keep_exact_grid_columns() {
        let cells = [
            render_cell("界", CellWidth::Wide),
            render_cell("", CellWidth::SpacerTail),
            render_cell("🙂", CellWidth::Wide),
            render_cell("", CellWidth::SpacerTail),
            render_cell("X", CellWidth::Narrow),
        ];
        let specs = terminal_run_specs(&cells);
        assert_eq!(
            specs
                .iter()
                .map(|spec| (spec.column, spec.cell_count))
                .collect::<Vec<_>>(),
            [(0, 2), (2, 2), (4, 1)]
        );

        let mut font_system = FontSystem::new();
        load_terminal_fonts(&mut font_system);
        for spec in &specs[..2] {
            let mut buffer = Buffer::new(&mut font_system, Metrics::new(FONT_SIZE, CELL_HEIGHT));
            let offset = shape_terminal_run(
                &mut font_system,
                &mut buffer,
                spec,
                Rgb::default(),
                TERMINAL_FONT_FAMILY,
                TerminalGridMetrics {
                    font_size: FONT_SIZE,
                    cell_width: CELL_WIDTH,
                    cell_height: CELL_HEIGHT,
                },
            );
            let glyph_width = buffer.layout_runs().next().expect("shaped glyph").line_w;
            assert!(offset >= 0.0);
            assert!(glyph_width + 2.0 * offset <= 2.0 * CELL_WIDTH + 0.01);
        }
    }

    #[test]
    fn pointer_coordinates_respect_padding_and_exact_cell_edges() {
        assert_eq!(
            terminal_point_in_grid(108.3, 219.9, (100.0, 200.0), (8.4, 20.0), 80, 24),
            Some(TerminalPoint { column: 0, row: 0 })
        );
        assert_eq!(
            terminal_point_in_grid(108.4, 220.0, (100.0, 200.0), (8.4, 20.0), 80, 24),
            Some(TerminalPoint { column: 1, row: 1 })
        );
        assert_eq!(
            terminal_point_in_grid(99.9, 200.0, (100.0, 200.0), (8.4, 20.0), 80, 24),
            None
        );
    }

    fn render_cell(grapheme: &str, width: CellWidth) -> mux_terminal::RenderCell {
        mux_terminal::RenderCell {
            grapheme: grapheme.to_owned(),
            foreground: Rgb::default(),
            background: Rgb::default(),
            underline_color: Rgb::default(),
            style: CellStyle::default(),
            width,
            semantic: mux_terminal::SemanticContent::Output,
            selected: false,
            hyperlink: None,
        }
    }
}
