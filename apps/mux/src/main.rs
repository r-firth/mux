// Winit reports physical coordinates as f64/u32 while the bounded GPU layout
// uses f32. Desktop window dimensions are far below f32's exact-integer limit.
#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]

mod backend;
mod layout;
mod render;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use backend::{BackendHandle, CommandMessage};
use layout::WorkspaceGeometry;
use mux_acp::{
    AgentConfigCategory, AgentConfigValue, AgentConfigValueSelection, AgentContext,
    AgentContextKind, AgentEvent, AgentProfile, AgentPrompt, AgentSessionSnapshot,
    AgentSessionStatus, AgentSlashCommand, built_in_agent_profiles,
};
use mux_protocol::{ServerEvent, SessionAttachment, SessionSummary};
use mux_terminal::{
    CellWidth, RenderFrame, Rgb, TerminalEngine, TerminalInteraction, TerminalKey,
    TerminalKeyAction, TerminalKeyEvent, TerminalModifiers, TerminalMouseAction,
    TerminalMouseButton, TerminalMouseEvent, TerminalRenderer, TerminalSelection,
    TerminalSelectionAutoscroll, TerminalSelectionGestureEvent, TerminalSelectionGestureStatus,
    TerminalSize, TerminalViewportScroll,
};
use mux_terminal_ghostty::{GhosttyEngine, GhosttyTheme};
use mux_workspace::{
    Action, InputMode, Key as MuxKey, KeyChord, Keymap, Modifiers, PaneId, Session,
    WorkspaceCommand,
};
use render::{
    AgentLauncherView, AgentSurfaceView, Renderer, SessionSwitcherView, TextPromptView, UiState,
    agent_composer_height,
};
use tracing::{error, info};
use unicode_segmentation::UnicodeSegmentation;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, Ime, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use winit::window::{CursorIcon, Window, WindowId};

enum UserEvent {
    Attached(SessionAttachment),
    Sessions(Vec<SessionSummary>),
    Server(ServerEvent),
    Agents(Vec<AgentSessionSnapshot>),
    AgentStarted(AgentSessionSnapshot),
    Agent(AgentEvent),
    BackendError(String),
    ExitRequested,
}

struct SessionSwitcher {
    entries: Vec<SessionSummary>,
    selected: usize,
    pending_kill: Option<mux_workspace::SessionId>,
}

#[derive(Clone, Copy)]
enum TextPromptKind {
    RenameTab,
    RenameSession(mux_workspace::SessionId),
}

struct TextPrompt {
    kind: TextPromptKind,
    draft: String,
}

struct AgentSurface {
    selected: usize,
    draft: String,
    loading: bool,
    launcher: Option<AgentLauncher>,
    context: AgentContextMode,
    pending_end: Option<mux_workspace::AgentSessionId>,
    timeline_scroll: usize,
    command_selection: usize,
}

const MUX_AGENT_COMMANDS: &[(&str, &str)] = &[
    ("new", "Start a new persistent agent session"),
    ("agents", "Return to running agent sessions"),
    ("cwd", "Inspect or set the next agent working directory"),
    (
        "context",
        "Attach no context, selected text, or the focused pane",
    ),
    ("login", "Authenticate with an agent-advertised ACP method"),
    ("model", "Inspect or select an agent-advertised model"),
    ("effort", "Inspect or select reasoning effort"),
    ("mode", "Inspect or select the agent mode"),
    ("config", "Set another agent-advertised option"),
    ("end", "End the selected persistent agent session"),
    ("help", "Show Mux agent commands"),
];

struct AgentLauncher {
    selected_profile: usize,
    cwd_override: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AgentContextMode {
    None,
    Selection,
    Pane,
}

enum AgentOptionSelector {
    Model,
    Effort,
    Id(String),
}

impl AgentContextMode {
    const fn label(self) -> &'static str {
        match self {
            Self::None => "no pane context",
            Self::Selection => "selected text attached",
            Self::Pane => "focused pane attached",
        }
    }
}

struct PaneReplica {
    engine: GhosttyEngine,
    frame: RenderFrame,
}

#[derive(Clone, Copy)]
struct SelectionDrag {
    pane_id: PaneId,
    autoscroll: TerminalSelectionAutoscroll,
}

struct CursorBlinkState {
    visible: bool,
    next: Option<Instant>,
    last_reset: Option<Instant>,
    reset_pending: bool,
    window_focused: bool,
}

impl Default for CursorBlinkState {
    fn default() -> Self {
        Self {
            visible: true,
            next: None,
            last_reset: None,
            reset_pending: false,
            window_focused: true,
        }
    }
}

struct Application {
    renderer: Option<Renderer>,
    backend: Option<BackendHandle>,
    session: Option<Session>,
    panes: HashMap<PaneId, PaneReplica>,
    dirty_panes: HashSet<PaneId>,
    view_dirty: bool,
    geometry: WorkspaceGeometry,
    sent_sizes: HashMap<PaneId, TerminalSize>,
    keymap: Keymap,
    mode: InputMode,
    modifiers: ModifiersState,
    suppressed_key_releases: HashSet<PhysicalKey>,
    scroll_accumulators: HashMap<PaneId, f32>,
    pressed_mouse_buttons: HashSet<MouseButton>,
    mouse_reporting_pane: Option<PaneId>,
    cursor_position: (f32, f32),
    hovered_hyperlink: Option<(PaneId, String)>,
    hyperlink_click_active: bool,
    selection_drag: Option<SelectionDrag>,
    selection_gesture_pane: Option<PaneId>,
    next_selection_scroll: Option<Instant>,
    selection_clock_origin: Instant,
    cursor_blink: CursorBlinkState,
    selected_pane: Option<PaneId>,
    clipboard: Option<arboard::Clipboard>,
    message: Option<String>,
    event_proxy: Option<EventLoopProxy<UserEvent>>,
    state_dir: Option<PathBuf>,
    session_switcher: Option<SessionSwitcher>,
    text_prompt: Option<TextPrompt>,
    agents: Vec<AgentSessionSnapshot>,
    agent_profiles: Vec<AgentProfile>,
    agent_surface: Option<AgentSurface>,
    agent_surface_progress: f32,
    agent_surface_target: f32,
    last_animation_frame: Option<Instant>,
    ime_preedit: String,
    ghostty_theme: GhosttyTheme,
}

impl Default for Application {
    fn default() -> Self {
        Self {
            renderer: None,
            backend: None,
            session: None,
            panes: HashMap::new(),
            dirty_panes: HashSet::new(),
            view_dirty: false,
            geometry: WorkspaceGeometry::default(),
            sent_sizes: HashMap::new(),
            keymap: Keymap::zellij_default(),
            mode: InputMode::Normal,
            modifiers: ModifiersState::empty(),
            suppressed_key_releases: HashSet::new(),
            scroll_accumulators: HashMap::new(),
            pressed_mouse_buttons: HashSet::new(),
            mouse_reporting_pane: None,
            cursor_position: (0.0, 0.0),
            hovered_hyperlink: None,
            hyperlink_click_active: false,
            selection_drag: None,
            selection_gesture_pane: None,
            next_selection_scroll: None,
            selection_clock_origin: Instant::now(),
            cursor_blink: CursorBlinkState::default(),
            selected_pane: None,
            clipboard: arboard::Clipboard::new().ok(),
            message: None,
            event_proxy: None,
            state_dir: None,
            session_switcher: None,
            text_prompt: None,
            agents: Vec::new(),
            agent_profiles: built_in_agent_profiles(),
            agent_surface: None,
            agent_surface_progress: 0.0,
            agent_surface_target: 0.0,
            last_animation_frame: None,
            ime_preedit: String::new(),
            ghostty_theme: GhosttyTheme::load_user().unwrap_or_default(),
        }
    }
}

impl Application {
    fn attach(&mut self, attachment: SessionAttachment) -> Result<()> {
        let mut panes = HashMap::with_capacity(attachment.panes.len());
        for pane in attachment.panes {
            let checkpoint = pane
                .terminal
                .checkpoint
                .as_ref()
                .ok_or_else(|| anyhow!("daemon returned a non-Ghostty terminal attachment"))?;
            let mut engine = GhosttyEngine::restore(checkpoint)
                .with_context(|| format!("restore terminal pane {}", pane.pane_id))?;
            // Development daemons from before themed terminal creation have
            // an unset (black) libghostty background. Upgrade those replicas
            // locally without disturbing a checkpoint that already carries
            // themed or OSC-modified colours.
            let mut frame = engine.render_frame()?;
            if frame.background == Rgb::default() && !self.ghostty_theme.is_empty() {
                engine.apply_theme(&self.ghostty_theme)?;
                engine.render_frame_into(&mut frame)?;
            }
            for chunk in &pane.terminal.replay {
                engine.apply_output(chunk.sequence, &chunk.bytes)?;
            }
            if !pane.terminal.replay.is_empty() {
                engine.render_frame_into(&mut frame)?;
            }
            panes.insert(pane.pane_id, PaneReplica { engine, frame });
        }
        self.session = Some(attachment.session);
        self.panes = panes;
        // Selections are GUI-local. Reattaching reconstructs emulator replicas,
        // so any transient pointer stream must end rather than being projected
        // onto potentially changed history or geometry.
        self.selected_pane = None;
        self.selection_drag = None;
        self.selection_gesture_pane = None;
        self.next_selection_scroll = None;
        self.hovered_hyperlink = None;
        self.hyperlink_click_active = false;
        if let Some(renderer) = &self.renderer {
            renderer.set_cursor_icon(CursorIcon::Default);
        }
        self.sent_sizes.clear();
        let changed_panes = self.panes.keys().copied().collect();
        self.dirty_panes.clear();
        self.message = None;
        self.sync_cursor_blink(true);
        self.sync_view(&changed_panes)?;
        self.request_redraw();
        Ok(())
    }

    fn apply_server_event(&mut self, event: ServerEvent) -> Result<()> {
        match event {
            ServerEvent::PaneOutput {
                pane_id,
                sequence,
                bytes,
                ..
            } => {
                if let Some(pane) = self.panes.get_mut(&pane_id) {
                    pane.engine.apply_output(sequence, &bytes)?;
                    self.dirty_panes.insert(pane_id);
                    self.note_cursor_activity();
                    self.request_redraw();
                }
            }
            ServerEvent::PaneExited {
                pane_id, status, ..
            } => {
                self.message = Some(format!(
                    "Pane {pane_id} exited{}",
                    status
                        .code
                        .map_or_else(String::new, |code| format!(" ({code})"))
                ));
                self.refresh_view()?;
            }
            ServerEvent::ResyncRequired { .. } | ServerEvent::WorkspaceChanged { .. } => {}
            ServerEvent::Agent(event) => {
                self.apply_agent_event(&event);
                return Ok(());
            }
            ServerEvent::AgentResyncRequired => {
                if let Some(backend) = &self.backend {
                    backend.send(CommandMessage::ListAgents);
                }
            }
        }
        Ok(())
    }

    fn replace_agents(&mut self, agents: Vec<AgentSessionSnapshot>) {
        self.agents = agents;
        if let Some(surface) = &mut self.agent_surface {
            surface.loading = false;
            surface.selected = surface.selected.min(self.agents.len().saturating_sub(1));
            if self.agents.is_empty() {
                surface.launcher.get_or_insert(AgentLauncher {
                    selected_profile: 0,
                    cwd_override: None,
                });
            }
        }
        self.schedule_view_refresh();
    }

    fn agent_started(&mut self, agent: AgentSessionSnapshot) -> Result<()> {
        if let Some(existing) = self.agents.iter_mut().find(|entry| entry.id == agent.id) {
            *existing = agent;
        } else {
            self.agents.push(agent);
        }
        if let Some(surface) = &mut self.agent_surface {
            surface.loading = false;
            surface.selected = self.agents.len().saturating_sub(1);
            surface.launcher = None;
            surface.pending_end = None;
        }
        self.refresh_view()
    }

    fn backend_error(&mut self, message: String) -> Result<()> {
        if let Some(surface) = &mut self.agent_surface {
            surface.loading = false;
        }
        self.message = Some(message);
        self.refresh_view()
    }

    fn apply_agent_event(&mut self, event: &AgentEvent) {
        let found = self
            .agents
            .iter_mut()
            .find(|agent| agent.id == event.session_id())
            .map(|agent| agent.apply(event))
            .is_some();
        if !found {
            if let Some(backend) = &self.backend {
                backend.send(CommandMessage::ListAgents);
            }
            return;
        }
        if matches!(
            event,
            AgentEvent::ConfigUpdated { .. }
                | AgentEvent::ModeUpdated { .. }
                | AgentEvent::AuthenticationStarted { .. }
                | AgentEvent::Closed { .. }
        ) {
            self.message = None;
        }
        if self.agent_surface_target == 0.0 {
            let agent_name = self
                .agents
                .iter()
                .find(|agent| agent.id == event.session_id())
                .map_or("Agent", |agent| agent.name.as_str());
            match event {
                AgentEvent::AuthenticationRequired { .. } => {
                    self.message = Some(format!("{agent_name} needs sign in  ·  ⇧⌘A, then /login"));
                }
                AgentEvent::AuthenticationStarted { .. } => {
                    self.message = Some(format!("Signing in to {agent_name}…"));
                }
                AgentEvent::AuthenticationFailed { message, .. } => {
                    self.message = Some(format!("{agent_name} sign in failed: {message}"));
                }
                AgentEvent::PermissionRequested { .. } => {
                    self.message = Some(format!("{agent_name} needs permission  ·  ⇧⌘A to review"));
                }
                AgentEvent::Completed { .. } => {
                    self.message = Some(format!("{agent_name} finished  ·  ⇧⌘A to open"));
                }
                AgentEvent::Failed { message, .. } => {
                    self.message = Some(format!("{agent_name}: {message}"));
                }
                _ => {}
            }
        }
        self.schedule_view_refresh();
    }

