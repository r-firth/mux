// Winit reports physical coordinates as f64/u32 while the bounded GPU layout
// uses f32. Desktop window dimensions are far below f32's exact-integer limit.
#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]

mod backend;
mod layout;
mod render;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result, anyhow};
use backend::{BackendHandle, CommandMessage};
use layout::WorkspaceGeometry;
use mux_acp::{
    AgentConfigCategory, AgentConfigValue, AgentConfigValueSelection, AgentContext,
    AgentContextKind, AgentEvent, AgentProfile, AgentPrompt, AgentSessionSnapshot,
    AgentSessionStatus, built_in_agent_profiles,
};
use mux_protocol::{ServerEvent, SessionAttachment, SessionSummary};
use mux_terminal::{
    CellWidth, RenderFrame, Rgb, TerminalEngine, TerminalInteraction, TerminalKey,
    TerminalKeyAction, TerminalKeyEvent, TerminalModifiers, TerminalMouseAction,
    TerminalMouseButton, TerminalMouseEvent, TerminalPoint, TerminalRenderer, TerminalSelection,
    TerminalSize, TerminalViewportScroll,
};
use mux_terminal_ghostty::{GhosttyEngine, GhosttyTheme};
use mux_workspace::{
    Action, InputMode, Key as MuxKey, KeyChord, Keymap, Modifiers, PaneId, Session,
    WorkspaceCommand,
};
use render::{AgentLauncherView, AgentSurfaceView, Renderer, SessionSwitcherView, UiState};
use tracing::{error, info};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, Ime, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use winit::window::{Window, WindowId};

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
}

struct AgentSurface {
    selected: usize,
    draft: String,
    loading: bool,
    launcher: Option<AgentLauncher>,
    context: AgentContextMode,
    pending_end: Option<mux_workspace::AgentSessionId>,
    timeline_scroll: usize,
}

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
    anchor: TerminalPoint,
    focus: TerminalPoint,
    rectangular: bool,
    moved: bool,
}