    fn sync_view(&mut self, changed_panes: &HashSet<PaneId>) -> Result<()> {
        let (Some(renderer), Some(session)) = (&self.renderer, &self.session) else {
            return Ok(());
        };
        let scale = renderer.window_scale_factor();
        let width = renderer.width() as f32 / scale;
        let height = renderer.height() as f32 / scale;
        self.geometry = layout::calculate(session, width, height, self.mode != InputMode::Normal);
        let sizes = self
            .geometry
            .panes
            .iter()
            .map(|pane| (pane.pane_id, renderer.terminal_size(*pane)))
            .collect::<Vec<_>>();

        let mut effective_changes = changed_panes.clone();
        for (pane_id, size) in sizes {
            if self.sent_sizes.insert(pane_id, size) == Some(size) {
                continue;
            }
            if let Some(pane) = self.panes.get_mut(&pane_id) {
                pane.engine.resize(size)?;
                pane.engine.render_frame_into(&mut pane.frame)?;
                effective_changes.insert(pane_id);
            }
            if let Some(backend) = &self.backend {
                backend.send(CommandMessage::Resize { pane_id, size });
            }
        }

        let command_suggestions = self.agent_command_suggestions();
        let frames = self
            .panes
            .iter()
            .map(|(pane_id, replica)| (*pane_id, &replica.frame))
            .collect::<HashMap<_, _>>();
        let renderer = self.renderer.as_mut().expect("renderer checked above");
        let session = self.session.as_ref().expect("session checked above");
        renderer.sync(
            session,
            &self.geometry,
            &frames,
            &effective_changes,
            &UiState {
                mode: self.mode,
                message: self.message.as_deref(),
                session_switcher: if self.text_prompt.is_none() {
                    self.session_switcher
                        .as_ref()
                        .map(|switcher| SessionSwitcherView {
                            entries: &switcher.entries,
                            selected: switcher.selected,
                            pending_kill: switcher.pending_kill,
                        })
                } else {
                    None
                },
                text_prompt: self.text_prompt.as_ref().map(|prompt| TextPromptView {
                    label: match prompt.kind {
                        TextPromptKind::RenameTab => "Rename tab",
                        TextPromptKind::RenameSession(_) => "Rename session",
                    },
                    draft: &prompt.draft,
                }),
                agent_surface: self.agent_surface.as_ref().map(|surface| AgentSurfaceView {
                    entries: &self.agents,
                    selected: surface.selected,
                    draft: &surface.draft,
                    loading: surface.loading,
                    progress: self.agent_surface_progress,
                    launcher: surface.launcher.as_ref().map(|launcher| AgentLauncherView {
                        profiles: &self.agent_profiles,
                        selected: launcher.selected_profile,
                        cwd_override: launcher.cwd_override.as_deref(),
                    }),
                    context_label: surface.context.label(),
                    notice: self.message.as_deref(),
                    timeline_scroll: surface.timeline_scroll,
                    command_suggestions: &command_suggestions,
                    command_selection: surface
                        .command_selection
                        .min(command_suggestions.len().saturating_sub(1)),
                }),
                ime_preedit: (!self.ime_preedit.is_empty()).then_some(self.ime_preedit.as_str()),
                hovered_hyperlink: self
                    .hovered_hyperlink
                    .as_ref()
                    .map(|(pane_id, uri)| (*pane_id, uri.as_str())),
                cursor_blink_visible: self.cursor_blink.visible,
            },
        );
        Ok(())
    }

    fn refresh_view(&mut self) -> Result<()> {
        self.sync_view(&HashSet::new())?;
        self.view_dirty = false;
        self.request_redraw();
        Ok(())
    }

    fn schedule_view_refresh(&mut self) {
        self.view_dirty = true;
        self.request_redraw();
    }

    fn flush_terminal_frames(&mut self) -> Result<()> {
        if self.dirty_panes.is_empty() && !self.view_dirty {
            return Ok(());
        }
        let changed_panes = std::mem::take(&mut self.dirty_panes);
        for pane_id in &changed_panes {
            if let Some(pane) = self.panes.get_mut(pane_id) {
                pane.engine.render_frame_into(&mut pane.frame)?;
            }
        }
        let reset_cursor = std::mem::take(&mut self.cursor_blink.reset_pending);
        self.sync_cursor_blink(reset_cursor);
        self.update_hyperlink_hover();
        self.sync_view(&changed_panes)?;
        self.view_dirty = false;
        Ok(())
    }

    fn focused_cursor_blinks(&self) -> bool {
        self.cursor_blink.window_focused
            && self
                .focused_pane()
                .and_then(|pane_id| self.panes.get(&pane_id))
                .and_then(|pane| pane.frame.cursor)
                .is_some_and(|cursor| cursor.visible && cursor.blinking)
    }

    fn sync_cursor_blink(&mut self, reset: bool) {
        if !self.focused_cursor_blinks() {
            self.cursor_blink.visible = true;
            self.cursor_blink.next = None;
            return;
        }
        if reset || self.cursor_blink.next.is_none() {
            self.cursor_blink.visible = true;
            self.cursor_blink.next = Some(Instant::now() + Duration::from_millis(600));
        }
    }

    fn note_cursor_activity(&mut self) {
        let now = Instant::now();
        if self
            .cursor_blink
            .last_reset
            .is_none_or(|last| now.duration_since(last) > Duration::from_millis(500))
        {
            self.cursor_blink.last_reset = Some(now);
            self.cursor_blink.reset_pending = true;
        }
    }

    fn advance_cursor_blink(&mut self) -> bool {
        let Some(deadline) = self.cursor_blink.next else {
            return false;
        };
        if Instant::now() < deadline {
            return false;
        }
        if !self.focused_cursor_blinks() {
            self.sync_cursor_blink(false);
            return false;
        }
        self.cursor_blink.visible = !self.cursor_blink.visible;
        self.cursor_blink.next = Some(Instant::now() + Duration::from_millis(600));
        true
    }

    fn request_redraw(&self) {
        if let Some(renderer) = &self.renderer {
            renderer.request_redraw();
        }
    }

    fn handle_key(&mut self, event: &KeyEvent) {
        if event.state == ElementState::Released
            && self.suppressed_key_releases.remove(&event.physical_key)
        {
            return;
        }

        if self.text_prompt.is_some() {
            if event.state == ElementState::Pressed {
                self.suppress_key_release(event);
                self.handle_text_prompt_key(event);
            }
            return;
        }

        if self.session_switcher.is_some() {
            if event.state == ElementState::Pressed {
                self.suppress_key_release(event);
                self.handle_session_switcher_key(event);
            }
            return;
        }

        if self.agent_surface.is_some() {
            if event.state == ElementState::Pressed {
                self.suppress_key_release(event);
                self.handle_agent_surface_key(event);
            }
            return;
        }

        if event.state == ElementState::Pressed {
            if self.handle_scrollback_shortcut(event) {
                self.suppress_key_release(event);
                return;
            }
            if self.handle_clipboard_shortcut(event) {
                self.suppress_key_release(event);
                return;
            }
            let chord = key_chord(event, self.modifiers);
            if let Some(action) =
                chord.and_then(|chord| self.keymap.resolve(self.mode, chord).cloned())
            {
                self.suppress_key_release(event);
                self.execute_action(action);
                return;
            }
        }

        if self.mode != InputMode::Normal {
            if event.state == ElementState::Pressed {
                self.suppress_key_release(event);
            }
            return;
        }

        if event.state == ElementState::Pressed && self.message.take().is_some() {
            let _ = self.refresh_view();
        }
        self.write_terminal_key(event);
    }

    fn handle_text_prompt_key(&mut self, event: &KeyEvent) {
        let key = event.key_without_modifiers();
        if self.modifiers.control_key()
            && matches!(key, Key::Character(ref value) if value.eq_ignore_ascii_case("c"))
        {
            self.text_prompt = None;
            self.ime_preedit.clear();
            self.mode = InputMode::Normal;
            let _ = self.refresh_view();
            return;
        }
        match key {
            Key::Named(NamedKey::Escape) => {
                self.text_prompt = None;
                self.ime_preedit.clear();
            }
            Key::Named(NamedKey::Enter) => {
                let prompt = self
                    .text_prompt
                    .take()
                    .expect("text prompt is open while handling its key");
                let value = prompt.draft.trim().to_owned();
                self.ime_preedit.clear();
                self.mode = InputMode::Normal;
                if !value.is_empty() {
                    match prompt.kind {
                        TextPromptKind::RenameTab => {
                            self.send_workspace(WorkspaceCommand::RenameTab(value));
                        }
                        TextPromptKind::RenameSession(session_id) => {
                            if let Some(backend) = &self.backend {
                                backend.send(CommandMessage::RenameSession {
                                    session_id,
                                    name: value,
                                });
                            }
                            self.session_switcher = None;
                        }
                    }
                    return;
                }
            }
            Key::Named(NamedKey::Backspace) => {
                if let Some(prompt) = &mut self.text_prompt {
                    pop_grapheme(&mut prompt.draft);
                }
            }
            Key::Character(ref value)
                if self.modifiers.control_key() && value.eq_ignore_ascii_case("u") =>
            {
                if let Some(prompt) = &mut self.text_prompt {
                    prompt.draft.clear();
                }
            }
            _ if !self.modifiers.control_key() && !self.modifiers.super_key() => {
                if let Some(text) = event.text.as_deref().filter(|text| {
                    !text.is_empty() && text.chars().all(|character| !character.is_control())
                }) && let Some(prompt) = &mut self.text_prompt
                {
                    prompt.draft.push_str(text);
                }
            }
            _ => {}
        }
        let _ = self.refresh_view();
    }

    fn handle_session_switcher_key(&mut self, event: &KeyEvent) {
        let key = event.key_without_modifiers();
        let mut attach = None;
        let mut create = false;
        let mut rename = None;
        let mut kill = None;
        let mut close = false;
        if let Some(switcher) = &mut self.session_switcher {
            let len = switcher.entries.len();
            match key {
                Key::Named(NamedKey::Escape) => close = true,
                Key::Named(NamedKey::ArrowUp) => {
                    if len > 0 {
                        switcher.selected = (switcher.selected + len - 1) % len;
                        switcher.pending_kill = None;
                    }
                }
                Key::Character(ref value) if value.eq_ignore_ascii_case("k") => {
                    if len > 0 {
                        switcher.selected = (switcher.selected + len - 1) % len;
                        switcher.pending_kill = None;
                    }
                }
                Key::Named(NamedKey::ArrowDown) => {
                    if len > 0 {
                        switcher.selected = (switcher.selected + 1) % len;
                        switcher.pending_kill = None;
                    }
                }
                Key::Character(ref value) if value.eq_ignore_ascii_case("j") => {
                    if len > 0 {
                        switcher.selected = (switcher.selected + 1) % len;
                        switcher.pending_kill = None;
                    }
                }
                Key::Named(NamedKey::Enter) => {
                    attach = switcher
                        .entries
                        .get(switcher.selected)
                        .map(|session| session.id);
                }
                Key::Character(ref value) if value.eq_ignore_ascii_case("n") => create = true,
                Key::Character(ref value) if value.eq_ignore_ascii_case("r") => {
                    rename = switcher.entries.get(switcher.selected).cloned();
                }
                Key::Character(ref value) if value.eq_ignore_ascii_case("x") => {
                    if let Some(session) = switcher.entries.get(switcher.selected) {
                        if switcher.pending_kill == Some(session.id) {
                            kill = Some(session.id);
                        } else {
                            switcher.pending_kill = Some(session.id);
                        }
                    }
                }
                Key::Character(ref value) => {
                    attach = value
                        .parse::<usize>()
                        .ok()
                        .and_then(|number| number.checked_sub(1))
                        .and_then(|index| switcher.entries.get(index))
                        .map(|session| session.id);
                }
                _ => {}
            }
        }
        if let Some(session_id) = attach {
            if let Some(backend) = &self.backend {
                backend.send(CommandMessage::AttachSession(session_id));
            }
            self.session_switcher = None;
        } else if create {
            let name = self.next_session_name();
            if let (Some(backend), Some(pane_id)) = (&self.backend, self.focused_pane()) {
                backend.send(CommandMessage::CreateSessionForPane { name, pane_id });
                self.session_switcher = None;
            }
        } else if let Some(session) = rename {
            self.text_prompt = Some(TextPrompt {
                kind: TextPromptKind::RenameSession(session.id),
                draft: session.name,
            });
        } else if let Some(session_id) = kill {
            if let Some(backend) = &self.backend {
                backend.send(CommandMessage::KillSession(session_id));
            }
            self.session_switcher = None;
        } else if close {
            self.session_switcher = None;
        }
        let _ = self.refresh_view();
    }

    fn next_session_name(&self) -> String {
        let names = self
            .session_switcher
            .as_ref()
            .map(|switcher| {
                switcher
                    .entries
                    .iter()
                    .map(|session| session.name.as_str())
                    .collect::<HashSet<_>>()
            })
            .unwrap_or_default();
        (1..=names.len() + 1)
            .map(|number| format!("session {number}"))
            .find(|name| !names.contains(name.as_str()))
            .expect("N sessions cannot fill N + 1 numbered names")
    }

    fn handle_agent_surface_key(&mut self, event: &KeyEvent) {
        let key = event.key_without_modifiers();
        if is_agent_surface_shortcut(event, self.modifiers)
            || matches!(key, Key::Named(NamedKey::Escape))
        {
            self.close_agent_surface();
            return;
        }

        if self.handle_agent_composer_shortcut(&key) {
            return;
        }

        if self.modifiers.control_key()
            && matches!(key, Key::Character(ref value) if value.eq_ignore_ascii_case("c"))
        {
            self.cancel_active_agent();
            return;
        }

        if let Some((session_id, request_id, option_id)) = self.permission_choice(&key) {
            if let Some(backend) = &self.backend {
                backend.send(CommandMessage::ResolveAgentPermission {
                    session_id,
                    request_id,
                    option_id: Some(option_id),
                });
            }
            return;
        }

        match key {
            Key::Named(NamedKey::Enter) => self.submit_agent_surface(),
            Key::Named(NamedKey::Backspace) => {
                if let Some(surface) = &mut self.agent_surface {
                    pop_grapheme(&mut surface.draft);
                    surface.command_selection = 0;
                }
                let _ = self.refresh_view();
            }
            Key::Named(NamedKey::ArrowUp) => {
                let suggestion_count = self.agent_command_suggestions().len();
                if let Some(surface) = &mut self.agent_surface {
                    if suggestion_count > 0 {
                        surface.command_selection =
                            (surface.command_selection + suggestion_count - 1) % suggestion_count;
                    } else if surface.draft.is_empty()
                        && let Some(launcher) = &mut surface.launcher
                    {
                        if !self.agent_profiles.is_empty() {
                            launcher.selected_profile =
                                (launcher.selected_profile + self.agent_profiles.len() - 1)
                                    % self.agent_profiles.len();
                        }
                    } else if surface.draft.is_empty() && !self.agents.is_empty() {
                        surface.selected =
                            (surface.selected + self.agents.len() - 1) % self.agents.len();
                        surface.timeline_scroll = 0;
                    }
                }
                let _ = self.refresh_view();
            }
            Key::Named(NamedKey::Tab) if self.complete_agent_slash_command() => {}
            Key::Named(NamedKey::ArrowDown | NamedKey::Tab) => {
                let suggestion_count = self.agent_command_suggestions().len();
                if let Some(surface) = &mut self.agent_surface {
                    if suggestion_count > 0 {
                        surface.command_selection =
                            (surface.command_selection + 1) % suggestion_count;
                    } else if surface.draft.is_empty()
                        && let Some(launcher) = &mut surface.launcher
                    {
                        if !self.agent_profiles.is_empty() {
                            launcher.selected_profile =
                                (launcher.selected_profile + 1) % self.agent_profiles.len();
                        }
                    } else if surface.draft.is_empty() && !self.agents.is_empty() {
                        surface.selected = (surface.selected + 1) % self.agents.len();
                        surface.timeline_scroll = 0;
                    }
                }
                let _ = self.refresh_view();
            }
            Key::Named(NamedKey::PageUp) => self.scroll_agent_timeline(6),
            Key::Named(NamedKey::PageDown) => self.scroll_agent_timeline(-6),
            Key::Named(NamedKey::Home) => self.scroll_agent_timeline(isize::MAX),
            Key::Named(NamedKey::End) => self.scroll_agent_timeline(isize::MIN),
            _ if !self.modifiers.control_key() && !self.modifiers.super_key() => {
                if let Some(text) = event.text.as_deref().filter(|text| {
                    !text.is_empty() && text.chars().all(|character| !character.is_control())
                }) {
                    if let Some(surface) = &mut self.agent_surface {
                        surface.draft.push_str(text);
                        surface.command_selection = 0;
                    }
                    let _ = self.refresh_view();
                }
            }
            _ => {}
        }
    }

    fn cancel_active_agent(&self) {
        let session_id = self
            .agent_surface
            .as_ref()
            .filter(|surface| surface.launcher.is_none())
            .and_then(|surface| self.agents.get(surface.selected))
            .map(|agent| agent.id);
        if let (Some(backend), Some(session_id)) = (&self.backend, session_id) {
            backend.send(CommandMessage::CancelAgent(session_id));
        }
    }

    fn handle_agent_composer_shortcut(&mut self, key: &Key) -> bool {
        if self.modifiers.super_key()
            && matches!(key, Key::Character(value) if value.eq_ignore_ascii_case("v"))
        {
            match self
                .clipboard
                .as_mut()
                .ok_or_else(|| anyhow!("system clipboard is unavailable"))
                .and_then(|clipboard| clipboard.get_text().context("read system clipboard"))
            {
                Ok(text) => {
                    if let Some(surface) = &mut self.agent_surface {
                        surface.draft.push_str(&text);
                        surface.command_selection = 0;
                    }
                    self.message = None;
                }
                Err(error) => self.message = Some(format!("{error:#}")),
            }
        } else if (self.modifiers.control_key()
            && matches!(key, Key::Character(value) if value.eq_ignore_ascii_case("u")))
            || (self.modifiers.super_key() && matches!(key, Key::Named(NamedKey::Backspace)))
        {
            if let Some(surface) = &mut self.agent_surface {
                surface.draft.clear();
                surface.command_selection = 0;
            }
        } else if self.modifiers.shift_key() && matches!(key, Key::Named(NamedKey::Enter)) {
            if let Some(surface) = &mut self.agent_surface {
                surface.draft.push('\n');
                surface.command_selection = 0;
            }
        } else {
            return false;
        }
        let _ = self.refresh_view();
        true
    }

    fn agent_command_suggestions(&self) -> Vec<AgentSlashCommand> {
        let Some(surface) = &self.agent_surface else {
            return Vec::new();
        };
        let Some(command) = surface.draft.strip_prefix('/') else {
            return Vec::new();
        };
        if command.chars().any(char::is_whitespace) {
            return Vec::new();
        }
        if surface.launcher.is_some() {
            return Vec::new();
        }
        let Some(agent) = self.agents.get(surface.selected) else {
            return Vec::new();
        };
        if agent.pending_permission().is_some() {
            return Vec::new();
        }
        let prefix = command.to_ascii_lowercase();
        let mut commands = Vec::new();
        let mut seen = HashSet::new();

        for advertised in &agent.available_commands {
            let name = advertised.name.trim_start_matches('/');
            let normalized = name.to_ascii_lowercase();
            if normalized.starts_with(&prefix)
                && !MUX_AGENT_COMMANDS
                    .iter()
                    .any(|(local, _)| local.eq_ignore_ascii_case(name))
                && seen.insert(normalized)
            {
                commands.push(AgentSlashCommand {
                    name: name.to_owned(),
                    description: advertised.description.clone(),
                });
            }
        }

        for &(name, description) in MUX_AGENT_COMMANDS {
            let normalized = name.to_ascii_lowercase();
            if normalized.starts_with(&prefix) && seen.insert(normalized) {
                commands.push(AgentSlashCommand {
                    name: name.to_owned(),
                    description: description.to_owned(),
                });
            }
        }
        commands.truncate(8);
        commands
    }

    fn complete_agent_slash_command(&mut self) -> bool {
        let suggestions = self.agent_command_suggestions();
        let Some(surface) = &mut self.agent_surface else {
            return false;
        };
        let Some(command) = suggestions.get(surface.command_selection) else {
            return false;
        };
        surface.draft = format!("/{} ", command.name);
        surface.command_selection = 0;
        let _ = self.refresh_view();
        true
    }

    fn permission_choice(
        &self,
        key: &Key,
    ) -> Option<(mux_workspace::AgentSessionId, String, String)> {
        let Key::Character(value) = key else {
            return None;
        };
        let index = value.parse::<usize>().ok()?.checked_sub(1)?;
        let surface = self.agent_surface.as_ref()?;
        if surface.launcher.is_some() {
            return None;
        }
        let agent = self.agents.get(surface.selected)?;
        let permission = agent.pending_permission()?;
        let option = permission.options.get(index)?;
        Some((agent.id, permission.request_id.clone(), option.id.clone()))
    }

    fn submit_agent_surface(&mut self) {
        let Some((draft, loading, launcher, selected, context_mode)) =
            self.agent_surface.as_ref().map(|surface| {
                (
                    surface.draft.trim().to_owned(),
                    surface.loading,
                    surface
                        .launcher
                        .as_ref()
                        .map(|launcher| (launcher.selected_profile, launcher.cwd_override.clone())),
                    surface.selected,
                    surface.context,
                )
            })
        else {
            return;
        };
        if draft.starts_with('/') && self.handle_agent_slash_command(&draft) {
            if let Some(surface) = &mut self.agent_surface {
                surface.draft.clear();
                surface.command_selection = 0;
            }
            let _ = self.refresh_view();
            return;
        }

        if let Some((selected_profile, cwd_override)) = launcher {
            if loading {
                return;
            }
            let Some(profile) = self.agent_profiles.get(selected_profile).cloned() else {
                return;
            };
            let Some(pane_id) = self.focused_pane_id() else {
                self.message = Some("No focused pane is available for the agent".to_owned());
                let _ = self.refresh_view();
                return;
            };
            if let Some(surface) = &mut self.agent_surface {
                surface.loading = true;
            }
            if let Some(backend) = &self.backend {
                backend.send(CommandMessage::StartAgent {
                    spec: profile.spec,
                    pane_id,
                    cwd_override,
                });
            }
            let _ = self.refresh_view();
            return;
        }

        let Some(agent) = self.agents.get(selected) else {
            return;
        };
        if draft.is_empty()
            || matches!(
                agent.status,
                AgentSessionStatus::Starting
                    | AgentSessionStatus::WaitingForAuthentication
                    | AgentSessionStatus::Authenticating
                    | AgentSessionStatus::Closed
            )
        {
            return;
        }
        let session_id = agent.id;
        match self.agent_prompt_context(context_mode) {
            Ok(context) => {
                if let Some(surface) = &mut self.agent_surface {
                    surface.draft.clear();
                    surface.pending_end = None;
                    surface.timeline_scroll = 0;
                    surface.command_selection = 0;
                }
                if let Some(backend) = &self.backend {
                    backend.send(CommandMessage::PromptAgent {
                        session_id,
                        prompt: AgentPrompt {
                            text: draft,
                            context,
                        },
                    });
                }
            }
            Err(error) => self.message = Some(error.to_string()),
        }
        let _ = self.refresh_view();
    }

    #[allow(clippy::too_many_lines)]
    fn handle_agent_slash_command(&mut self, command: &str) -> bool {
        let mut parts = command[1..].split_whitespace();
        let Some(name) = parts.next().map(str::to_ascii_lowercase) else {
            return false;
        };
        let arguments = parts.collect::<Vec<_>>();
        match name.as_str() {
            "new" => {
                let requested = arguments.join(" ").to_ascii_lowercase();
                let selected_profile = if requested.is_empty() {
                    0
                } else {
                    self.agent_profiles
                        .iter()
                        .position(|profile| {
                            profile.id.eq_ignore_ascii_case(&requested)
                                || profile.name.to_ascii_lowercase().contains(&requested)
                        })
                        .unwrap_or(0)
                };
                if let Some(surface) = &mut self.agent_surface {
                    surface.launcher = Some(AgentLauncher {
                        selected_profile,
                        cwd_override: None,
                    });
                    surface.loading = false;
                    surface.pending_end = None;
                    surface.timeline_scroll = 0;
                }
                true
            }
            "agents" | "sessions" => {
                if self.agents.is_empty() {
                    self.message = Some("No running agent sessions · use /new".to_owned());
                } else if let Some(surface) = &mut self.agent_surface {
                    surface.launcher = None;
                    surface.selected = surface.selected.min(self.agents.len() - 1);
                    surface.timeline_scroll = 0;
                }
                true
            }
            "cwd" => {
                let requested = arguments.join(" ");
                if requested.is_empty() {
                    self.message = Some(self.agent_cwd_description());
                    return true;
                }
                let path = expand_home_path(&requested);
                if !path.is_absolute() || !path.is_dir() {
                    self.message = Some(format!(
                        "Agent cwd must be an existing absolute directory: {}",
                        path.display()
                    ));
                } else if let Some(surface) = &mut self.agent_surface {
                    if let Some(launcher) = &mut surface.launcher {
                        launcher.cwd_override = Some(path.clone());
                        self.message = Some(format!("New agent cwd · {}", path.display()));
                    } else {
                        self.message = Some(
                            "A running agent's cwd is fixed · use /new, then /cwd <path>"
                                .to_owned(),
                        );
                    }
                }
                true
            }
            "context" => {
                let requested = arguments.first().copied().unwrap_or("");
                if let Some(surface) = &mut self.agent_surface {
                    surface.context = match requested.to_ascii_lowercase().as_str() {
                        "none" | "off" => AgentContextMode::None,
                        "selection" | "selected" => AgentContextMode::Selection,
                        "pane" | "screen" | "viewport" => AgentContextMode::Pane,
                        "" => match surface.context {
                            AgentContextMode::None => AgentContextMode::Selection,
                            AgentContextMode::Selection => AgentContextMode::Pane,
                            AgentContextMode::Pane => AgentContextMode::None,
                        },
                        _ => {
                            self.message = Some("Usage: /context none|selection|pane".to_owned());
                            return true;
                        }
                    };
                    self.message = Some(format!("Agent context · {}", surface.context.label()));
                }
                true
            }
            "login" | "auth" => {
                self.authenticate_active_agent(arguments.first().copied());
                true
            }
            "end" | "close" => {
                let Some((selected, session_id)) = self.active_agent_selection() else {
                    self.message = Some("No active agent session to end".to_owned());
                    return true;
                };
                let confirmed = self
                    .agent_surface
                    .as_ref()
                    .is_some_and(|surface| surface.pending_end == Some(session_id));
                if confirmed {
                    if let Some(backend) = &self.backend {
                        backend.send(CommandMessage::CloseAgent(session_id));
                    }
                    if let Some(surface) = &mut self.agent_surface {
                        surface.pending_end = None;
                    }
                    self.message = Some(format!("Ending {}…", self.agents[selected].name));
                } else {
                    if let Some(surface) = &mut self.agent_surface {
                        surface.pending_end = Some(session_id);
                    }
                    self.message = Some("Run /end again to confirm".to_owned());
                }
                true
            }
            "mode" => {
                self.configure_agent_mode(arguments.first().copied());
                true
            }
            "model" => {
                self.configure_agent_option(
                    &AgentOptionSelector::Model,
                    arguments.first().copied(),
                );
                true
            }
            "effort" | "reasoning" => {
                self.configure_agent_option(
                    &AgentOptionSelector::Effort,
                    arguments.first().copied(),
                );
                true
            }
            "config" => {
                let Some(config_id) = arguments.first().copied() else {
                    self.message = Some("Usage: /config <id> <value>".to_owned());
                    return true;
                };
                self.configure_agent_option(
                    &AgentOptionSelector::Id(config_id.to_owned()),
                    arguments.get(1).copied(),
                );
                true
            }
            "help" => {
                self.message = Some(
                    "/new · /agents · /cwd · /context · /login · /model · /effort · /mode · /end"
                        .to_owned(),
                );
                true
            }
            _ => false,
        }
    }