struct Application {
    renderer: Option<Renderer>,
    backend: Option<BackendHandle>,
    session: Option<Session>,
    panes: HashMap<PaneId, PaneReplica>,
    dirty_panes: HashSet<PaneId>,
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
    selection_drag: Option<SelectionDrag>,
    selected_pane: Option<PaneId>,
    active_selection: Option<(PaneId, TerminalSelection)>,
    clipboard: Option<arboard::Clipboard>,
    message: Option<String>,
    event_proxy: Option<EventLoopProxy<UserEvent>>,
    state_dir: Option<PathBuf>,
    session_switcher: Option<SessionSwitcher>,
    agents: Vec<AgentSessionSnapshot>,
    agent_profiles: Vec<AgentProfile>,
    agent_surface: Option<AgentSurface>,
    agent_surface_progress: f32,
    agent_surface_target: f32,
    last_animation_frame: Option<Instant>,
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
            selection_drag: None,
            selected_pane: None,
            active_selection: None,
            clipboard: arboard::Clipboard::new().ok(),
            message: None,
            event_proxy: None,
            state_dir: None,
            session_switcher: None,
            agents: Vec::new(),
            agent_profiles: built_in_agent_profiles(),
            agent_surface: None,
            agent_surface_progress: 0.0,
            agent_surface_target: 0.0,
            last_animation_frame: None,
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
            if engine.render_frame()?.background == Rgb::default() && !self.ghostty_theme.is_empty()
            {
                engine.apply_theme(&self.ghostty_theme)?;
            }
            for chunk in &pane.terminal.replay {
                engine.apply_output(chunk.sequence, &chunk.bytes)?;
            }
            if let Some((selected_pane, selection)) = self.active_selection
                && selected_pane == pane.pane_id
            {
                engine.set_selection(Some(selection))?;
            }
            let frame = engine.render_frame()?;
            panes.insert(pane.pane_id, PaneReplica { engine, frame });
        }
        self.session = Some(attachment.session);
        self.panes = panes;
        self.active_selection = self
            .active_selection
            .filter(|(pane_id, _)| self.panes.contains_key(pane_id));
        self.selected_pane = self.active_selection.map(|(pane_id, _)| pane_id);
        self.selection_drag = self
            .selection_drag
            .filter(|drag| self.panes.contains_key(&drag.pane_id));
        self.sent_sizes.clear();
        let changed_panes = self.panes.keys().copied().collect();
        self.dirty_panes.clear();
        self.message = None;
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
            ServerEvent::Agent(event) => return self.apply_agent_event(&event),
            ServerEvent::AgentResyncRequired => {
                if let Some(backend) = &self.backend {
                    backend.send(CommandMessage::ListAgents);
                }
            }
        }
        Ok(())
    }

    fn replace_agents(&mut self, agents: Vec<AgentSessionSnapshot>) -> Result<()> {
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
        self.refresh_view()
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

    fn apply_agent_event(&mut self, event: &AgentEvent) -> Result<()> {
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
            return Ok(());
        }
        if matches!(
            event,
            AgentEvent::ConfigUpdated { .. }
                | AgentEvent::ModeUpdated { .. }
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
        self.refresh_view()
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
                pane.frame = pane.engine.render_frame()?;
                effective_changes.insert(pane_id);
            }
            if let Some(backend) = &self.backend {
                backend.send(CommandMessage::Resize { pane_id, size });
            }
        }

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
            UiState {
                mode: self.mode,
                message: self.message.as_deref(),
                session_switcher: self.session_switcher.as_ref().map(|switcher| {
                    SessionSwitcherView {
                        entries: &switcher.entries,
                        selected: switcher.selected,
                    }
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
                }),
            },
        );
        Ok(())
    }

    fn refresh_view(&mut self) -> Result<()> {
        self.sync_view(&HashSet::new())?;
        self.request_redraw();
        Ok(())
    }

    fn flush_terminal_frames(&mut self) -> Result<()> {
        if self.dirty_panes.is_empty() {
            return Ok(());
        }
        let changed_panes = std::mem::take(&mut self.dirty_panes);
        for pane_id in &changed_panes {
            if let Some(pane) = self.panes.get_mut(pane_id) {
                pane.frame = pane.engine.render_frame()?;
            }
        }
        self.sync_view(&changed_panes)
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

    fn handle_session_switcher_key(&mut self, event: &KeyEvent) {
        let key = event.key_without_modifiers();
        let mut attach = None;
        let mut close = false;
        if let Some(switcher) = &mut self.session_switcher {
            let len = switcher.entries.len();
            match key {
                Key::Named(NamedKey::Escape) => close = true,
                Key::Named(NamedKey::ArrowUp) => {
                    if len > 0 {
                        switcher.selected = (switcher.selected + len - 1) % len;
                    }
                }
                Key::Character(ref value) if value.eq_ignore_ascii_case("k") => {
                    if len > 0 {
                        switcher.selected = (switcher.selected + len - 1) % len;
                    }
                }
                Key::Named(NamedKey::ArrowDown) => {
                    if len > 0 {
                        switcher.selected = (switcher.selected + 1) % len;
                    }
                }
                Key::Character(ref value) if value.eq_ignore_ascii_case("j") => {
                    if len > 0 {
                        switcher.selected = (switcher.selected + 1) % len;
                    }
                }
                Key::Named(NamedKey::Enter) => {
                    attach = switcher
                        .entries
                        .get(switcher.selected)
                        .map(|session| session.id);
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
        } else if close {
            self.session_switcher = None;
        }
        let _ = self.refresh_view();
    }

    fn handle_agent_surface_key(&mut self, event: &KeyEvent) {
        let key = event.key_without_modifiers();
        if is_agent_surface_shortcut(event, self.modifiers)
            || matches!(key, Key::Named(NamedKey::Escape))
        {
            self.close_agent_surface();
            return;
        }

        let active_id = self
            .agent_surface
            .as_ref()
            .filter(|surface| surface.launcher.is_none())
            .and_then(|surface| self.agents.get(surface.selected))
            .map(|agent| agent.id);
        if self.modifiers.control_key()
            && matches!(key, Key::Character(ref value) if value.eq_ignore_ascii_case("c"))
        {
            if let (Some(backend), Some(session_id)) = (&self.backend, active_id) {
                backend.send(CommandMessage::CancelAgent(session_id));
            }
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
                    surface.draft.pop();
                }
                let _ = self.refresh_view();
            }
            Key::Named(NamedKey::ArrowUp) => {
                if let Some(surface) = &mut self.agent_surface
                    && surface.draft.is_empty()
                {
                    if let Some(launcher) = &mut surface.launcher {
                        if !self.agent_profiles.is_empty() {
                            launcher.selected_profile =
                                (launcher.selected_profile + self.agent_profiles.len() - 1)
                                    % self.agent_profiles.len();
                        }
                    } else if !self.agents.is_empty() {
                        surface.selected =
                            (surface.selected + self.agents.len() - 1) % self.agents.len();
                        surface.timeline_scroll = 0;
                    }
                }
                let _ = self.refresh_view();
            }
            Key::Named(NamedKey::ArrowDown | NamedKey::Tab) => {
                if let Some(surface) = &mut self.agent_surface
                    && surface.draft.is_empty()
                {
                    if let Some(launcher) = &mut surface.launcher {
                        if !self.agent_profiles.is_empty() {
                            launcher.selected_profile =
                                (launcher.selected_profile + 1) % self.agent_profiles.len();
                        }
                    } else if !self.agents.is_empty() {
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
                    }
                    let _ = self.refresh_view();
                }
            }
            _ => {}
        }
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
                AgentSessionStatus::Starting | AgentSessionStatus::Closed
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
                    "/new · /agents · /cwd · /context · /model · /effort · /mode · /end".to_owned(),
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
                    .active_selection
                    .map_or_else(|| self.focused_pane_id(), |(pane_id, _)| Some(pane_id))
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
        self.agent_surface = Some(AgentSurface {
            selected: 0,
            draft: String::new(),
            loading: true,
            launcher: None,
            context: AgentContextMode::None,
            pending_end: None,
            timeline_scroll: 0,
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
                pane.frame = pane.engine.render_frame()?;
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
        if whole_rows != 0 && !self.report_mouse_wheel(pane_id, whole_rows) {
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
        let reported = pane_id.is_some_and(|pane_id| {
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
                self.selection_drag = None;
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
            let composer_y = panel_y + panel_height - 80.0 * scale;
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
        if pane_id.is_some_and(|pane_id| {
            self.report_mouse_event(pane_id, TerminalMouseAction::Motion, button)
        }) {
            return;
        }
        self.mouse_dragged();
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
            self.selection_drag = None;
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

    fn scroll_pane(&mut self, pane_id: PaneId, scroll: TerminalViewportScroll) {
        let result = (|| -> Result<()> {
            let pane = self
                .panes
                .get_mut(&pane_id)
                .ok_or_else(|| anyhow!("terminal pane is unavailable"))?;
            pane.engine.scroll_viewport(scroll)?;
            pane.frame = pane.engine.render_frame()?;
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
            Action::WriteTerminal(bytes) => self.write_focused(bytes),
            Action::OpenSessionSwitcher => {
                self.mode = InputMode::Normal;
                self.session_switcher = Some(SessionSwitcher {
                    entries: Vec::new(),
                    selected: 0,
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
            Action::RenameTab | Action::OpenCommandPalette => {}
        }
    }

    fn write_focused(&self, bytes: Vec<u8>) {
        if let Some(backend) = &self.backend {
            backend.send(CommandMessage::WriteFocused { bytes });
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
        pane.frame = pane.engine.render_frame()?;
        if let Some(selection) = selection {
            self.active_selection = Some((pane_id, selection));
            self.selected_pane = Some(pane_id);
        } else if self
            .active_selection
            .is_some_and(|(selected_pane, _)| selected_pane == pane_id)
        {
            self.active_selection = None;
            self.selected_pane = None;
        }
        self.sync_view(&HashSet::from([pane_id]))?;
        self.request_redraw();
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
        if let Some(tab) = self
            .geometry
            .tabs
            .iter()
            .find(|tab| tab.rect.contains(x, y))
        {
            self.send_workspace(WorkspaceCommand::SelectTab(tab.tab_id));
            return;
        }
        if cfg!(target_os = "macos") && y < layout::TAB_BAR_HEIGHT {
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
            return;
        };
        if !pane.focused {
            self.send_workspace(WorkspaceCommand::SetFocusedPane(pane.pane_id));
        }
        let point =
            renderer.terminal_point_at(pane, self.cursor_position.0, self.cursor_position.1);
        let Some(point) = point else {
            if let Err(error) = self.clear_selected_pane() {
                self.message = Some(error.to_string());
            }
            return;
        };
        if self.selected_pane != Some(pane.pane_id)
            && let Err(error) = self.clear_selected_pane()
        {
            self.message = Some(error.to_string());
            return;
        }
        let drag = SelectionDrag {
            pane_id: pane.pane_id,
            anchor: point,
            focus: point,
            rectangular: self.modifiers.alt_key(),
            moved: false,
        };
        self.selection_drag = Some(drag);
        self.selected_pane = Some(pane.pane_id);
        if let Err(error) = self.set_pane_selection(
            pane.pane_id,
            Some(TerminalSelection {
                anchor: point,
                focus: point,
                rectangular: drag.rectangular,
            }),
        ) {
            self.message = Some(error.to_string());
        }
    }

    fn mouse_dragged(&mut self) {
        let Some(mut drag) = self.selection_drag else {
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
        let Some(point) =
            renderer.terminal_point_at(pane, self.cursor_position.0, self.cursor_position.1)
        else {
            return;
        };
        if point == drag.focus {
            return;
        }
        drag.focus = point;
        drag.moved = true;
        self.selection_drag = Some(drag);
        if let Err(error) = self.set_pane_selection(
            drag.pane_id,
            Some(TerminalSelection {
                anchor: drag.anchor,
                focus: drag.focus,
                rectangular: drag.rectangular,
            }),
        ) {
            self.message = Some(error.to_string());
        }
    }

    fn mouse_released(&mut self) {
        self.mouse_dragged();
        let Some(drag) = self.selection_drag.take() else {
            return;
        };
        if !drag.moved {
            self.selected_pane = None;
            if let Err(error) = self.set_pane_selection(drag.pane_id, None) {
                self.message = Some(error.to_string());
            }
        }
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
                self.session_switcher = Some(SessionSwitcher { entries, selected });
                self.refresh_view()
            }
            UserEvent::Server(event) => self.apply_server_event(event),
            UserEvent::Agents(agents) => self.replace_agents(agents),
            UserEvent::AgentStarted(agent) => self.agent_started(agent),
            UserEvent::Agent(event) => self.apply_agent_event(&event),
            UserEvent::BackendError(message) => {
                self.message = Some(message);
                self.refresh_view()
            }
            UserEvent::ExitRequested => unreachable!("handled above"),
        };
        if let Err(error) = result {
            self.message = Some(error.to_string());
            let _ = self.refresh_view();
        }
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
            WindowEvent::RedrawRequested => {
                if self.advance_ui_animation() {
                    let _ = self.sync_view(&HashSet::new());
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
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::Ime(Ime::Commit(text)) if self.agent_surface.is_some() => {
                if let Some(surface) = &mut self.agent_surface {
                    surface.draft.push_str(&text);
                }
                let _ = self.refresh_view();
            }
            WindowEvent::Ime(Ime::Commit(text)) if self.mode == InputMode::Normal => {
                self.write_focused(text.into_bytes());
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.handle_cursor_moved(position.x as f32, position.y as f32);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                self.handle_mouse_button(state, button);
            }
            WindowEvent::MouseWheel { delta, .. } => self.handle_mouse_wheel(delta),
            _ => {}
        }
    }
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
}