    fn active_agent_selection(&self) -> Option<(usize, mux_workspace::AgentSessionId)> {
        let surface = self.agent_surface.as_ref()?;
        if surface.launcher.is_some() {
            return None;
        }
        let agent = self.agents.get(surface.selected)?;
        Some((surface.selected, agent.id))
    }

    fn authenticate_active_agent(&mut self, requested: Option<&str>) {
        let Some((selected, session_id)) = self.active_agent_selection() else {
            self.message = Some("Start or select an agent session first".to_owned());
            return;
        };
        let agent = &self.agents[selected];
        if agent.status == AgentSessionStatus::Authenticating {
            self.message = Some("Agent sign in is already in progress".to_owned());
            return;
        }
        if agent.status != AgentSessionStatus::WaitingForAuthentication {
            self.message = Some("This agent session does not currently require sign in".to_owned());
            return;
        }
        if agent.auth_methods.is_empty() {
            self.message =
                Some("The agent requested sign in without advertising a method".to_owned());
            return;
        }

        let method = if let Some(requested) = requested {
            let requested = requested.to_ascii_lowercase();
            agent.auth_methods.iter().find(|method| {
                method.id.eq_ignore_ascii_case(&requested)
                    || method.name.eq_ignore_ascii_case(&requested)
                    || method.name.to_ascii_lowercase().contains(&requested)
            })
        } else if agent.auth_methods.len() == 1 {
            agent.auth_methods.first()
        } else {
            self.message = Some(format!(
                "Choose a sign-in method · {}",
                agent
                    .auth_methods
                    .iter()
                    .map(|method| format!("/login {}", method.id))
                    .collect::<Vec<_>>()
                    .join(" · ")
            ));
            return;
        };
        let Some(method) = method else {
            self.message = Some(format!(
                "Unknown sign-in method · {}",
                agent
                    .auth_methods
                    .iter()
                    .map(|method| method.id.as_str())
                    .collect::<Vec<_>>()
                    .join(" · ")
            ));
            return;
        };
        let method_id = method.id.clone();
        let method_name = method.name.clone();
        if let Some(backend) = &self.backend {
            backend.send(CommandMessage::AuthenticateAgent {
                session_id,
                method_id,
            });
        }
        self.message = Some(format!("Starting {method_name}…"));
    }

    fn configure_agent_mode(&mut self, requested: Option<&str>) {
        let Some((selected, session_id)) = self.active_agent_selection() else {
            self.message = Some("Start or select an agent session first".to_owned());
            return;
        };
        let agent = &self.agents[selected];
        let Some(requested) = requested else {
            let choices = agent
                .modes
                .iter()
                .map(|mode| mode.id.as_str())
                .collect::<Vec<_>>()
                .join(" · ");
            self.message = Some(if choices.is_empty() {
                "This agent does not expose session modes".to_owned()
            } else {
                format!(
                    "Mode {} · {choices}",
                    agent.current_mode.as_deref().unwrap_or("unknown")
                )
            });
            return;
        };
        let Some(mode) = agent.modes.iter().find(|mode| {
            mode.id.eq_ignore_ascii_case(requested) || mode.name.eq_ignore_ascii_case(requested)
        }) else {
            self.message = Some(format!("Unknown agent mode: {requested}"));
            return;
        };
        let mode_id = mode.id.clone();
        if let Some(backend) = &self.backend {
            backend.send(CommandMessage::SetAgentMode {
                session_id,
                mode_id: mode_id.clone(),
            });
        }
        self.message = Some(format!("Agent mode · {mode_id}"));
    }

    fn configure_agent_option(&mut self, selector: &AgentOptionSelector, requested: Option<&str>) {
        let Some((selected, session_id)) = self.active_agent_selection() else {
            self.message = Some("Start or select an agent session first".to_owned());
            return;
        };
        let agent = &self.agents[selected];
        let option = agent.config_options.iter().find(|option| match selector {
            AgentOptionSelector::Model => option.category == AgentConfigCategory::Model,
            AgentOptionSelector::Effort => {
                option.category == AgentConfigCategory::ThoughtLevel
                    || option.id.to_ascii_lowercase().contains("effort")
                    || option.name.to_ascii_lowercase().contains("reason")
            }
            AgentOptionSelector::Id(id) => option.id.eq_ignore_ascii_case(id),
        });
        let Some(option) = option else {
            self.message = Some("This agent does not expose that setting".to_owned());
            return;
        };
        let Some(requested) = requested else {
            self.message = Some(describe_agent_option(option));
            return;
        };
        let value = match &option.value {
            AgentConfigValue::Select { choices, .. } => {
                let Some(choice) = choices.iter().find(|choice| {
                    choice.id.eq_ignore_ascii_case(requested)
                        || choice.name.eq_ignore_ascii_case(requested)
                }) else {
                    self.message = Some(format!(
                        "Unknown {} value: {requested} · {}",
                        option.name,
                        choices
                            .iter()
                            .map(|choice| choice.id.as_str())
                            .collect::<Vec<_>>()
                            .join(" · ")
                    ));
                    return;
                };
                AgentConfigValueSelection::Choice(choice.id.clone())
            }
            AgentConfigValue::Boolean(_) => match requested.to_ascii_lowercase().as_str() {
                "true" | "on" | "yes" | "1" => AgentConfigValueSelection::Boolean(true),
                "false" | "off" | "no" | "0" => AgentConfigValueSelection::Boolean(false),
                _ => {
                    self.message = Some(format!("{} expects on or off", option.name));
                    return;
                }
            },
        };
        let config_id = option.id.clone();
        let option_name = option.name.clone();
        if let Some(backend) = &self.backend {
            backend.send(CommandMessage::SetAgentConfig {
                session_id,
                config_id,
                value,
            });
        }
        self.message = Some(format!("Updating {option_name}…"));
    }

    fn focused_pane_id(&self) -> Option<PaneId> {
        self.session
            .as_ref()?
            .active_tab()
            .map(|tab| tab.focused_pane)
    }

    fn agent_cwd_description(&self) -> String {
        if let Some(surface) = &self.agent_surface
            && let Some(launcher) = &surface.launcher
        {
            return launcher.cwd_override.as_ref().map_or_else(
                || "New agent cwd · focused pane (live)".to_owned(),
                |cwd| format!("New agent cwd · {}", cwd.display()),
            );
        }
        self.active_agent_selection().map_or_else(
            || "Agent cwd · focused pane (live)".to_owned(),
            |(selected, _)| format!("Agent cwd · {}", self.agents[selected].cwd.display()),
        )
    }

    fn agent_prompt_context(&self, mode: AgentContextMode) -> Result<Vec<AgentContext>> {
        match mode {
            AgentContextMode::None => Ok(Vec::new()),
            AgentContextMode::Selection => {
                let pane_id = self
                    .selected_pane
                    .or_else(|| self.focused_pane_id())
                    .ok_or_else(|| anyhow!("No pane is available for context"))?;
                let pane = self
                    .panes
                    .get(&pane_id)
                    .ok_or_else(|| anyhow!("Selected pane is unavailable"))?;
                let text = pane
                    .engine
                    .selected_text()?
                    .filter(|text| !text.trim().is_empty())
                    .ok_or_else(|| {
                        anyhow!("Select terminal text before attaching selection context")
                    })?;
                Ok(vec![AgentContext {
                    kind: AgentContextKind::TerminalSelection,
                    pane_id,
                    label: "terminal selection".to_owned(),
                    text,
                }])
            }
            AgentContextMode::Pane => {
                let pane_id = self
                    .focused_pane_id()
                    .ok_or_else(|| anyhow!("No focused pane is available for context"))?;
                let pane = self
                    .panes
                    .get(&pane_id)
                    .ok_or_else(|| anyhow!("Focused pane is unavailable"))?;
                Ok(vec![AgentContext {
                    kind: AgentContextKind::TerminalViewport,
                    pane_id,
                    label: "focused terminal viewport".to_owned(),
                    text: terminal_frame_text(&pane.frame),
                }])
            }
        }
    }

    fn toggle_agent_surface(&mut self) {
        if self.agent_surface_target > 0.5 {
            self.close_agent_surface();
            return;
        }
        self.mode = InputMode::Normal;
        self.session_switcher = None;
        self.clear_hyperlink_hover();
        self.agent_surface = Some(AgentSurface {
            selected: 0,
            draft: String::new(),
            loading: true,
            launcher: None,
            context: AgentContextMode::None,
            pending_end: None,
            timeline_scroll: 0,
            command_selection: 0,
        });
        self.agent_surface_target = 1.0;
        self.last_animation_frame = Some(Instant::now());
        if let Some(backend) = &self.backend {
            backend.send(CommandMessage::ListAgents);
        }
        let _ = self.refresh_view();
    }

    fn close_agent_surface(&mut self) {
        self.agent_surface_target = 0.0;
        self.last_animation_frame = Some(Instant::now());
        self.request_redraw();
    }

    fn advance_ui_animation(&mut self) -> bool {
        if (self.agent_surface_progress - self.agent_surface_target).abs() < 0.002 {
            self.agent_surface_progress = self.agent_surface_target;
            self.last_animation_frame = None;
            if self.agent_surface_target == 0.0 {
                self.agent_surface = None;
            }
            return false;
        }
        let now = Instant::now();
        let elapsed = self
            .last_animation_frame
            .replace(now)
            .map_or(1.0 / 60.0, |last| (now - last).as_secs_f32().min(0.05));
        let blend = 1.0 - (-18.0 * elapsed).exp();
        self.agent_surface_progress +=
            (self.agent_surface_target - self.agent_surface_progress) * blend;
        true
    }

    fn suppress_key_release(&mut self, event: &KeyEvent) {
        self.suppressed_key_releases.insert(event.physical_key);
    }

    fn write_terminal_key(&mut self, event: &KeyEvent) {
        let terminal_event = terminal_key_event(event, self.modifiers);
        let result = (|| -> Result<(Vec<u8>, Option<PaneId>)> {
            let pane_id = self
                .focused_pane()
                .ok_or_else(|| anyhow!("no focused terminal pane"))?;
            let pane = self
                .panes
                .get_mut(&pane_id)
                .ok_or_else(|| anyhow!("focused pane is unavailable"))?;
            let bytes = pane
                .engine
                .encode_key(&terminal_event)
                .map_err(anyhow::Error::from)?;
            let snapped_to_bottom = !bytes.is_empty()
                && terminal_event.action != TerminalKeyAction::Release
                && pane.frame.scroll.is_scrolled();
            if snapped_to_bottom {
                pane.engine
                    .scroll_viewport(TerminalViewportScroll::Bottom)?;
                pane.engine.render_frame_into(&mut pane.frame)?;
            }
            Ok((bytes, snapped_to_bottom.then_some(pane_id)))
        })();
        match result {
            Ok((bytes, changed_pane)) => {
                if let Some(pane_id) = changed_pane
                    && let Err(error) = self.sync_view(&HashSet::from([pane_id]))
                {
                    self.message = Some(format!("restore terminal viewport: {error:#}"));
                    let _ = self.refresh_view();
                    return;
                }
                if !bytes.is_empty() {
                    if terminal_event.action != TerminalKeyAction::Release {
                        self.prepare_terminal_input();
                    }
                    self.write_focused(bytes);
                }
            }
            Err(error) => {
                self.message = Some(format!("encode terminal key: {error:#}"));
                let _ = self.refresh_view();
            }
        }
    }

    fn handle_scrollback_shortcut(&mut self, event: &KeyEvent) -> bool {
        if !self.modifiers.shift_key()
            || self.modifiers.control_key()
            || self.modifiers.alt_key()
            || self.modifiers.super_key()
        {
            return false;
        }
        let Some(pane_id) = self.focused_pane() else {
            return false;
        };
        let page = self
            .panes
            .get(&pane_id)
            .map_or(1, |pane| i64::from(pane.frame.rows.saturating_sub(1)));
        let scroll = match event.logical_key {
            Key::Named(NamedKey::PageUp) => TerminalViewportScroll::Delta(-page),
            Key::Named(NamedKey::PageDown) => TerminalViewportScroll::Delta(page),
            Key::Named(NamedKey::Home) => TerminalViewportScroll::Top,
            Key::Named(NamedKey::End) => TerminalViewportScroll::Bottom,
            _ => return false,
        };
        self.scroll_pane(pane_id, scroll);
        true
    }

    fn handle_mouse_wheel(&mut self, delta: MouseScrollDelta) {
        if self.agent_surface.is_some() {
            let direction = match delta {
                MouseScrollDelta::LineDelta(_, vertical) => vertical.signum(),
                MouseScrollDelta::PixelDelta(position) => (position.y as f32).signum(),
            };
            if direction > 0.0 {
                self.scroll_agent_timeline(3);
            } else if direction < 0.0 {
                self.scroll_agent_timeline(-3);
            }
            return;
        }
        let Some(renderer) = &self.renderer else {
            return;
        };
        let scale = renderer.window_scale_factor();
        let x = self.cursor_position.0 / scale;
        let y = self.cursor_position.1 / scale;
        let pane_id = self
            .geometry
            .panes
            .iter()
            .find(|pane| pane.rect.contains(x, y))
            .map(|pane| pane.pane_id);
        let Some(pane_id) = pane_id else {
            return;
        };
        let rows = match delta {
            MouseScrollDelta::LineDelta(_, vertical) => -vertical * 3.0,
            MouseScrollDelta::PixelDelta(position) => {
                -(position.y as f32) / renderer.terminal_cell_height()
            }
        };
        let accumulator = self.scroll_accumulators.entry(pane_id).or_default();
        *accumulator += rows;
        let whole_rows = accumulator.trunc() as i64;
        *accumulator -= whole_rows as f32;
        if whole_rows != 0
            && (self.modifiers.shift_key() || !self.report_mouse_wheel(pane_id, whole_rows))
        {
            self.scroll_pane(pane_id, TerminalViewportScroll::Delta(whole_rows));
        }
    }

    fn handle_mouse_button(&mut self, state: ElementState, button: MouseButton) {
        if self.handle_agent_surface_mouse_button(state, button) {
            return;
        }
        if self.session_switcher.is_some() {
            return;
        }
        if button == MouseButton::Left {
            if state == ElementState::Released && self.hyperlink_click_active {
                self.hyperlink_click_active = false;
                return;
            }
            if state == ElementState::Pressed
                && self.hyperlink_modifier_held()
                && let Some((_, uri)) = self.hyperlink_at_cursor()
            {
                self.hyperlink_click_active = true;
                if let Err(error) = self.cancel_selection_gesture() {
                    self.message = Some(error.to_string());
                }
                if let Err(error) = open_hyperlink(&uri) {
                    self.message = Some(format!("open hyperlink: {error:#}"));
                    let _ = self.refresh_view();
                }
                return;
            }
        }
        match state {
            ElementState::Pressed => {
                self.pressed_mouse_buttons.insert(button);
            }
            ElementState::Released => {
                self.pressed_mouse_buttons.remove(&button);
            }
        }
        let pane_id = if state == ElementState::Released {
            self.mouse_reporting_pane
                .or_else(|| self.pane_at_cursor().map(|pane| pane.pane_id))
        } else {
            self.pane_at_cursor().map(|pane| pane.pane_id)
        };
        let selecting = self.selection_drag.is_some()
            || (button == MouseButton::Left && self.modifiers.shift_key());
        let reported = !selecting
            && pane_id.is_some_and(|pane_id| {
                self.report_mouse_event(
                    pane_id,
                    match state {
                        ElementState::Pressed => TerminalMouseAction::Press,
                        ElementState::Released => TerminalMouseAction::Release,
                    },
                    terminal_mouse_button(button),
                )
            });
        if reported {
            if state == ElementState::Pressed {
                self.mouse_reporting_pane = pane_id;
                if let Err(error) = self.cancel_selection_gesture() {
                    self.message = Some(error.to_string());
                }
                if let Err(error) = self.clear_selected_pane() {
                    self.message = Some(error.to_string());
                }
                if let Some(pane) = self.pane_at_cursor()
                    && !pane.focused
                {
                    self.send_workspace(WorkspaceCommand::SetFocusedPane(pane.pane_id));
                }
            } else if self.pressed_mouse_buttons.is_empty() {
                self.mouse_reporting_pane = None;
            }
            return;
        }

        match (state, button) {
            (ElementState::Pressed, MouseButton::Left) => self.mouse_pressed(),
            (ElementState::Released, MouseButton::Left) => self.mouse_released(),
            _ => {}
        }
    }

    fn scroll_agent_timeline(&mut self, delta: isize) {
        let Some(surface) = &mut self.agent_surface else {
            return;
        };
        if surface.launcher.is_some() {
            return;
        }
        let maximum = self
            .agents
            .get(surface.selected)
            .map_or(0, |agent| agent.timeline.len().saturating_sub(1));
        surface.timeline_scroll = if delta == isize::MAX {
            maximum
        } else if delta == isize::MIN {
            0
        } else if delta.is_positive() {
            surface
                .timeline_scroll
                .saturating_add(delta.unsigned_abs())
                .min(maximum)
        } else {
            surface.timeline_scroll.saturating_sub(delta.unsigned_abs())
        };
        let _ = self.refresh_view();
    }

    #[allow(clippy::too_many_lines)]
    fn handle_agent_surface_mouse_button(
        &mut self,
        state: ElementState,
        button: MouseButton,
    ) -> bool {
        let command_suggestions = self.agent_command_suggestions();
        let Some(surface) = &self.agent_surface else {
            return false;
        };
        if state != ElementState::Pressed || button != MouseButton::Left {
            return true;
        }
        let Some(renderer) = &self.renderer else {
            return true;
        };
        let scale = renderer.window_scale_factor();
        let window_width = renderer.width() as f32;
        let window_height = renderer.height() as f32;
        let panel_width = (480.0 * scale).min(window_width * 0.62);
        let panel_x = window_width - panel_width;
        let panel_y = layout::TAB_BAR_HEIGHT * scale;
        let panel_height = window_height - panel_y;
        let (x, y) = self.cursor_position;

        if x < panel_x {
            self.close_agent_surface();
            return true;
        }
        if y >= panel_y + 7.0 * scale
            && y < panel_y + 37.0 * scale
            && x >= window_width - 112.0 * scale
        {
            self.close_agent_surface();
            return true;
        }

        if surface.launcher.is_some() {
            let notice_offset = if self.message.is_some() { 30.0 } else { 0.0 };
            let profile_rows_y = panel_y + (104.0 + notice_offset) * scale;
            if x >= panel_x + 18.0 * scale && x < window_width - 18.0 * scale {
                for index in 0..self.agent_profiles.len().min(5) {
                    let profile_y = profile_rows_y + index as f32 * 68.0 * scale;
                    if y >= profile_y && y < profile_y + 58.0 * scale {
                        if let Some(surface) = &mut self.agent_surface
                            && let Some(launcher) = &mut surface.launcher
                        {
                            launcher.selected_profile = index;
                        }
                        let _ = self.refresh_view();
                        return true;
                    }
                }
            }
            let footer_y = panel_y + panel_height - 92.0 * scale;
            if x >= window_width - 94.0 * scale
                && x < window_width - 30.0 * scale
                && y >= footer_y + 34.0 * scale
                && y < footer_y + 64.0 * scale
            {
                self.submit_agent_surface();
            }
            return true;
        }

        let Some(agent) = self.agents.get(surface.selected) else {
            return true;
        };
        if let Some(permission) = agent.pending_permission() {
            let composer_height = (58.0 + permission.options.len().min(4) as f32 * 26.0) * scale;
            let composer_y = panel_y + panel_height - composer_height - 16.0 * scale;
            if x >= panel_x + 18.0 * scale && x < window_width - 18.0 * scale {
                for (index, option) in permission.options.iter().take(4).enumerate() {
                    let option_y = composer_y + (32.0 + index as f32 * 26.0) * scale;
                    if y >= option_y && y < option_y + 25.0 * scale {
                        if let Some(backend) = &self.backend {
                            backend.send(CommandMessage::ResolveAgentPermission {
                                session_id: agent.id,
                                request_id: permission.request_id.clone(),
                                option_id: Some(option.id.clone()),
                            });
                        }
                        return true;
                    }
                }
            }
        } else {
            let composer_height = agent_composer_height(&surface.draft) * scale;
            let composer_y = panel_y + panel_height - composer_height - 16.0 * scale;
            if !command_suggestions.is_empty()
                && x >= panel_x + 18.0 * scale
                && x < window_width - 18.0 * scale
            {
                let palette_height = (28.0 + command_suggestions.len() as f32 * 30.0) * scale;
                let palette_y = composer_y - palette_height - 8.0 * scale;
                for (index, command) in command_suggestions.iter().enumerate() {
                    let row_y = palette_y + (25.0 + index as f32 * 30.0) * scale;
                    if y >= row_y && y < row_y + 27.0 * scale {
                        if let Some(surface) = &mut self.agent_surface {
                            surface.draft = format!("/{} ", command.name);
                            surface.command_selection = 0;
                        }
                        let _ = self.refresh_view();
                        return true;
                    }
                }
            }
            if x >= window_width - 91.0 * scale
                && x < window_width - 30.0 * scale
                && y >= composer_y + 26.0 * scale
                && y < composer_y + 57.0 * scale
            {
                self.submit_agent_surface();
            }
        }
        true
    }

    fn handle_cursor_moved(&mut self, physical_x: f32, physical_y: f32) {
        self.cursor_position = (physical_x, physical_y);
        let pane_id = self
            .mouse_reporting_pane
            .or_else(|| self.pane_at_cursor().map(|pane| pane.pane_id));
        let button = self
            .pressed_mouse_buttons
            .iter()
            .find_map(|button| terminal_mouse_button(*button));
        if self.selection_drag.is_none()
            && pane_id.is_some_and(|pane_id| {
                self.report_mouse_event(pane_id, TerminalMouseAction::Motion, button)
            })
        {
            return;
        }
        self.mouse_dragged();
        if self.update_hyperlink_hover() {
            let _ = self.refresh_view();
        }
    }

    fn report_mouse_wheel(&mut self, pane_id: PaneId, rows: i64) -> bool {
        let button = if rows < 0 {
            TerminalMouseButton::Four
        } else {
            TerminalMouseButton::Five
        };
        let mut bytes = Vec::new();
        for _ in 0..rows.unsigned_abs() {
            match self.encode_mouse_event(pane_id, TerminalMouseAction::Press, Some(button)) {
                Ok(encoded) => bytes.extend(encoded),
                Err(error) => {
                    self.message = Some(format!("encode terminal mouse wheel: {error:#}"));
                    let _ = self.refresh_view();
                    return true;
                }
            }
        }
        if bytes.is_empty() {
            false
        } else {
            if let Err(error) = self.cancel_selection_gesture() {
                self.message = Some(error.to_string());
            }
            if let Err(error) = self.clear_selected_pane() {
                self.message = Some(error.to_string());
            }
            self.write_pane(pane_id, bytes);
            true
        }
    }

    fn report_mouse_event(
        &mut self,
        pane_id: PaneId,
        action: TerminalMouseAction,
        button: Option<TerminalMouseButton>,
    ) -> bool {
        if self.modifiers.shift_key()
            && (!self.pressed_mouse_buttons.is_empty() || self.selection_drag.is_some())
        {
            return false;
        }
        match self.encode_mouse_event(pane_id, action, button) {
            Ok(bytes) if bytes.is_empty() => false,
            Ok(bytes) => {
                self.write_pane(pane_id, bytes);
                true
            }
            Err(error) => {
                self.message = Some(format!("encode terminal mouse: {error:#}"));
                let _ = self.refresh_view();
                true
            }
        }
    }

    fn encode_mouse_event(
        &mut self,
        pane_id: PaneId,
        action: TerminalMouseAction,
        button: Option<TerminalMouseButton>,
    ) -> Result<Vec<u8>> {
        let renderer = self
            .renderer
            .as_ref()
            .ok_or_else(|| anyhow!("terminal renderer is unavailable"))?;
        let pane_geometry = self
            .geometry
            .panes
            .iter()
            .find(|pane| pane.pane_id == pane_id)
            .copied()
            .ok_or_else(|| anyhow!("terminal pane geometry is unavailable"))?;
        let (geometry, x, y) = renderer.terminal_mouse_geometry(
            pane_geometry,
            self.cursor_position.0,
            self.cursor_position.1,
        );
        self.panes
            .get_mut(&pane_id)
            .ok_or_else(|| anyhow!("terminal pane is unavailable"))?
            .engine
            .encode_mouse(&TerminalMouseEvent {
                action,
                button,
                modifiers: terminal_modifiers(self.modifiers),
                x,
                y,
                geometry,
                any_button_pressed: !self.pressed_mouse_buttons.is_empty(),
            })
            .map_err(Into::into)
    }

    fn pane_at_cursor(&self) -> Option<layout::PaneGeometry> {
        let renderer = self.renderer.as_ref()?;
        let scale = renderer.window_scale_factor();
        let x = self.cursor_position.0 / scale;
        let y = self.cursor_position.1 / scale;
        self.geometry
            .panes
            .iter()
            .find(|pane| pane.rect.contains(x, y))
            .copied()
    }

    fn hyperlink_modifier_held(&self) -> bool {
        if cfg!(target_os = "macos") {
            self.modifiers.super_key()
        } else {
            self.modifiers.control_key()
        }
    }

    fn hyperlink_at_cursor(&self) -> Option<(PaneId, String)> {
        if self.mode != InputMode::Normal
            || self.text_prompt.is_some()
            || self.session_switcher.is_some()
            || self.agent_surface.is_some()
            || !self.pressed_mouse_buttons.is_empty()
        {
            return None;
        }
        let renderer = self.renderer.as_ref()?;
        let pane = self.pane_at_cursor()?;
        let point =
            renderer.terminal_point_at(pane, self.cursor_position.0, self.cursor_position.1)?;
        let frame = &self.panes.get(&pane.pane_id)?.frame;
        let index = usize::from(point.row)
            .checked_mul(usize::from(frame.cols))?
            .checked_add(usize::from(point.column))?;
        frame
            .cells
            .get(index)?
            .hyperlink
            .as_ref()
            .map(|uri| (pane.pane_id, uri.clone()))
    }

    fn update_hyperlink_hover(&mut self) -> bool {
        let next = self
            .hyperlink_modifier_held()
            .then(|| self.hyperlink_at_cursor())
            .flatten();
        if next == self.hovered_hyperlink {
            return false;
        }
        self.hovered_hyperlink = next;
        if let Some(renderer) = &self.renderer {
            renderer.set_cursor_icon(if self.hovered_hyperlink.is_some() {
                CursorIcon::Pointer
            } else {
                CursorIcon::Default
            });
        }
        true
    }

    fn clear_hyperlink_hover(&mut self) -> bool {
        if self.hovered_hyperlink.take().is_none() {
            return false;
        }
        if let Some(renderer) = &self.renderer {
            renderer.set_cursor_icon(CursorIcon::Default);
        }
        true
    }

    fn scroll_pane(&mut self, pane_id: PaneId, scroll: TerminalViewportScroll) {
        let result = (|| -> Result<()> {
            let pane = self
                .panes
                .get_mut(&pane_id)
                .ok_or_else(|| anyhow!("terminal pane is unavailable"))?;
            pane.engine.scroll_viewport(scroll)?;
            pane.engine.render_frame_into(&mut pane.frame)?;
            self.sync_view(&HashSet::from([pane_id]))?;
            self.request_redraw();
            Ok(())
        })();
        if let Err(error) = result {
            self.message = Some(format!("scroll terminal: {error:#}"));
            let _ = self.refresh_view();
        }
    }

    fn handle_clipboard_shortcut(&mut self, event: &KeyEvent) -> bool {
        let Key::Character(value) = &event.logical_key else {
            return false;
        };
        let platform_shortcut = self.modifiers.super_key()
            || (self.modifiers.control_key() && self.modifiers.shift_key());
        if !platform_shortcut {
            return false;
        }
        match value.to_lowercase().as_str() {
            "c" => {
                self.copy_selection();
                true
            }
            "v" => {
                self.paste_clipboard();
                true
            }
            _ => false,
        }
    }

    fn copy_selection(&mut self) {
        let result = (|| -> Result<()> {
            let pane_id = self
                .selected_pane
                .or_else(|| self.focused_pane())
                .ok_or_else(|| anyhow!("no terminal pane is selected"))?;
            let text = self
                .panes
                .get(&pane_id)
                .ok_or_else(|| anyhow!("selected pane is unavailable"))?
                .engine
                .selected_text()?
                .ok_or_else(|| anyhow!("no terminal text is selected"))?;
            self.clipboard
                .as_mut()
                .ok_or_else(|| anyhow!("system clipboard is unavailable"))?
                .set_text(text)
                .context("copy terminal selection")
        })();
        match result {
            Ok(()) => self.message = None,
            Err(error) => self.message = Some(format!("{error:#}")),
        }
        let _ = self.refresh_view();
    }

    fn paste_clipboard(&mut self) {
        let result = (|| -> Result<Vec<u8>> {
            let text = self
                .clipboard
                .as_mut()
                .ok_or_else(|| anyhow!("system clipboard is unavailable"))?
                .get_text()
                .context("read system clipboard")?;
            let pane_id = self
                .focused_pane()
                .ok_or_else(|| anyhow!("no focused terminal pane"))?;
            self.panes
                .get(&pane_id)
                .ok_or_else(|| anyhow!("focused pane is unavailable"))?
                .engine
                .encode_paste(&text)
                .map_err(Into::into)
        })();
        match result {
            Ok(bytes) => {
                self.message = None;
                self.prepare_terminal_input();
                self.write_focused(bytes);
                let _ = self.refresh_view();
            }
            Err(error) => {
                self.message = Some(format!("{error:#}"));
                let _ = self.refresh_view();
            }
        }
    }

    fn execute_action(&mut self, action: Action) {
        match action {
            Action::Sequence(actions) => {
                for action in actions {
                    self.execute_action(action);
                }
            }
            Action::EnterMode(mode) => {
                self.mode = mode;
                if let Err(error) = self.refresh_view() {
                    self.message = Some(error.to_string());
                }
            }
            Action::SplitPane(axis) => self.send_workspace(WorkspaceCommand::SplitPane(axis)),
            Action::FocusPane(direction) => {
                self.send_workspace(WorkspaceCommand::FocusPane(direction));
            }
            Action::FocusPaneOrTab(direction) => {
                self.send_workspace(WorkspaceCommand::FocusPaneOrTab(direction));
            }
            Action::ResizePane(direction) => {
                self.send_workspace(WorkspaceCommand::ResizePane(direction));
            }
            Action::ClosePane => self.send_workspace(WorkspaceCommand::ClosePane),
            Action::TogglePaneZoom => self.send_workspace(WorkspaceCommand::TogglePaneZoom),
            Action::NewTab => self.send_workspace(WorkspaceCommand::NewTab),
            Action::CloseTab => self.send_workspace(WorkspaceCommand::CloseTab),
            Action::NextTab => self.send_workspace(WorkspaceCommand::NextTab),
            Action::PreviousTab => self.send_workspace(WorkspaceCommand::PreviousTab),
            Action::SelectTab(number) => {
                if let Some(tab) = self
                    .session
                    .as_ref()
                    .and_then(|session| session.tabs.get(usize::from(number.saturating_sub(1))))
                {
                    self.send_workspace(WorkspaceCommand::SelectTab(tab.id));
                }
            }
            Action::WriteTerminal(bytes) => {
                self.prepare_terminal_input();
                self.write_focused(bytes);
            }
            Action::OpenSessionSwitcher => {
                self.mode = InputMode::Normal;
                self.clear_hyperlink_hover();
                self.session_switcher = Some(SessionSwitcher {
                    entries: Vec::new(),
                    selected: 0,
                    pending_kill: None,
                });
                if let Some(backend) = &self.backend {
                    backend.send(CommandMessage::ListSessions);
                }
                let _ = self.refresh_view();
            }
            Action::DetachSession => {
                if let Some(proxy) = &self.event_proxy {
                    let _ = proxy.send_event(UserEvent::ExitRequested);
                }
            }
            Action::OpenAgentSurface => self.toggle_agent_surface(),
            Action::RenameTab => {
                self.clear_hyperlink_hover();
                self.text_prompt = Some(TextPrompt {
                    kind: TextPromptKind::RenameTab,
                    draft: String::new(),
                });
                let _ = self.refresh_view();
            }
            Action::OpenCommandPalette => {}
        }
    }

    fn write_focused(&self, bytes: Vec<u8>) {
        if let Some(backend) = &self.backend {
            backend.send(CommandMessage::WriteFocused { bytes });
        }
    }

    fn prepare_terminal_input(&mut self) {
        let cursor_was_hidden = !self.cursor_blink.visible;
        self.cursor_blink.last_reset = Some(Instant::now());
        self.cursor_blink.reset_pending = false;
        self.sync_cursor_blink(true);
        if let Err(error) = self.cancel_selection_gesture() {
            self.message = Some(error.to_string());
        }
        if let Err(error) = self.clear_selected_pane() {
            self.message = Some(error.to_string());
        }
        if let Some(pane_id) = self.focused_pane()
            && self
                .panes
                .get(&pane_id)
                .is_some_and(|pane| pane.frame.scroll.is_scrolled())
        {
            self.scroll_pane(pane_id, TerminalViewportScroll::Bottom);
        } else if cursor_was_hidden {
            let _ = self.refresh_view();
        }
    }

    fn write_pane(&self, pane_id: PaneId, bytes: Vec<u8>) {
        if let Some(backend) = &self.backend {
            backend.send(CommandMessage::Write { pane_id, bytes });
        }
    }

    fn send_workspace(&self, command: WorkspaceCommand) {
        let (Some(backend), Some(session)) = (&self.backend, &self.session) else {
            return;
        };
        backend.send(CommandMessage::Workspace {
            session_id: session.id,
            command,
        });
    }

    fn focused_pane(&self) -> Option<PaneId> {
        self.session
            .as_ref()?
            .active_tab()
            .map(|tab| tab.focused_pane)
    }

    fn set_pane_selection(
        &mut self,
        pane_id: PaneId,
        selection: Option<TerminalSelection>,
    ) -> Result<()> {
        let pane = self
            .panes
            .get_mut(&pane_id)
            .ok_or_else(|| anyhow!("selection pane is unavailable"))?;
        pane.engine.set_selection(selection)?;
        pane.engine.render_frame_into(&mut pane.frame)?;
        if selection.is_some() {
            self.selected_pane = Some(pane_id);
        } else if self.selected_pane == Some(pane_id) {
            self.selected_pane = None;
        }
        self.sync_view(&HashSet::from([pane_id]))?;
        self.request_redraw();
        Ok(())
    }

    fn apply_selection_gesture(
        &mut self,
        pane_id: PaneId,
        event: TerminalSelectionGestureEvent,
    ) -> Result<TerminalSelectionGestureStatus> {
        let status = {
            let pane = self
                .panes
                .get_mut(&pane_id)
                .ok_or_else(|| anyhow!("selection pane is unavailable"))?;
            let status = pane.engine.selection_gesture(event)?;
            pane.engine.render_frame_into(&mut pane.frame)?;
            status
        };
        self.selected_pane = status.has_selection.then_some(pane_id);
        if let Some(drag) = &mut self.selection_drag
            && drag.pane_id == pane_id
        {
            drag.autoscroll = status.autoscroll;
            self.next_selection_scroll = match status.autoscroll {
                TerminalSelectionAutoscroll::None => None,
                TerminalSelectionAutoscroll::Up | TerminalSelectionAutoscroll::Down => self
                    .next_selection_scroll
                    .or_else(|| Some(Instant::now() + Duration::from_millis(15))),
            };
        }
        self.sync_view(&HashSet::from([pane_id]))?;
        self.request_redraw();
        Ok(status)
    }

    fn cancel_selection_gesture(&mut self) -> Result<()> {
        self.selection_drag = None;
        self.next_selection_scroll = None;
        if let Some(pane_id) = self.selection_gesture_pane.take()
            && let Some(pane) = self.panes.get_mut(&pane_id)
        {
            pane.engine.reset_selection_gesture()?;
        }
        Ok(())
    }

    fn clear_selected_pane(&mut self) -> Result<()> {
        if let Some(pane_id) = self.selected_pane.take()
            && self.panes.contains_key(&pane_id)
        {
            self.set_pane_selection(pane_id, None)?;
        }
        Ok(())
    }

    fn mouse_pressed(&mut self) {
        let Some(renderer) = &self.renderer else {
            return;
        };
        let scale = renderer.window_scale_factor();
        let x = self.cursor_position.0 / scale;
        let y = self.cursor_position.1 / scale;
        if let Some(tab_id) = self
            .geometry
            .tabs
            .iter()
            .find(|tab| tab.rect.contains(x, y))
            .map(|tab| tab.tab_id)
        {
            if let Err(error) = self.cancel_selection_gesture() {
                self.message = Some(error.to_string());
            }
            self.send_workspace(WorkspaceCommand::SelectTab(tab_id));
            return;
        }
        if cfg!(target_os = "macos") && y < layout::TAB_BAR_HEIGHT {
            if let Err(error) = self.cancel_selection_gesture() {
                self.message = Some(error.to_string());
            }
            if let Some(renderer) = &self.renderer
                && let Err(error) = renderer.drag_window()
            {
                self.message = Some(error.to_string());
            }
            return;
        }
        let pane = self
            .geometry
            .panes
            .iter()
            .find(|pane| pane.rect.contains(x, y))
            .copied();
        let Some(pane) = pane else {
            if let Err(error) = self.cancel_selection_gesture() {
                self.message = Some(error.to_string());
            }
            return;
        };
        if !pane.focused {
            self.send_workspace(WorkspaceCommand::SetFocusedPane(pane.pane_id));
        }
        let pointer = renderer.terminal_selection_pointer(
            pane,
            self.cursor_position.0,
            self.cursor_position.1,
        );
        let Some(point) = pointer.point else {
            if let Err(error) = self.cancel_selection_gesture() {
                self.message = Some(error.to_string());
            }
            if let Err(error) = self.clear_selected_pane() {
                self.message = Some(error.to_string());
            }
            return;
        };
        if self.selection_gesture_pane != Some(pane.pane_id)
            && let Err(error) = self.cancel_selection_gesture()
        {
            self.message = Some(error.to_string());
            return;
        }
        if self.selected_pane != Some(pane.pane_id)
            && let Err(error) = self.clear_selected_pane()
        {
            self.message = Some(error.to_string());
            return;
        }
        self.selection_gesture_pane = Some(pane.pane_id);
        self.selection_drag = Some(SelectionDrag {
            pane_id: pane.pane_id,
            autoscroll: TerminalSelectionAutoscroll::None,
        });
        let time_ns = Instant::now()
            .duration_since(self.selection_clock_origin)
            .as_nanos()
            .min(u128::from(u64::MAX)) as u64;
        if let Err(error) = self.apply_selection_gesture(
            pane.pane_id,
            TerminalSelectionGestureEvent::Press {
                point,
                position: pointer.position,
                time_ns,
                repeat_distance: f64::from(pointer.geometry.cell_width),
                repeat_interval_ns: 500_000_000,
            },
        ) {
            self.message = Some(error.to_string());
        }
    }

    fn mouse_dragged(&mut self) {
        let Some(drag) = self.selection_drag else {
            return;
        };
        let Some(renderer) = &self.renderer else {
            return;
        };
        let Some(pane) = self
            .geometry
            .panes
            .iter()
            .find(|pane| pane.pane_id == drag.pane_id)
            .copied()
        else {
            return;
        };
        let pointer = renderer.terminal_selection_pointer(
            pane,
            self.cursor_position.0,
            self.cursor_position.1,
        );
        if let Err(error) = self.apply_selection_gesture(
            drag.pane_id,
            TerminalSelectionGestureEvent::Drag {
                point: pointer.clamped_point,
                position: pointer.position,
                rectangular: self.modifiers.alt_key(),
                geometry: pointer.geometry,
            },
        ) {
            self.message = Some(error.to_string());
        }
    }

    fn selection_autoscroll_tick(&mut self) {
        let Some(drag) = self.selection_drag else {
            self.next_selection_scroll = None;
            return;
        };
        if drag.autoscroll == TerminalSelectionAutoscroll::None {
            self.next_selection_scroll = None;
            return;
        }
        let Some(renderer) = &self.renderer else {
            self.next_selection_scroll = None;
            return;
        };
        let Some(pane) = self
            .geometry
            .panes
            .iter()
            .find(|pane| pane.pane_id == drag.pane_id)
            .copied()
        else {
            self.next_selection_scroll = None;
            return;
        };
        let pointer = renderer.terminal_selection_pointer(
            pane,
            self.cursor_position.0,
            self.cursor_position.1,
        );
        self.next_selection_scroll = None;
        if let Err(error) = self.apply_selection_gesture(
            drag.pane_id,
            TerminalSelectionGestureEvent::AutoscrollTick {
                viewport: pointer.clamped_point,
                position: pointer.position,
                rectangular: self.modifiers.alt_key(),
                geometry: pointer.geometry,
            },
        ) {
            self.message = Some(error.to_string());
            self.next_selection_scroll = None;
        }
    }

    fn mouse_released(&mut self) {
        self.mouse_dragged();
        let Some(drag) = self.selection_drag else {
            return;
        };
        let point = self.renderer.as_ref().and_then(|renderer| {
            self.geometry
                .panes
                .iter()
                .find(|pane| pane.pane_id == drag.pane_id)
                .copied()
                .and_then(|pane| {
                    renderer
                        .terminal_selection_pointer(
                            pane,
                            self.cursor_position.0,
                            self.cursor_position.1,
                        )
                        .point
                })
        });
        if let Err(error) = self.apply_selection_gesture(
            drag.pane_id,
            TerminalSelectionGestureEvent::Release { point },
        ) {
            self.message = Some(error.to_string());
        }
        self.selection_drag = None;
        self.next_selection_scroll = None;
    }
}

impl ApplicationHandler<UserEvent> for Application {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.renderer.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("Mux")
            .with_inner_size(LogicalSize::new(1120.0, 720.0))
            .with_min_inner_size(LogicalSize::new(560.0, 360.0))
            .with_decorations(!cfg!(target_os = "macos"));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                error!(%error, "failed to create native window");
                event_loop.exit();
                return;
            }
        };
        match pollster::block_on(Renderer::new(window)) {
            Ok(renderer) => self.renderer = Some(renderer),
            Err(error) => {
                error!(%error, "failed to initialize renderer");
                event_loop.exit();
                return;
            }
        }
        self.backend = self
            .event_proxy
            .clone()
            .map(|proxy| backend::spawn(proxy, self.state_dir.clone()));
        if let Err(error) = self.refresh_view() {
            self.message = Some(error.to_string());
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        if matches!(event, UserEvent::ExitRequested) {
            event_loop.exit();
            return;
        }
        let result = match event {
            UserEvent::Attached(attachment) => self.attach(attachment),
            UserEvent::Sessions(entries) => {
                let selected = self
                    .session
                    .as_ref()
                    .and_then(|active| entries.iter().position(|entry| entry.id == active.id))
                    .unwrap_or(0);
                self.session_switcher = Some(SessionSwitcher {
                    entries,
                    selected,
                    pending_kill: None,
                });
                self.refresh_view()
            }
            UserEvent::Server(event) => self.apply_server_event(event),
            UserEvent::Agents(agents) => {
                self.replace_agents(agents);
                Ok(())
            }
            UserEvent::AgentStarted(agent) => self.agent_started(agent),
            UserEvent::Agent(event) => {
                self.apply_agent_event(&event);
                Ok(())
            }
            UserEvent::BackendError(message) => self.backend_error(message),
            UserEvent::ExitRequested => unreachable!("handled above"),
        };
        if let Err(error) = result {
            self.message = Some(error.to_string());
            let _ = self.refresh_view();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self
            .next_selection_scroll
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.selection_autoscroll_tick();
        }
        if self.advance_cursor_blink() {
            let _ = self.refresh_view();
        }
        let deadline = earliest_deadline(self.next_selection_scroll, self.cursor_blink.next);
        event_loop.set_control_flow(deadline.map_or(ControlFlow::Wait, ControlFlow::WaitUntil));
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(renderer) = &mut self.renderer {
                    renderer.resize(
                        size.width,
                        size.height,
                        f64::from(renderer.window_scale_factor()),
                    );
                }
                if let Err(error) = self.refresh_view() {
                    self.message = Some(error.to_string());
                }
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                if let Some(renderer) = &mut self.renderer {
                    renderer.resize(renderer.width(), renderer.height(), scale_factor);
                }
                if let Err(error) = self.refresh_view() {
                    self.message = Some(error.to_string());
                }
            }
            WindowEvent::Focused(focused) => {
                self.cursor_blink.window_focused = focused;
                self.sync_cursor_blink(focused);
                let _ = self.refresh_view();
            }
            WindowEvent::RedrawRequested => {
                if self.advance_ui_animation() {
                    self.view_dirty = true;
                    self.request_redraw();
                }
                if let Err(error) = self.flush_terminal_frames() {
                    self.message = Some(error.to_string());
                    let _ = self.sync_view(&HashSet::new());
                }
                if let Some(renderer) = &mut self.renderer {
                    if let Err(error) = renderer.draw() {
                        self.message = Some(error.to_string());
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => self.handle_key(&event),
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
                if self.update_hyperlink_hover() {
                    let _ = self.refresh_view();
                }
            }
            WindowEvent::Ime(Ime::Preedit(text, _)) => {
                if !text.is_empty()
                    && self.text_prompt.is_none()
                    && self.agent_surface.is_none()
                    && self.mode == InputMode::Normal
                {
                    self.prepare_terminal_input();
                }
                self.ime_preedit = text;
                let _ = self.refresh_view();
            }
            WindowEvent::Ime(Ime::Commit(text)) if self.text_prompt.is_some() => {
                self.ime_preedit.clear();
                if let Some(prompt) = &mut self.text_prompt {
                    prompt.draft.push_str(&text);
                }
                let _ = self.refresh_view();
            }
            WindowEvent::Ime(Ime::Commit(text)) if self.agent_surface.is_some() => {
                self.ime_preedit.clear();
                if let Some(surface) = &mut self.agent_surface {
                    surface.draft.push_str(&text);
                    surface.command_selection = 0;
                }
                let _ = self.refresh_view();
            }
            WindowEvent::Ime(Ime::Commit(text)) if self.mode == InputMode::Normal => {
                self.ime_preedit.clear();
                self.prepare_terminal_input();
                self.write_focused(text.into_bytes());
                let _ = self.refresh_view();
            }
            WindowEvent::Ime(Ime::Commit(_) | Ime::Disabled) => {
                self.ime_preedit.clear();
                let _ = self.refresh_view();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.handle_cursor_moved(position.x as f32, position.y as f32);
            }
            WindowEvent::CursorLeft { .. } => {
                if self.clear_hyperlink_hover() {
                    let _ = self.refresh_view();
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                self.handle_mouse_button(state, button);
            }
            WindowEvent::MouseWheel { delta, .. } => self.handle_mouse_wheel(delta),
            _ => {}
        }
    }
}

fn earliest_deadline(first: Option<Instant>, second: Option<Instant>) -> Option<Instant> {
    match (first, second) {
        (Some(first), Some(second)) => Some(first.min(second)),
        (Some(deadline), None) | (None, Some(deadline)) => Some(deadline),
        (None, None) => None,
    }
}

fn open_hyperlink(uri: &str) -> Result<()> {
    validate_hyperlink_uri(uri)?;

    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("/usr/bin/open");
    #[cfg(target_os = "linux")]
    let mut command = std::process::Command::new("xdg-open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("rundll32");
        command.arg("url.dll,FileProtocolHandler");
        command
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    return Err(anyhow!(
        "opening hyperlinks is unsupported on this platform"
    ));

    let mut child = command.arg(uri).spawn().context("launch URI handler")?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

fn validate_hyperlink_uri(uri: &str) -> Result<()> {
    let scheme = uri
        .split_once(':')
        .map(|(scheme, _)| scheme)
        .filter(|scheme| {
            let mut characters = scheme.chars();
            characters
                .next()
                .is_some_and(|first| first.is_ascii_alphabetic())
                && characters.all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.')
                })
        })
        .ok_or_else(|| anyhow!("terminal hyperlink is not an absolute URI"))?;
    if scheme.len() > 32 || uri.chars().any(char::is_control) {
        return Err(anyhow!("terminal hyperlink is not a safe URI"));
    }
    Ok(())
}

fn expand_home_path(value: &str) -> PathBuf {
    if value == "~" {
        return directories::BaseDirs::new().map_or_else(
            || PathBuf::from(value),
            |directories| directories.home_dir().to_path_buf(),
        );
    }
    if let Some(relative) = value.strip_prefix("~/")
        && let Some(directories) = directories::BaseDirs::new()
    {
        return directories.home_dir().join(relative);
    }
    PathBuf::from(value)
}

fn pop_grapheme(value: &mut String) {
    if let Some((start, _)) = value.grapheme_indices(true).next_back() {
        value.truncate(start);
    }
}

fn describe_agent_option(option: &mux_acp::AgentConfigOption) -> String {
    match &option.value {
        AgentConfigValue::Select { current, choices } => format!(
            "{} {} · {}",
            option.name,
            current,
            choices
                .iter()
                .map(|choice| choice.id.as_str())
                .collect::<Vec<_>>()
                .join(" · ")
        ),
        AgentConfigValue::Boolean(current) => {
            format!(
                "{} {} · on · off",
                option.name,
                if *current { "on" } else { "off" }
            )
        }
    }
}

fn terminal_frame_text(frame: &RenderFrame) -> String {
    let columns = usize::from(frame.cols);
    let mut text = String::new();
    for row in 0..usize::from(frame.rows) {
        let start = row * columns;
        let end = start + columns;
        let mut line = String::new();
        for cell in &frame.cells[start..end] {
            if cell.width == CellWidth::SpacerTail {
                continue;
            }
            if cell.style.invisible || cell.grapheme.is_empty() {
                line.push(' ');
            } else {
                line.push_str(&cell.grapheme);
            }
        }
        text.push_str(line.trim_end());
        if !frame
            .row_metadata
            .get(row)
            .is_some_and(|metadata| metadata.wrapped)
        {
            text.push('\n');
        }
    }
    text.trim_end_matches('\n').to_owned()
}

fn is_agent_surface_shortcut(event: &KeyEvent, modifiers: ModifiersState) -> bool {
    modifiers.super_key()
        && modifiers.shift_key()
        && matches!(
            event.key_without_modifiers(),
            Key::Character(value) if value.eq_ignore_ascii_case("a")
        )
}

fn key_chord(event: &KeyEvent, modifiers: ModifiersState) -> Option<KeyChord> {
    let logical_key = event.key_without_modifiers();
    let key = &logical_key;
    let key = match key {
        Key::Character(value) => MuxKey::Character(value.chars().next()?.to_ascii_lowercase()),
        Key::Named(NamedKey::Escape) => MuxKey::Escape,
        Key::Named(NamedKey::Enter) => MuxKey::Enter,
        Key::Named(NamedKey::Tab) => MuxKey::Tab,
        Key::Named(NamedKey::Backspace) => MuxKey::Backspace,
        Key::Named(NamedKey::ArrowLeft) => MuxKey::ArrowLeft,
        Key::Named(NamedKey::ArrowRight) => MuxKey::ArrowRight,
        Key::Named(NamedKey::ArrowUp) => MuxKey::ArrowUp,
        Key::Named(NamedKey::ArrowDown) => MuxKey::ArrowDown,
        _ => return None,
    };
    Some(KeyChord {
        key,
        modifiers: mux_modifiers(modifiers),
    })
}

fn mux_modifiers(modifiers: ModifiersState) -> Modifiers {
    let mut result = Modifiers::EMPTY;
    if modifiers.control_key() {
        result = result.union(Modifiers::CONTROL);
    }
    if modifiers.alt_key() {
        result = result.union(Modifiers::ALT);
    }
    if modifiers.shift_key() {
        result = result.union(Modifiers::SHIFT);
    }
    if modifiers.super_key() {
        result = result.union(Modifiers::SUPER);
    }
    result
}

fn terminal_key_event(event: &KeyEvent, modifiers: ModifiersState) -> TerminalKeyEvent {
    let key = terminal_key(event);
    TerminalKeyEvent {
        action: match event.state {
            ElementState::Released => TerminalKeyAction::Release,
            ElementState::Pressed if event.repeat => TerminalKeyAction::Repeat,
            ElementState::Pressed => TerminalKeyAction::Press,
        },
        key,
        modifiers: terminal_modifiers(modifiers),
        consumed_modifiers: consumed_terminal_modifiers(event, modifiers),
        text: event
            .text
            .as_deref()
            .filter(|text| terminal_text_is_usable(text))
            .map(str::to_owned),
        unshifted_codepoint: unshifted_codepoint(event, key),
        composing: false,
    }
}

fn consumed_terminal_modifiers(event: &KeyEvent, modifiers: ModifiersState) -> TerminalModifiers {
    let shifted_text = event
        .text
        .as_deref()
        .filter(|text| terminal_text_is_usable(text))
        .zip(match event.key_without_modifiers() {
            Key::Character(text) => Some(text),
            _ => None,
        })
        .is_some_and(|(text, unmodified)| text != unmodified.as_str());
    TerminalModifiers {
        shift: modifiers.shift_key() && shifted_text,
        ..TerminalModifiers::default()
    }
}

fn terminal_modifiers(modifiers: ModifiersState) -> TerminalModifiers {
    TerminalModifiers {
        shift: modifiers.shift_key(),
        control: modifiers.control_key(),
        alt: modifiers.alt_key(),
        super_key: modifiers.super_key(),
    }
}

const fn terminal_mouse_button(button: MouseButton) -> Option<TerminalMouseButton> {
    match button {
        MouseButton::Left => Some(TerminalMouseButton::Left),
        MouseButton::Right => Some(TerminalMouseButton::Right),
        MouseButton::Middle => Some(TerminalMouseButton::Middle),
        MouseButton::Back | MouseButton::Other(8) => Some(TerminalMouseButton::Eight),
        MouseButton::Forward | MouseButton::Other(9) => Some(TerminalMouseButton::Nine),
        MouseButton::Other(4) => Some(TerminalMouseButton::Four),
        MouseButton::Other(5) => Some(TerminalMouseButton::Five),
        MouseButton::Other(6) => Some(TerminalMouseButton::Six),
        MouseButton::Other(7) => Some(TerminalMouseButton::Seven),
        MouseButton::Other(10) => Some(TerminalMouseButton::Ten),
        MouseButton::Other(11) => Some(TerminalMouseButton::Eleven),
        MouseButton::Other(_) => None,
    }
}

fn terminal_key(event: &KeyEvent) -> TerminalKey {
    if let PhysicalKey::Code(code) = event.physical_key
        && let Some(key) = physical_terminal_key(code)
    {
        return key;
    }

    match event.logical_key {
        Key::Named(NamedKey::Backspace) => TerminalKey::Backspace,
        Key::Named(NamedKey::Enter) => TerminalKey::Enter,
        Key::Named(NamedKey::Tab) => TerminalKey::Tab,
        Key::Named(NamedKey::Delete) => TerminalKey::Delete,
        Key::Named(NamedKey::Insert) => TerminalKey::Insert,
        Key::Named(NamedKey::Home) => TerminalKey::Home,
        Key::Named(NamedKey::End) => TerminalKey::End,
        Key::Named(NamedKey::PageUp) => TerminalKey::PageUp,
        Key::Named(NamedKey::PageDown) => TerminalKey::PageDown,
        Key::Named(NamedKey::ArrowUp) => TerminalKey::ArrowUp,
        Key::Named(NamedKey::ArrowDown) => TerminalKey::ArrowDown,
        Key::Named(NamedKey::ArrowLeft) => TerminalKey::ArrowLeft,
        Key::Named(NamedKey::ArrowRight) => TerminalKey::ArrowRight,
        Key::Named(NamedKey::Escape) => TerminalKey::Escape,
        Key::Character(ref text) if text.as_str() == " " => TerminalKey::Space,
        _ => TerminalKey::Unidentified,
    }
}

const fn physical_terminal_key(code: KeyCode) -> Option<TerminalKey> {
    if let Some(key) = writing_system_key(code) {
        return Some(key);
    }
    if let Some(number) = function_key_number(code) {
        return Some(TerminalKey::Function(number));
    }
    Some(match code {
        KeyCode::Backspace => TerminalKey::Backspace,
        KeyCode::Enter => TerminalKey::Enter,
        KeyCode::Tab => TerminalKey::Tab,
        KeyCode::Space => TerminalKey::Space,
        KeyCode::Delete => TerminalKey::Delete,
        KeyCode::Insert => TerminalKey::Insert,
        KeyCode::Home => TerminalKey::Home,
        KeyCode::End => TerminalKey::End,
        KeyCode::PageUp => TerminalKey::PageUp,
        KeyCode::PageDown => TerminalKey::PageDown,
        KeyCode::ArrowUp => TerminalKey::ArrowUp,
        KeyCode::ArrowDown => TerminalKey::ArrowDown,
        KeyCode::ArrowLeft => TerminalKey::ArrowLeft,
        KeyCode::ArrowRight => TerminalKey::ArrowRight,
        KeyCode::Escape => TerminalKey::Escape,
        KeyCode::NumpadEnter => TerminalKey::NumpadEnter,
        _ => return None,
    })
}

const fn writing_system_key(code: KeyCode) -> Option<TerminalKey> {
    Some(match code {
        KeyCode::Backquote => TerminalKey::Backquote,
        KeyCode::Backslash => TerminalKey::Backslash,
        KeyCode::BracketLeft => TerminalKey::BracketLeft,
        KeyCode::BracketRight => TerminalKey::BracketRight,
        KeyCode::Comma => TerminalKey::Comma,
        KeyCode::Digit0 => TerminalKey::Digit(0),
        KeyCode::Digit1 => TerminalKey::Digit(1),
        KeyCode::Digit2 => TerminalKey::Digit(2),
        KeyCode::Digit3 => TerminalKey::Digit(3),
        KeyCode::Digit4 => TerminalKey::Digit(4),
        KeyCode::Digit5 => TerminalKey::Digit(5),
        KeyCode::Digit6 => TerminalKey::Digit(6),
        KeyCode::Digit7 => TerminalKey::Digit(7),
        KeyCode::Digit8 => TerminalKey::Digit(8),
        KeyCode::Digit9 => TerminalKey::Digit(9),
        KeyCode::Equal => TerminalKey::Equal,
        KeyCode::IntlBackslash => TerminalKey::IntlBackslash,
        KeyCode::IntlRo => TerminalKey::IntlRo,
        KeyCode::IntlYen => TerminalKey::IntlYen,
        KeyCode::KeyA => TerminalKey::Letter('a'),
        KeyCode::KeyB => TerminalKey::Letter('b'),
        KeyCode::KeyC => TerminalKey::Letter('c'),
        KeyCode::KeyD => TerminalKey::Letter('d'),
        KeyCode::KeyE => TerminalKey::Letter('e'),
        KeyCode::KeyF => TerminalKey::Letter('f'),
        KeyCode::KeyG => TerminalKey::Letter('g'),
        KeyCode::KeyH => TerminalKey::Letter('h'),
        KeyCode::KeyI => TerminalKey::Letter('i'),
        KeyCode::KeyJ => TerminalKey::Letter('j'),
        KeyCode::KeyK => TerminalKey::Letter('k'),
        KeyCode::KeyL => TerminalKey::Letter('l'),
        KeyCode::KeyM => TerminalKey::Letter('m'),
        KeyCode::KeyN => TerminalKey::Letter('n'),
        KeyCode::KeyO => TerminalKey::Letter('o'),
        KeyCode::KeyP => TerminalKey::Letter('p'),
        KeyCode::KeyQ => TerminalKey::Letter('q'),
        KeyCode::KeyR => TerminalKey::Letter('r'),
        KeyCode::KeyS => TerminalKey::Letter('s'),
        KeyCode::KeyT => TerminalKey::Letter('t'),
        KeyCode::KeyU => TerminalKey::Letter('u'),
        KeyCode::KeyV => TerminalKey::Letter('v'),
        KeyCode::KeyW => TerminalKey::Letter('w'),
        KeyCode::KeyX => TerminalKey::Letter('x'),
        KeyCode::KeyY => TerminalKey::Letter('y'),
        KeyCode::KeyZ => TerminalKey::Letter('z'),
        KeyCode::Minus => TerminalKey::Minus,
        KeyCode::Period => TerminalKey::Period,
        KeyCode::Quote => TerminalKey::Quote,
        KeyCode::Semicolon => TerminalKey::Semicolon,
        KeyCode::Slash => TerminalKey::Slash,
        _ => return None,
    })
}

fn unshifted_codepoint(event: &KeyEvent, key: TerminalKey) -> Option<char> {
    match key {
        TerminalKey::Backspace => Some('\u{8}'),
        TerminalKey::Enter | TerminalKey::NumpadEnter => Some('\r'),
        TerminalKey::Tab => Some('\t'),
        TerminalKey::Space => Some(' '),
        TerminalKey::Escape => Some('\u{1b}'),
        _ => match event.key_without_modifiers() {
            Key::Character(text) => text.chars().next(),
            _ => None,
        },
    }
}

fn terminal_text_is_usable(text: &str) -> bool {
    !text.is_empty()
        && !text.chars().any(|character| {
            character.is_control() || ('\u{f700}'..='\u{f8ff}').contains(&character)
        })
}

const fn function_key_number(code: KeyCode) -> Option<u8> {
    match code {
        KeyCode::F1 => Some(1),
        KeyCode::F2 => Some(2),
        KeyCode::F3 => Some(3),
        KeyCode::F4 => Some(4),
        KeyCode::F5 => Some(5),
        KeyCode::F6 => Some(6),
        KeyCode::F7 => Some(7),
        KeyCode::F8 => Some(8),
        KeyCode::F9 => Some(9),
        KeyCode::F10 => Some(10),
        KeyCode::F11 => Some(11),
        KeyCode::F12 => Some(12),
        KeyCode::F13 => Some(13),
        KeyCode::F14 => Some(14),
        KeyCode::F15 => Some(15),
        KeyCode::F16 => Some(16),
        KeyCode::F17 => Some(17),
        KeyCode::F18 => Some(18),
        KeyCode::F19 => Some(19),
        KeyCode::F20 => Some(20),
        KeyCode::F21 => Some(21),
        KeyCode::F22 => Some(22),
        KeyCode::F23 => Some(23),
        KeyCode::F24 => Some(24),
        KeyCode::F25 => Some(25),
        _ => None,
    }
}

fn parse_state_dir() -> Option<PathBuf> {
    let mut arguments = std::env::args_os().skip(1);
    while let Some(argument) = arguments.next() {
        if argument == "--state-dir" {
            return arguments.next().map(PathBuf::from);
        }
    }
    std::env::var_os("MUX_STATE_DIR").map(PathBuf::from)
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "mux=info".into()),
        )
        .init();

    if std::env::args_os().any(|argument| argument == "--daemon") {
        let state_dir = parse_state_dir()
            .or_else(mux_client::default_state_dir)
            .ok_or_else(|| anyhow!("no application data directory"))?;
        info!(state_dir = %state_dir.display(), "starting persistent workspace daemon");
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        return runtime.block_on(backend::run_daemon(state_dir));
    }

    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    let mut application = Application {
        event_proxy: Some(event_loop.create_proxy()),
        state_dir: parse_state_dir(),
        ..Application::default()
    };
    event_loop.run_app(&mut application)?;
    Ok(())
}

#[cfg(test)]
mod input_tests {
    use super::*;

    #[test]
    fn physical_tab_is_always_identified_for_libghostty() {
        assert_eq!(physical_terminal_key(KeyCode::Tab), Some(TerminalKey::Tab));
    }

    #[test]
    fn composer_backspace_removes_one_user_perceived_character() {
        let mut value = "agent e\u{301}🚀".to_owned();
        pop_grapheme(&mut value);
        assert_eq!(value, "agent e\u{301}");
        pop_grapheme(&mut value);
        assert_eq!(value, "agent ");
    }

    #[test]
    fn terminal_hyperlinks_require_a_safe_absolute_uri() {
        assert!(validate_hyperlink_uri("https://example.com/docs").is_ok());
        assert!(validate_hyperlink_uri("mailto:hello@example.com").is_ok());
        assert!(validate_hyperlink_uri("not a URI").is_err());
        assert!(validate_hyperlink_uri("1nvalid:value").is_err());
        assert!(validate_hyperlink_uri("https://example.com\ncommand").is_err());
    }

    #[test]
    fn event_loop_uses_the_earliest_interaction_deadline() {
        let now = Instant::now();
        let first = now + Duration::from_millis(15);
        let second = now + Duration::from_millis(600);
        assert_eq!(earliest_deadline(Some(first), Some(second)), Some(first));
        assert_eq!(earliest_deadline(None, Some(second)), Some(second));
        assert_eq!(earliest_deadline(None, None), None);
    }

    #[test]
    fn agent_composer_supports_clear_and_multiline_shortcuts() {
        let mut application = Application {
            agent_surface: Some(AgentSurface {
                selected: 0,
                draft: "first line".to_owned(),
                loading: false,
                launcher: None,
                context: AgentContextMode::None,
                pending_end: None,
                timeline_scroll: 0,
                command_selection: 3,
            }),
            ..Application::default()
        };

        application.modifiers = ModifiersState::CONTROL;
        assert!(application.handle_agent_composer_shortcut(&Key::Character("u".into())));
        let surface = application.agent_surface.as_ref().expect("agent surface");
        assert!(surface.draft.is_empty());
        assert_eq!(surface.command_selection, 0);

        application.modifiers = ModifiersState::SHIFT;
        assert!(application.handle_agent_composer_shortcut(&Key::Named(NamedKey::Enter)));
        assert_eq!(
            application
                .agent_surface
                .as_ref()
                .expect("agent surface")
                .draft,
            "\n"
        );
    }

    #[test]
    fn failed_agent_start_returns_the_launcher_to_a_retryable_state() {
        let mut application = Application {
            agent_surface: Some(AgentSurface {
                selected: 0,
                draft: String::new(),
                loading: true,
                launcher: Some(AgentLauncher {
                    selected_profile: 0,
                    cwd_override: None,
                }),
                context: AgentContextMode::None,
                pending_end: None,
                timeline_scroll: 0,
                command_selection: 0,
            }),
            ..Application::default()
        };

        application
            .backend_error("agent runtime unavailable".to_owned())
            .expect("surface update");

        assert!(!application.agent_surface.as_ref().expect("surface").loading);
        assert_eq!(
            application.message.as_deref(),
            Some("agent runtime unavailable")
        );
    }
}
