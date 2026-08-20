#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

mod agent_completion;
mod backend;
mod gpui_terminal;
mod layout;
mod settings;

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::time::{Duration, Instant};

use agent_completion::{
    AgentCommandArgument, AgentCompletion, AgentCompletionKind, AgentCompletionMenu,
    AgentCompletionProvider,
};
use anyhow::{Context as _, Result, anyhow};
use backend::{BackendHandle, CommandMessage};
use bezel::{
    theme::{self as bezel_theme, Appearance as BezelAppearance, Theme as BezelTheme},
    ui::{
        self as bezel_ui, icons as bezel_icons,
        widgets::{self as bezel_widgets, Content as _, Scaffolding as _, Status as _},
    },
};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    Animation, AnimationExt as _, App, AppContext as _, AssetSource, Bounds, Context, Entity,
    FocusHandle, Hsla, InteractiveElement as _, IntoElement, KeyBinding, KeyDownEvent, KeyUpEvent,
    Menu, MenuItem, ParentElement as _, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled, SystemMenuType, Window, WindowBounds, WindowOptions,
    div, px, rgb, size,
};
use gpui_component::{
    Icon, IconName, InteractiveElementExt as _, Selectable as _, Sizable as _, StyledExt as _,
    Theme as ComponentTheme, ThemeMode, TitleBar, WindowExt as _,
    animation::cubic_bezier,
    button::{Button, ButtonVariant, ButtonVariants as _},
    dialog::DialogButtonProps,
    h_flex,
    input::{Enter, Input, InputEvent, InputState, Position, Textarea, TextareaState},
    notification::Notification,
    scroll::ScrollableElement as _,
    switch::Switch,
    v_flex,
};
use gpui_terminal::{GridMetrics, TerminalRenderCache};
use mux_acp::{
    AgentConfigCategory, AgentConfigValue, AgentConfigValueSelection, AgentContext,
    AgentContextKind, AgentEvent, AgentMessageRole, AgentProfile, AgentPrompt,
    AgentSessionSnapshot, AgentSessionStatus, AgentTimelineItem, AgentTool, AgentToolKind,
    ToolStatus, built_in_agent_profiles,
};
use mux_protocol::{PaneAttachment, ServerEvent, SessionAttachment, SessionSummary};
use mux_terminal::{
    CellWidth, RenderFrame, Rgb, TerminalEngine, TerminalInteraction, TerminalKey,
    TerminalKeyAction, TerminalKeyEvent, TerminalModifiers, TerminalMouseAction,
    TerminalMouseButton, TerminalMouseEvent, TerminalMouseGeometry, TerminalPoint,
    TerminalRenderer, TerminalSelectionGeometry, TerminalSelectionGestureEvent, TerminalSize,
    TerminalSurfacePosition, TerminalViewportScroll,
};
use mux_terminal_ghostty::{GhosttyEngine, GhosttyFont, GhosttyTheme};
use mux_workspace::{
    Action, AgentSessionId, Direction, InputMode, Key as MuxKey, KeyChord, Keymap, Modifiers,
    PaneId, Session, SessionId, TabId, WorkspaceCommand,
};
use settings::AppSettings;
use tracing::{debug, error, info, warn};

const WINDOW_WIDTH: f32 = 1120.0;
const WINDOW_HEIGHT: f32 = 720.0;
const SURFACE: u32 = 0x0011_131a;
const MUTED_TEXT: u32 = 0x008c_96a8;
const SIGNAL: u32 = 0x005e_b6e8;
const HEADER_ACTIONS_WIDTH: f32 = 142.0;
const EMBEDDED_TERMINAL_FONT: &str = "JetBrainsMono Nerd Font Mono";
const INITIAL_USER_EVENT_BATCH_CAPACITY: usize = 8;
const MAX_USER_EVENT_BATCH: usize = 256;

struct MuxAssets;

impl AssetSource for MuxAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        let bezel_assets = bezel_icons::Assets;
        if let Some(asset) = bezel_assets.load(path)? {
            return Ok(Some(asset));
        }
        gpui_component_assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        let mut assets = bezel_icons::Assets.list(path)?;
        assets.extend(gpui_component_assets::Assets.list(path)?);
        assets.sort_unstable();
        assets.dedup();
        Ok(assets)
    }
}

#[cfg(target_os = "macos")]
fn reduce_motion_requested() -> bool {
    use objc2_app_kit::NSWorkspace;

    NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
}

#[cfg(not(target_os = "macos"))]
const fn reduce_motion_requested() -> bool {
    false
}

gpui::actions!(
    mux_agent,
    [
        CancelAgentTurn,
        DismissAgentCompletion,
        ForwardTerminalBacktab,
        ForwardTerminalTab,
        InsertAgentCompletion,
        NavigateAgentDown,
        NavigateAgentLeft,
        NavigateAgentRight,
        NavigateAgentUp,
        QuitMux,
        SelectNextAgentCompletion,
        SelectPreviousAgentCompletion,
        ToggleAgentPane
    ]
);

enum UserEvent {
    Attached(SessionAttachment),
    WorkspaceUpdated(SessionAttachment),
    Sessions(Vec<SessionSummary>),
    Server(ServerEvent),
    Agents(Vec<AgentSessionSnapshot>),
    AgentStarted(AgentSessionSnapshot),
    Agent(AgentEvent),
    AgentFiles {
        pane_id: PaneId,
        cwd: PathBuf,
        files: agent_completion::AgentFileIndex,
    },
    BackendError(String),
}

struct PaneReplica {
    engine: GhosttyEngine,
    frame: Rc<RenderFrame>,
    render_cache: Rc<RefCell<TerminalRenderCache>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PaneOutputUpdate {
    Applied,
    Duplicate,
    Gap { expected: u64, actual: u64 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AgentTabActivity {
    Idle,
    Working,
    Attention,
}

impl PaneReplica {
    fn new(engine: GhosttyEngine, frame: RenderFrame) -> Self {
        Self {
            engine,
            frame: Rc::new(frame),
            render_cache: Rc::new(RefCell::new(TerminalRenderCache::default())),
        }
    }

    fn apply_output(&mut self, sequence: u64, bytes: &[u8]) -> Result<PaneOutputUpdate> {
        let expected = self.engine.next_output_sequence();
        if sequence < expected {
            return Ok(PaneOutputUpdate::Duplicate);
        }
        if sequence > expected {
            return Ok(PaneOutputUpdate::Gap {
                expected,
                actual: sequence,
            });
        }
        self.engine.apply_output(sequence, bytes)?;
        Ok(PaneOutputUpdate::Applied)
    }

    fn publish_frame(&mut self) -> Result<()> {
        // A canvas keeps its frame alive through both prepaint and paint. If a
        // draw is still holding the old snapshot, `make_mut` clones before
        // updating; otherwise libghostty writes into the existing allocation.
        // Either way, a draw can never combine shaped text from one terminal
        // state with backgrounds or a cursor from another.
        self.engine
            .render_frame_into(Rc::make_mut(&mut self.frame))?;
        Ok(())
    }
}

fn restore_pane_replica(
    pane: &PaneAttachment,
    ghostty_theme: &GhosttyTheme,
) -> Result<PaneReplica> {
    pane.terminal
        .validate_sequence_contract()
        .with_context(|| format!("invalid terminal attachment for pane {}", pane.pane_id))?;
    let checkpoint = pane
        .terminal
        .checkpoint
        .as_ref()
        .ok_or_else(|| anyhow!("daemon returned a non-libghostty terminal attachment"))?;
    let mut engine = GhosttyEngine::restore(checkpoint)
        .with_context(|| format!("restore terminal pane {}", pane.pane_id))?;
    let mut frame = engine.render_frame()?;
    if frame.background == Rgb::default() && !ghostty_theme.is_empty() {
        engine.apply_theme(ghostty_theme)?;
        engine.render_frame_into(&mut frame)?;
    }
    for chunk in &pane.terminal.replay {
        engine.apply_output(chunk.sequence, &chunk.bytes)?;
    }
    let restored_next_sequence = engine.next_output_sequence();
    if restored_next_sequence != pane.terminal.next_sequence {
        return Err(anyhow!(
            "inconsistent terminal attachment for pane {}: restored cursor {}, advertised cursor {}",
            pane.pane_id,
            restored_next_sequence,
            pane.terminal.next_sequence
        ));
    }
    if !pane.terminal.replay.is_empty() {
        engine.render_frame_into(&mut frame)?;
    }
    Ok(PaneReplica::new(engine, frame))
}

fn restore_pane_replicas(
    attachments: &[PaneAttachment],
    ghostty_theme: &GhosttyTheme,
) -> Result<HashMap<PaneId, PaneReplica>> {
    attachments
        .iter()
        .map(|pane| Ok((pane.pane_id, restore_pane_replica(pane, ghostty_theme)?)))
        .collect()
}

fn reconcile_pane_replicas(
    mut existing: HashMap<PaneId, PaneReplica>,
    attachments: &[PaneAttachment],
    ghostty_theme: &GhosttyTheme,
) -> Result<HashMap<PaneId, PaneReplica>> {
    let mut reconciled = HashMap::with_capacity(attachments.len());
    for pane in attachments {
        let replica = match existing.remove(&pane.pane_id) {
            Some(replica)
                if replica.engine.next_output_sequence() == pane.terminal.next_sequence =>
            {
                replica
            }
            Some(_) | None => restore_pane_replica(pane, ghostty_theme)?,
        };
        reconciled.insert(pane.pane_id, replica);
    }
    Ok(reconciled)
}

#[derive(Default)]
struct PaneScrollState {
    fractional_rows: f32,
}

#[derive(Clone, Copy)]
struct TerminalPointerCapture {
    pane_id: PaneId,
    rect: layout::Rect,
}

impl PaneScrollState {
    fn accumulate(&mut self, delta_rows: f32, reset_fraction: bool) -> i64 {
        if reset_fraction
            || (self.fractional_rows != 0.0 && self.fractional_rows.signum() != delta_rows.signum())
        {
            self.fractional_rows = 0.0;
        }
        let accumulated = self.fractional_rows + delta_rows;
        let whole_rows = accumulated.trunc() as i64;
        self.fractional_rows = accumulated - whole_rows as f32;
        whole_rows
    }
}

#[derive(Clone, Copy)]
struct SelectionPointer {
    point: Option<TerminalPoint>,
    clamped_point: TerminalPoint,
    position: TerminalSurfacePosition,
    geometry: TerminalSelectionGeometry,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AgentContextMode {
    None,
    Tab,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MotionPreference {
    Full,
    Reduced,
}

struct MuxApp {
    focus_handle: FocusHandle,
    backend: BackendHandle,
    state_dir: Option<PathBuf>,
    settings: AppSettings,
    profiles: Vec<AgentProfile>,
    session: Option<Session>,
    panes: HashMap<PaneId, PaneReplica>,
    pane_scrolls: HashMap<PaneId, PaneScrollState>,
    sent_sizes: HashMap<PaneId, TerminalSize>,
    sessions: Vec<SessionSummary>,
    agents: Vec<AgentSessionSnapshot>,
    /// A missing entry selects the first session for that tab. `None` as the
    /// stored value deliberately selects the new-session composer.
    selected_agents: HashMap<TabId, Option<AgentSessionId>>,
    agent_input: Entity<TextareaState>,
    agent_input_tab: Option<TabId>,
    agent_drafts: HashMap<TabId, String>,
    agent_completion: Rc<AgentCompletionProvider>,
    agent_completion_menu: Option<AgentCompletionMenu>,
    _agent_input_subscription: gpui::Subscription,
    pending_agent_prompt: Option<AgentPrompt>,
    agent_panes: HashMap<TabId, PaneId>,
    agent_scrolls: HashMap<TabId, ScrollHandle>,
    agent_follow_tail: HashSet<TabId>,
    agent_scroll_needs_settle: HashSet<TabId>,
    agent_help_tabs: HashSet<TabId>,
    expanded_agent_items: HashSet<String>,
    agent_context: AgentContextMode,
    selected_pane: Option<PaneId>,
    pending_focused_pane: Option<PaneId>,
    selection_drag: Option<TerminalPointerCapture>,
    mouse_reporting: Option<TerminalPointerCapture>,
    terminal_key_presses: HashMap<String, PaneId>,
    terminal_resync_pending: Option<SessionId>,
    selection_clock_origin: Instant,
    keymap: Keymap,
    mode: InputMode,
    motion: MotionPreference,
    metrics: GridMetrics,
    terminal_font: String,
    ghostty_theme: GhosttyTheme,
    clipboard: Option<arboard::Clipboard>,
}

struct MuxLayerHost {
    view: Entity<MuxApp>,
}

fn create_agent_input(
    window: &mut Window,
    cx: &mut Context<MuxApp>,
) -> (Entity<TextareaState>, gpui::Subscription) {
    let input = cx.new(|cx| {
        TextareaState::new(window, cx)
            .auto_grow(1, 6)
            .placeholder("Message an agent · / for commands · @ for files…")
    });
    let subscription = cx.subscribe_in(
        &input,
        window,
        |this, _, event: &InputEvent, window, cx| match event {
            InputEvent::Change => {
                if let Some(tab_id) = this.agent_input_tab {
                    let value = this.agent_input.read(cx).value().to_string();
                    this.agent_drafts.insert(tab_id, value);
                }
                this.refresh_agent_completion_menu(cx);
            }
            InputEvent::PressEnter {
                secondary: false,
                shift: false,
            } if this.agent_completion_menu.is_none() => {
                this.submit_agent_prompt(window, cx);
            }
            InputEvent::PressEnter { .. } | InputEvent::Focus | InputEvent::Blur => {}
        },
    );
    (input, subscription)
}

impl MuxApp {
    fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        state_dir: Option<PathBuf>,
        settings: AppSettings,
        settings_error: Option<String>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window, cx);
        let font_config = GhosttyFont::load_user().unwrap_or_default();
        let font_size = font_config.size.unwrap_or(14.0).clamp(8.0, 36.0);
        let requested_font = font_config.family.as_deref().unwrap_or_default().trim();
        let available_fonts = cx.text_system().all_font_names();
        let terminal_font = available_fonts
            .iter()
            .find(|family| family.eq_ignore_ascii_case(requested_font))
            .cloned()
            .unwrap_or_else(|| EMBEDDED_TERMINAL_FONT.to_owned());
        info!(
            requested = requested_font,
            resolved = terminal_font,
            font_size,
            "resolved GPUI terminal font"
        );
        let metrics = GridMetrics::from_font(&terminal_font, font_size, cx.text_system());
        info!(
            cell_width = metrics.cell_width,
            cell_height = metrics.cell_height,
            "measured terminal grid"
        );
        let reduce_motion = reduce_motion_requested();
        info!(reduce_motion, "resolved interface motion preference");
        let motion = if reduce_motion {
            MotionPreference::Reduced
        } else {
            MotionPreference::Full
        };
        let agent_completion = Rc::new(AgentCompletionProvider::default());
        let (agent_input, agent_input_subscription) = create_agent_input(window, cx);
        let (events, receiver) = async_channel::unbounded();
        let backend = backend::spawn(events, state_dir.clone());
        backend.send(CommandMessage::ListAgents);
        info!("GPUI workspace view initialized");
        Self::spawn_backend_event_loop(receiver, window, cx);

        if let Some(message) = settings_error {
            window.push_notification(Notification::warning(message), cx);
        }

        let profiles = merge_agent_profiles(&settings);
        Self {
            focus_handle,
            backend,
            state_dir,
            settings,
            profiles,
            session: None,
            panes: HashMap::new(),
            pane_scrolls: HashMap::new(),
            sent_sizes: HashMap::new(),
            sessions: Vec::new(),
            agents: Vec::new(),
            selected_agents: HashMap::new(),
            agent_input,
            agent_input_tab: None,
            agent_drafts: HashMap::new(),
            agent_completion,
            agent_completion_menu: None,
            _agent_input_subscription: agent_input_subscription,
            pending_agent_prompt: None,
            agent_panes: HashMap::new(),
            agent_scrolls: HashMap::new(),
            agent_follow_tail: HashSet::new(),
            agent_scroll_needs_settle: HashSet::new(),
            agent_help_tabs: HashSet::new(),
            expanded_agent_items: HashSet::new(),
            agent_context: AgentContextMode::Tab,
            selected_pane: None,
            pending_focused_pane: None,
            selection_drag: None,
            mouse_reporting: None,
            terminal_key_presses: HashMap::new(),
            terminal_resync_pending: None,
            selection_clock_origin: Instant::now(),
            keymap: Keymap::zellij_default(),
            mode: InputMode::Normal,
            motion,
            metrics,
            terminal_font,
            ghostty_theme: GhosttyTheme::load_user().unwrap_or_default(),
            clipboard: arboard::Clipboard::new().ok(),
        }
    }

    fn spawn_backend_event_loop(
        receiver: async_channel::Receiver<UserEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |entity, cx| {
            while let Ok(first_event) = receiver.recv().await {
                let first_label = first_event.label();
                let mut batch = Vec::with_capacity(INITIAL_USER_EVENT_BATCH_CAPACITY);
                batch.push(first_event);
                while batch.len() < MAX_USER_EVENT_BATCH {
                    let Ok(event) = receiver.try_recv() else {
                        break;
                    };
                    batch.push(event);
                }
                debug!(
                    event_count = batch.len(),
                    first_event = first_label,
                    "GPUI received backend event batch"
                );
                let _ = cx.update(|window, app| {
                    let _ = entity.update(app, |this, cx| {
                        if this.apply_user_events(batch, window, cx) {
                            cx.notify();
                        }
                    });
                });
            }
        })
        .detach();
    }

    fn apply_user_events(
        &mut self,
        events: Vec<UserEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut dirty_panes = HashSet::new();
        let mut needs_render = false;
        for event in events {
            match event {
                UserEvent::Server(ServerEvent::PaneOutput {
                    session_id,
                    pane_id,
                    sequence,
                    bytes,
                }) => {
                    let visible = self.terminal_pane_is_visible(pane_id);
                    let result = self
                        .panes
                        .get_mut(&pane_id)
                        .map_or(Ok(PaneOutputUpdate::Duplicate), |pane| {
                            pane.apply_output(sequence, &bytes)
                        });
                    match result {
                        Ok(PaneOutputUpdate::Applied) if visible => {
                            dirty_panes.insert(pane_id);
                            needs_render = true;
                        }
                        Ok(PaneOutputUpdate::Applied | PaneOutputUpdate::Duplicate) => {}
                        Ok(PaneOutputUpdate::Gap { expected, actual }) => {
                            if self.terminal_resync_pending != Some(session_id) {
                                warn!(
                                    %session_id,
                                    %pane_id,
                                    expected,
                                    actual,
                                    "GUI terminal replica fell behind; requesting a fresh attachment"
                                );
                                self.terminal_resync_pending = Some(session_id);
                                self.backend.send(CommandMessage::AttachSession(session_id));
                            }
                        }
                        Err(error) => Self::report_ui_error(&error, window, cx),
                    }
                }
                event @ (UserEvent::Attached(_) | UserEvent::WorkspaceUpdated(_)) => {
                    self.publish_terminal_frames(&mut dirty_panes, window, cx);
                    self.apply_user_event(event, window, cx);
                    needs_render = true;
                }
                event => {
                    self.apply_user_event(event, window, cx);
                    needs_render = true;
                }
            }
        }
        self.publish_terminal_frames(&mut dirty_panes, window, cx);
        needs_render
    }

    fn publish_terminal_frames(
        &mut self,
        dirty_panes: &mut HashSet<PaneId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for pane_id in dirty_panes.drain() {
            if let Some(pane) = self.panes.get_mut(&pane_id)
                && let Err(error) = pane.publish_frame()
            {
                Self::report_ui_error(&error, window, cx);
            }
        }
    }

    fn report_ui_error(error: &anyhow::Error, window: &mut Window, cx: &mut Context<Self>) {
        error!(%error, "Mux UI update failed");
        window.push_notification(Notification::error(error.to_string()), cx);
    }

    fn apply_user_event(&mut self, event: UserEvent, window: &mut Window, cx: &mut Context<Self>) {
        let result = match event {
            UserEvent::Attached(attachment) => {
                self.apply_workspace_attachment(attachment, true, window, cx)
            }
            UserEvent::WorkspaceUpdated(attachment) => {
                self.apply_workspace_attachment(attachment, false, window, cx)
            }
            UserEvent::Sessions(sessions) => {
                self.sessions = sessions;
                Ok(())
            }
            UserEvent::Server(event) => self.apply_server_event(event),
            UserEvent::Agents(agents) => {
                self.agents = agents;
                let valid = self
                    .agents
                    .iter()
                    .filter_map(|agent| agent.tab_id.map(|tab_id| (tab_id, agent.id)))
                    .collect::<HashSet<_>>();
                self.selected_agents.retain(|tab_id, selection| {
                    selection.is_none_or(|session_id| valid.contains(&(*tab_id, session_id)))
                });
                if let Some(tab_id) = self.active_tab_id()
                    && self.agent_follow_tail.contains(&tab_id)
                {
                    self.agent_scroll_needs_settle.insert(tab_id);
                    self.agent_scroll_for(tab_id).scroll_to_bottom();
                }
                Ok(())
            }
            UserEvent::AgentStarted(agent) => {
                let session_id = agent.id;
                if let Some(tab_id) = agent.tab_id {
                    self.selected_agents.insert(tab_id, Some(session_id));
                    self.agent_follow_tail.insert(tab_id);
                    self.agent_scroll_needs_settle.insert(tab_id);
                    self.agent_scroll_for(tab_id).scroll_to_bottom();
                }
                self.agents.push(agent);
                if let Some(prompt) = self.pending_agent_prompt.take() {
                    self.backend
                        .send(CommandMessage::PromptAgent { session_id, prompt });
                }
                Ok(())
            }
            UserEvent::Agent(event) => {
                let tab_id = self
                    .agents
                    .iter()
                    .find(|agent| agent.id == event.session_id())
                    .and_then(|agent| agent.tab_id);
                if let Some(agent) = self
                    .agents
                    .iter_mut()
                    .find(|agent| agent.id == event.session_id())
                {
                    agent.apply(&event);
                } else {
                    self.backend.send(CommandMessage::ListAgents);
                }
                if let Some(tab_id) = tab_id
                    && self.agent_follow_tail.contains(&tab_id)
                {
                    self.agent_scroll_for(tab_id).scroll_to_bottom();
                }
                Ok(())
            }
            UserEvent::AgentFiles {
                pane_id,
                cwd,
                files,
            } => {
                if self.active_agent_pane() == Some(pane_id) {
                    self.agent_completion.set_file_index(cwd, files);
                    self.refresh_agent_completion_menu(cx);
                }
                Ok(())
            }
            UserEvent::BackendError(message) => {
                self.pending_agent_prompt = None;
                Err(anyhow!(message))
            }
        };
        if let Err(error) = result {
            Self::report_ui_error(&error, window, cx);
        }
    }

    fn apply_workspace_attachment(
        &mut self,
        mut attachment: SessionAttachment,
        rebuild: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        // Older live daemons may still hold numeric titles allocated from a
        // stale tab count. Repair the presentation snapshot without touching
        // stable tab IDs or user-authored text titles.
        attachment.session.normalize_numeric_tab_titles();
        let session_id = attachment.session.id;
        if rebuild {
            self.attach(attachment)?;
            if self.terminal_resync_pending == Some(session_id) {
                self.terminal_resync_pending = None;
            }
        } else {
            self.update_workspace(attachment)?;
        }
        self.sync_agent_draft_for_active_tab(window, cx);
        if let Some(pane_id) = self.active_agent_pane()
            && Some(pane_id) == self.focused_pane_id()
        {
            self.backend
                .send(CommandMessage::RefreshAgentFiles { pane_id });
            self.focus_agent_composer(window);
        }
        Ok(())
    }

    fn attach(&mut self, attachment: SessionAttachment) -> Result<()> {
        let panes = restore_pane_replicas(&attachment.panes, &self.ghostty_theme)?;
        self.session = Some(attachment.session);
        // Every attachment replaces the local emulators from daemon-owned
        // checkpoints. Force the next layout pass to size both copies again;
        // retaining an old sent size can leave a fresh replica at the
        // checkpoint's previous dimensions indefinitely.
        self.sent_sizes.clear();
        self.pane_scrolls.clear();
        self.panes = panes;
        self.selected_pane = None;
        self.pending_focused_pane = None;
        self.selection_drag = None;
        self.mouse_reporting = None;
        self.terminal_key_presses.clear();
        self.reconcile_workspace_ui_state();
        Ok(())
    }

    fn update_workspace(&mut self, attachment: SessionAttachment) -> Result<()> {
        let panes = reconcile_pane_replicas(
            std::mem::take(&mut self.panes),
            &attachment.panes,
            &self.ghostty_theme,
        )?;
        self.session = Some(attachment.session);
        self.panes = panes;
        let pane_ids = self.panes.keys().copied().collect::<HashSet<_>>();
        self.sent_sizes
            .retain(|pane_id, _| pane_ids.contains(pane_id));
        self.pane_scrolls
            .retain(|pane_id, _| pane_ids.contains(pane_id));
        self.selected_pane = self
            .selected_pane
            .filter(|pane_id| pane_ids.contains(pane_id));
        self.pending_focused_pane = self
            .pending_focused_pane
            .filter(|pane_id| pane_ids.contains(pane_id));
        if self.pending_focused_pane == self.focused_pane_id() {
            self.pending_focused_pane = None;
        }
        self.selection_drag = self
            .selection_drag
            .filter(|capture| pane_ids.contains(&capture.pane_id));
        self.mouse_reporting = self
            .mouse_reporting
            .filter(|capture| pane_ids.contains(&capture.pane_id));
        self.terminal_key_presses
            .retain(|_, pane_id| pane_ids.contains(pane_id));
        self.reconcile_workspace_ui_state();
        Ok(())
    }

    fn reconcile_workspace_ui_state(&mut self) {
        let valid_agent_panes = self
            .session
            .as_ref()
            .into_iter()
            .flat_map(|session| &session.tabs)
            .filter_map(|tab| {
                self.agent_panes
                    .get(&tab.id)
                    .copied()
                    .filter(|pane_id| tab.layout.contains(*pane_id))
                    .map(|pane_id| (tab.id, pane_id))
            })
            .collect::<HashMap<_, _>>();
        self.agent_panes = valid_agent_panes;
    }

    fn apply_server_event(&mut self, event: ServerEvent) -> Result<()> {
        match event {
            ServerEvent::PaneOutput {
                session_id,
                pane_id,
                sequence,
                bytes,
            } => {
                if let Some(pane) = self.panes.get_mut(&pane_id) {
                    match pane.apply_output(sequence, &bytes)? {
                        PaneOutputUpdate::Applied => pane.publish_frame()?,
                        PaneOutputUpdate::Duplicate => {}
                        PaneOutputUpdate::Gap { expected, actual } => {
                            if self.terminal_resync_pending != Some(session_id) {
                                warn!(
                                    %session_id,
                                    %pane_id,
                                    expected,
                                    actual,
                                    "GUI terminal replica fell behind; requesting a fresh attachment"
                                );
                                self.terminal_resync_pending = Some(session_id);
                                self.backend.send(CommandMessage::AttachSession(session_id));
                            }
                        }
                    }
                }
            }
            ServerEvent::PaneExited { .. }
            | ServerEvent::ResyncRequired { .. }
            | ServerEvent::WorkspaceChanged { .. } => {}
            ServerEvent::Agent(event) => {
                if let Some(agent) = self
                    .agents
                    .iter_mut()
                    .find(|agent| agent.id == event.session_id())
                {
                    agent.apply(&event);
                }
            }
            ServerEvent::AgentResyncRequired => self.backend.send(CommandMessage::ListAgents),
        }
        Ok(())
    }

    fn focused_pane_id(&self) -> Option<PaneId> {
        self.session
            .as_ref()?
            .active_tab()
            .map(|tab| tab.focused_pane)
    }

    fn terminal_input_pane_id(&self) -> Option<PaneId> {
        terminal_input_pane(self.pending_focused_pane, self.focused_pane_id())
    }

    fn active_tab_id(&self) -> Option<TabId> {
        self.session.as_ref().map(|session| session.active_tab)
    }

    fn active_agent_pane(&self) -> Option<PaneId> {
        let tab = self.session.as_ref()?.active_tab()?;
        self.agent_panes
            .get(&tab.id)
            .copied()
            .filter(|pane_id| tab.layout.contains(*pane_id))
    }

    fn terminal_pane_is_visible(&self, pane_id: PaneId) -> bool {
        let Some(session) = self.session.as_ref() else {
            return false;
        };
        pane_needs_live_frame(session, self.active_agent_pane(), pane_id)
    }

    fn agent_scroll_for(&mut self, tab_id: TabId) -> ScrollHandle {
        self.agent_scrolls.entry(tab_id).or_default().clone()
    }

    fn follow_active_agent_tail(&mut self) {
        if let Some(tab_id) = self.active_tab_id() {
            self.agent_follow_tail.insert(tab_id);
            self.agent_scroll_needs_settle.insert(tab_id);
            self.agent_scroll_for(tab_id).scroll_to_bottom();
        }
    }

    fn focus_agent_composer(&self, window: &mut Window) {
        let input = self.agent_input.clone();
        window.on_next_frame(move |window, cx| {
            input.update(cx, |input, cx| input.focus(window, cx));
        });
    }

    fn sync_agent_draft_for_active_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active_tab = self.active_tab_id();
        if self.agent_input_tab == active_tab {
            return;
        }
        if let Some(previous_tab) = self.agent_input_tab {
            let value = self.agent_input.read(cx).value().to_string();
            self.agent_drafts.insert(previous_tab, value);
        }
        self.agent_input_tab = active_tab;
        self.agent_completion_menu = None;
        self.agent_completion.clear_file_index();
        let value = active_tab
            .and_then(|tab_id| self.agent_drafts.get(&tab_id).cloned())
            .unwrap_or_default();
        self.agent_input.update(cx, |input, cx| {
            input.set_value(value, window, cx);
        });
    }

    fn refresh_agent_completion_menu(&mut self, cx: &App) {
        let input = self.agent_input.read(cx);
        let items = self
            .agent_completion
            .completions(input.value().as_ref(), input.cursor());
        if items.is_empty() {
            self.agent_completion_menu = None;
            return;
        }
        let previous = self
            .agent_completion_menu
            .as_ref()
            .and_then(|menu| menu.items.get(menu.selected))
            .map(|item| item.label.as_str());
        let selected = previous
            .and_then(|label| items.iter().position(|item| item.label == label))
            .unwrap_or_default();
        self.agent_completion_menu = Some(AgentCompletionMenu { items, selected });
    }

    fn select_agent_completion(&mut self, delta: isize, cx: &mut Context<Self>) -> bool {
        let Some(menu) = self.agent_completion_menu.as_mut() else {
            return false;
        };
        menu.select_relative(delta);
        cx.notify();
        true
    }

    fn dismiss_agent_completion(&mut self, cx: &mut Context<Self>) -> bool {
        if self.agent_completion_menu.take().is_none() {
            return false;
        }
        cx.notify();
        true
    }

    fn accept_agent_completion(
        &mut self,
        index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(menu) = self.agent_completion_menu.as_ref() else {
            return false;
        };
        let index = index.unwrap_or(menu.selected);
        let Some(completion) = menu.items.get(index).cloned() else {
            return false;
        };
        let value = self.agent_input.read(cx).value().to_string();
        if completion.start > completion.end
            || completion.end > value.len()
            || !value.is_char_boundary(completion.start)
            || !value.is_char_boundary(completion.end)
        {
            self.agent_completion_menu = None;
            return false;
        }
        let mut updated = String::with_capacity(
            value.len() - (completion.end - completion.start) + completion.replacement.len(),
        );
        updated.push_str(&value[..completion.start]);
        updated.push_str(&completion.replacement);
        let cursor = updated.len();
        updated.push_str(&value[completion.end..]);
        let position = input_position_at(&updated, cursor);
        self.agent_completion_menu = None;
        self.agent_input.update(cx, |input, cx| {
            input.set_value(updated, window, cx);
            input.set_cursor_position(position, window, cx);
        });
        cx.notify();
        true
    }

    fn toggle_agent_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab_id) = self.active_tab_id() else {
            return;
        };
        let Some(pane_id) = self.focused_pane_id() else {
            return;
        };
        if self.agent_panes.get(&tab_id) == Some(&pane_id) {
            self.agent_panes.remove(&tab_id);
            self.agent_follow_tail.remove(&tab_id);
            self.agent_scroll_needs_settle.remove(&tab_id);
            if let Some(pane) = self.panes.get_mut(&pane_id)
                && let Err(error) = pane.publish_frame()
            {
                Self::report_ui_error(&error, window, cx);
            }
            self.focus_handle.focus(window, cx);
        } else {
            self.agent_panes.insert(tab_id, pane_id);
            self.agent_follow_tail.insert(tab_id);
            self.agent_scroll_needs_settle.insert(tab_id);
            self.agent_scroll_for(tab_id).scroll_to_bottom();
            self.backend.send(CommandMessage::ListAgents);
            self.backend
                .send(CommandMessage::RefreshAgentFiles { pane_id });
            self.focus_agent_composer(window);
        }
        self.mode = InputMode::Normal;
        cx.notify();
    }

    fn return_agent_pane_to_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab_id) = self.active_tab_id()
            && let Some(pane_id) = self.agent_panes.remove(&tab_id)
        {
            self.agent_follow_tail.remove(&tab_id);
            self.agent_scroll_needs_settle.remove(&tab_id);
            if let Some(pane) = self.panes.get_mut(&pane_id)
                && let Err(error) = pane.publish_frame()
            {
                Self::report_ui_error(&error, window, cx);
            }
        }
        self.focus_handle.focus(window, cx);
        self.mode = InputMode::Normal;
        cx.notify();
    }

    fn navigate_from_agent_pane(
        &mut self,
        direction: Direction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(direction, Direction::Left | Direction::Right)
            && self.select_adjacent_agent(direction)
        {
            self.follow_active_agent_tail();
            self.focus_agent_composer(window);
            cx.notify();
            return;
        }
        let command = if matches!(direction, Direction::Left | Direction::Right) {
            WorkspaceCommand::FocusPaneOrTab(direction)
        } else {
            WorkspaceCommand::FocusPane(direction)
        };
        self.send_workspace(command);
        self.focus_handle.focus(window, cx);
        cx.notify();
    }

    /// Treat the tab-local agent sessions as neighboring surfaces within the
    /// agent pane. At either edge, returning `false` lets the same Option-arrow
    /// continue into the terminal pane or tab in that direction.
    fn select_adjacent_agent(&mut self, direction: Direction) -> bool {
        let agents = self
            .agents_for_active_tab()
            .map(|agent| agent.id)
            .collect::<Vec<_>>();
        if agents.is_empty() {
            return false;
        }
        let selected = self.active_agent().map(|agent| agent.id);
        let index = selected
            .and_then(|selected| agents.iter().position(|agent| *agent == selected))
            .unwrap_or(agents.len());
        let next = match direction {
            Direction::Left => index.checked_sub(1),
            Direction::Right if index + 1 < agents.len() => Some(index + 1),
            Direction::Right | Direction::Up | Direction::Down => None,
        };
        let Some(next) = next else {
            return false;
        };
        self.select_active_tab_agent(Some(agents[next]));
        true
    }

    fn agents_for_active_tab(&self) -> impl Iterator<Item = &AgentSessionSnapshot> {
        let tab_id = self.active_tab_id();
        self.agents.iter().filter(move |agent| {
            agent.tab_id == tab_id && tab_id.is_some() && agent_session_is_visible(agent.status)
        })
    }

    fn active_agent(&self) -> Option<&AgentSessionSnapshot> {
        let tab_id = self.active_tab_id()?;
        match self.selected_agents.get(&tab_id) {
            Some(Some(session_id)) => self
                .agents_for_active_tab()
                .find(|agent| agent.id == *session_id)
                .or_else(|| self.agents_for_active_tab().next()),
            Some(None) => None,
            None => self.agents_for_active_tab().next(),
        }
    }

    fn select_active_tab_agent(&mut self, selection: Option<AgentSessionId>) {
        if let Some(tab_id) = self.active_tab_id() {
            self.selected_agents.insert(tab_id, selection);
        }
    }

    fn send_workspace(&self, command: WorkspaceCommand) {
        if let Some(session) = &self.session {
            self.backend.send(CommandMessage::Workspace {
                session_id: session.id,
                command,
            });
        }
    }

    fn request_pane_focus(&mut self, pane_id: PaneId) {
        if self.terminal_input_pane_id() == Some(pane_id) {
            return;
        }
        self.pending_focused_pane = Some(pane_id);
        self.send_workspace(WorkspaceCommand::SetFocusedPane(pane_id));
    }

    fn write_focused(&self, bytes: Vec<u8>) {
        if !bytes.is_empty() {
            self.backend.send(CommandMessage::WriteFocused { bytes });
        }
    }

    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !event.is_held {
            // A fresh press supersedes any release ownership left behind when
            // the window lost focus before GPUI delivered the prior key-up.
            self.terminal_key_presses.remove(&event.keystroke.key);
        }
        if window.has_active_dialog(cx) || window.has_active_sheet(cx) {
            cx.propagate();
            return;
        }
        if self.active_agent_pane() == self.terminal_input_pane_id() {
            // The focused pane is currently a native agent surface. Its input
            // and modal-key handlers own this event; never leak it through to
            // the live terminal process hidden behind the surface.
            cx.propagate();
            return;
        }

        let Some(chord) = key_chord(&event.keystroke) else {
            self.send_terminal_key_down(&event.keystroke, event.is_held, window.capslock().on);
            cx.stop_propagation();
            return;
        };
        if let Some(action) = self.keymap.resolve(self.mode, chord).cloned() {
            self.perform_action(action, window, cx);
            cx.stop_propagation();
        } else {
            self.send_terminal_key_down(&event.keystroke, event.is_held, window.capslock().on);
            cx.stop_propagation();
        }
    }

    fn handle_key_up(&mut self, event: &KeyUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pane_id) =
            take_terminal_key_release(&mut self.terminal_key_presses, &event.keystroke.key)
        else {
            // The corresponding key-down belonged to Mux, a dialog, or a
            // native agent surface. Never invent a terminal release for it.
            cx.propagate();
            return;
        };
        self.send_terminal_key_to(pane_id, &event.keystroke, true, false, window.capslock().on);
        cx.stop_propagation();
    }

    fn send_terminal_key_down(&mut self, keystroke: &gpui::Keystroke, held: bool, caps_lock: bool) {
        if keystroke.modifiers.platform && keystroke.key == "c" {
            let selected = self.selected_pane.or_else(|| self.focused_pane_id());
            if let Some(text) = selected
                .and_then(|pane_id| self.panes.get(&pane_id))
                .and_then(|pane| pane.engine.selected_text().ok().flatten())
                && let Some(clipboard) = &mut self.clipboard
            {
                let _ = clipboard.set_text(text);
            }
            return;
        }
        if keystroke.modifiers.platform && keystroke.key == "v" {
            if let Some(clipboard) = &mut self.clipboard
                && let Ok(text) = clipboard.get_text()
                && let Some(pane_id) = self.terminal_input_pane_id()
                && let Some(pane) = self.panes.get(&pane_id)
                && let Ok(bytes) = pane.engine.encode_paste(&text)
            {
                self.write_focused(bytes);
            }
            return;
        }

        let pane_id = terminal_key_down_target(
            &self.terminal_key_presses,
            &keystroke.key,
            held,
            self.terminal_input_pane_id(),
        );
        let Some(pane_id) = pane_id else {
            return;
        };
        if self.send_terminal_key_to(pane_id, keystroke, false, held, caps_lock) && !held {
            self.terminal_key_presses
                .insert(keystroke.key.clone(), pane_id);
        }
    }

    fn send_terminal_key_to(
        &self,
        pane_id: PaneId,
        keystroke: &gpui::Keystroke,
        release: bool,
        held: bool,
        caps_lock: bool,
    ) -> bool {
        let Some(pane) = self.panes.get(&pane_id) else {
            return false;
        };
        let terminal_event = terminal_key_event(keystroke, release, held, caps_lock);
        match pane.engine.encode_key(&terminal_event) {
            Ok(bytes) => {
                if !bytes.is_empty() {
                    self.backend.send(CommandMessage::Write { pane_id, bytes });
                }
                true
            }
            Err(error) => {
                error!(%error, "encode terminal key");
                false
            }
        }
    }

    fn forward_terminal_tab(&mut self, shift: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_agent_pane() == self.terminal_input_pane_id() {
            return;
        }
        let keystroke = terminal_tab_keystroke(shift);
        self.terminal_key_presses.remove(&keystroke.key);
        self.send_terminal_key_down(&keystroke, false, window.capslock().on);
        cx.stop_propagation();
    }

    fn on_forward_terminal_tab(
        &mut self,
        _: &ForwardTerminalTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.forward_terminal_tab(false, window, cx);
    }

    fn on_forward_terminal_backtab(
        &mut self,
        _: &ForwardTerminalBacktab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.forward_terminal_tab(true, window, cx);
    }

    fn perform_action(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            Action::Sequence(actions) => {
                for action in actions {
                    self.perform_action(action, window, cx);
                }
            }
            Action::EnterMode(mode) => self.mode = mode,
            Action::WriteTerminal(bytes) => self.write_focused(bytes),
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
            Action::RenameTab => self.open_rename_tab(window, cx),
            Action::SelectTab(number) => {
                if let Some(tab) = self
                    .session
                    .as_ref()
                    .and_then(|session| session.tabs.get(usize::from(number.saturating_sub(1))))
                {
                    self.send_workspace(WorkspaceCommand::SelectTab(tab.id));
                }
            }
            Action::NextTab => self.send_workspace(WorkspaceCommand::NextTab),
            Action::PreviousTab => self.send_workspace(WorkspaceCommand::PreviousTab),
            Action::OpenSessionSwitcher => self.open_sessions(window, cx),
            Action::DetachSession => cx.quit(),
            Action::OpenAgentSurface => self.toggle_agents(window, cx),
            Action::OpenSettings => self.open_settings(window, cx),
            Action::OpenCommandPalette => window.push_notification(
                Notification::info("Command palette is being moved to GPUI"),
                cx,
            ),
        }
        cx.notify();
    }

    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        info!("opening GPUI settings dialog");
        let theme = BezelTheme::of(cx).clone();
        let profiles = self.profiles.clone();
        let settings_path = self
            .state_dir
            .as_ref()
            .map(|path| path.join("settings.json"));
        let app = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _window, cx| {
            let mut content = v_flex()
                .gap_2()
                .font_family(theme.font_sans.clone());
            content = content.child(
                h_flex()
                    .items_start()
                    .gap_2()
                    .pb_1()
                    .child(
                        bezel_icons::icon(bezel_icons::TUNING)
                            .size(px(16.0))
                            .text_color(theme.accent),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .whitespace_normal()
                            .text_sm()
                            .line_height(px(20.0))
                            .text_color(theme.text_muted)
                            .child("ACP agents run out of process. Custom agents use Zed-compatible agent_servers entries; restart Mux after editing the file."),
                    ),
            );
            if let Some(settings_path) = settings_path.clone() {
                content = content.child(agent_settings_file_row(&app, &settings_path, &theme));
            }
            for profile in &profiles {
                let profile_id = profile.id.clone();
                let enabled = app
                    .upgrade()
                    .is_some_and(|entity| entity.read(cx).settings.agent_enabled(&profile_id));
                let toggle_app = app.clone();
                let title = profile.name.clone();
                let description = profile.description.clone();
                content = content.child(
                    h_flex()
                        .justify_between()
                        .gap_4()
                        .p_3()
                        .rounded(px(BezelTheme::SURFACE_RADIUS))
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.surface_card)
                        .child(
                            v_flex()
                                .gap_1()
                                .child(div().text_sm().font_semibold().child(title))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.text_muted)
                                        .child(description),
                                ),
                        )
                        .child(
                            Switch::new(SharedString::from(profile_id.clone()))
                                .checked(enabled)
                                .on_click(move |enabled, _window, cx| {
                                    let _ = toggle_app.update(cx, |this, cx| {
                                        this.settings.set_agent_enabled(&profile_id, *enabled);
                                        if let Some(state_dir) = &this.state_dir
                                            && let Err(error) = this.settings.save(state_dir)
                                        {
                                            error!(%error, "save settings");
                                        }
                                        cx.notify();
                                    });
                                }),
                        ),
                );
            }
            dialog.title("Settings").w(px(560.0)).child(content)
        });
    }

    fn open_sessions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.backend.send(CommandMessage::ListSessions);
        let app = cx.weak_entity();
        let active_session_id = self.session.as_ref().map(|session| session.id);
        window.open_dialog(cx, move |dialog, _window, cx| {
            let create_app = app.clone();
            let theme = BezelTheme::of(cx).clone();
            let mut content = v_flex().gap_3().font_family(theme.font_sans.clone()).child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_start()
                    .gap_2()
                    .child(theme.row_tile(bezel_icons::CLOUD))
                    .child(
                        v_flex()
                            .min_w_0()
                            .flex_1()
                            .child(theme.row_title("Durable workspaces"))
                            .child(theme.meta_line(vec![
                                    div()
                                        .child("Switch without stopping shells, panes, or agents.")
                                        .into_any_element(),
                                ])),
                    )
                    .child(
                        Button::new("session-new")
                            .icon(IconName::Plus)
                            .label("New")
                            .secondary()
                            .small()
                            .compact()
                            .tooltip("Move the focused pane into a new session")
                            .on_click(move |_, window, cx| {
                                let _ = create_app.update(cx, |this, _| {
                                    if let Some(pane_id) = this.focused_pane_id() {
                                        let mut number = this.sessions.len() + 1;
                                        let name = loop {
                                            let candidate = format!("session-{number}");
                                            if !this
                                                .sessions
                                                .iter()
                                                .any(|session| session.name == candidate)
                                            {
                                                break candidate;
                                            }
                                            number += 1;
                                        };
                                        this.backend.send(CommandMessage::CreateSessionForPane {
                                            name,
                                            pane_id,
                                        });
                                    }
                                });
                                window.close_dialog(cx);
                            }),
                    ),
            );
            let sessions = app
                .upgrade()
                .map(|entity| entity.read(cx).sessions.clone())
                .unwrap_or_default();
            if sessions.is_empty() {
                content = content.child(
                    div()
                        .w_full()
                        .p_3()
                        .rounded(px(BezelTheme::SURFACE_RADIUS))
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.surface_card)
                        .text_sm()
                        .text_color(theme.text_muted)
                        .child("Loading sessions…"),
                );
            } else {
                let mut group = theme.group_box();
                for (index, session) in sessions.iter().enumerate() {
                    group = group.child(Self::session_switcher_row(
                        &app,
                        session,
                        active_session_id,
                        index == 0,
                        &theme,
                    ));
                }
                content = content.child(group);
            }
            dialog.title("Sessions").w(px(560.0)).child(content)
        });
    }

    fn session_switcher_row(
        app: &gpui::WeakEntity<MuxApp>,
        session: &SessionSummary,
        active_session_id: Option<SessionId>,
        first: bool,
        theme: &BezelTheme,
    ) -> gpui::AnyElement {
        let session_id = session.id;
        let active = active_session_id == Some(session_id);
        let badge = if active {
            theme.badge_active("CURRENT").into_any_element()
        } else {
            theme.badge("DURABLE").into_any_element()
        };
        let actions = Self::session_switcher_actions(app, session, active);
        theme
            .card_row(first)
            .child(theme.row_tile(bezel_icons::MONITOR))
            .child(
                v_flex()
                    .min_w_0()
                    .flex_1()
                    .child(
                        h_flex()
                            .min_w_0()
                            .gap_2()
                            .child(theme.row_title(session.name.clone()))
                            .child(badge),
                    )
                    .child(theme.meta_line(vec![
                        div()
                            .child(format!(
                                "{} · owned by muxd",
                                format_session_pane_count(session.pane_count)
                            ))
                            .into_any_element(),
                    ])),
            )
            .child(actions)
            .into_any_element()
    }

    fn session_switcher_actions(
        app: &gpui::WeakEntity<MuxApp>,
        session: &SessionSummary,
        active: bool,
    ) -> gpui::AnyElement {
        let attach_app = app.clone();
        let rename_app = app.clone();
        let kill_app = app.clone();
        let session_id = session.id;
        let rename_session = session.clone();
        let kill_name = session.name.clone();
        h_flex()
            .flex_none()
            .gap_1()
            .when(!active, |actions| {
                actions.child(
                    Button::new(SharedString::from(format!("session-{session_id}")))
                        .label("Open")
                        .ghost()
                        .small()
                        .compact()
                        .on_click(move |_, window, cx| {
                            let _ = attach_app.update(cx, |this, _| {
                                this.backend.send(CommandMessage::AttachSession(session_id));
                            });
                            window.close_dialog(cx);
                        }),
                )
            })
            .child(
                Button::new(SharedString::from(format!("rename-session-{session_id}")))
                    .icon(IconName::ALargeSmall)
                    .label("Rename")
                    .ghost()
                    .small()
                    .compact()
                    .on_click(move |_, window, cx| {
                        open_rename_session_dialog(rename_app.clone(), &rename_session, window, cx);
                    }),
            )
            .child(
                Button::new(SharedString::from(format!("kill-session-{session_id}")))
                    .icon(IconName::Delete)
                    .label("End")
                    .danger()
                    .small()
                    .compact()
                    .on_click(move |_, window, cx| {
                        let confirm_app = kill_app.clone();
                        let kill_name = kill_name.clone();
                        window.open_alert_dialog(cx, move |dialog, _, _| {
                            let confirm_app = confirm_app.clone();
                            dialog
                                .title(format!("End {kill_name}?"))
                                .button_props(
                                    DialogButtonProps::default()
                                        .ok_text("End session")
                                        .ok_variant(ButtonVariant::Danger)
                                        .cancel_text("Keep session")
                                        .show_cancel(true),
                                )
                                .on_ok(move |_, window, cx| {
                                    let _ = confirm_app.update(cx, |this, _| {
                                        this.backend.send(CommandMessage::KillSession(session_id));
                                    });
                                    window.close_all_dialogs(cx);
                                    true
                                })
                                .child(
                                    div()
                                        .text_sm()
                                        .child("All processes in this durable session will exit."),
                                )
                        });
                    }),
            )
            .into_any_element()
    }

    fn open_rename_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self
            .session
            .as_ref()
            .and_then(Session::active_tab)
            .map(|tab| tab.title.clone())
            .unwrap_or_default();
        let input = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("Tab name");
            input.set_value(current, window, cx);
            input
        });
        let submit_input = input.clone();
        let app = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _window, _cx| {
            let submit_app = app.clone();
            let input_for_button = submit_input.clone();
            dialog.title("Rename tab").w(px(420.0)).child(
                v_flex().gap_3().child(Input::new(&submit_input)).child(
                    Button::new("rename-tab-submit")
                        .label("Rename")
                        .primary()
                        .on_click(move |_, window, cx| {
                            let value = input_for_button.read(cx).value().to_string();
                            if !value.trim().is_empty() {
                                let _ = submit_app.update(cx, |this, _| {
                                    this.send_workspace(WorkspaceCommand::RenameTab(value));
                                    this.mode = InputMode::Normal;
                                });
                                window.close_dialog(cx);
                            }
                        }),
                ),
            )
        });
    }

    fn toggle_agents(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_agent_pane(window, cx);
    }

    fn submit_agent_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let draft = self.agent_input.read(cx).value().to_string();
        let draft = draft.trim();
        if draft.is_empty() {
            return;
        }
        if self.handle_agent_slash_command(draft, window, cx) {
            self.agent_input.update(cx, |input, cx| {
                input.set_value("", window, cx);
            });
            return;
        }
        if let Some(tab_id) = self.active_tab_id() {
            self.agent_help_tabs.remove(&tab_id);
        }
        let Some(session_id) = self
            .active_agent()
            .filter(|agent| agent.status != AgentSessionStatus::Closed)
            .map(|agent| agent.id)
        else {
            if self.pending_agent_prompt.is_some() {
                window.push_notification(Notification::info("Agent is starting…"), cx);
                return;
            }
            let Some(profile) = self.enabled_profiles().next().cloned() else {
                window.push_notification(
                    Notification::warning("Enable an ACP agent in Settings first"),
                    cx,
                );
                return;
            };
            let prompt = AgentPrompt {
                text: draft.to_owned(),
                context: self.agent_prompt_context().unwrap_or_default(),
                files: self
                    .agent_completion
                    .reference_paths(draft)
                    .into_iter()
                    .map(|path| mux_acp::AgentFileReference {
                        path,
                        text: String::new(),
                    })
                    .collect(),
            };
            self.pending_agent_prompt = Some(prompt);
            self.follow_active_agent_tail();
            self.start_agent(profile, None);
            self.agent_input.update(cx, |input, cx| {
                input.set_value("", window, cx);
            });
            return;
        };
        let context = self.agent_prompt_context().unwrap_or_default();
        self.follow_active_agent_tail();
        self.backend.send(CommandMessage::PromptAgent {
            session_id,
            prompt: AgentPrompt {
                text: draft.to_owned(),
                context,
                files: self
                    .agent_completion
                    .reference_paths(draft)
                    .into_iter()
                    .map(|path| mux_acp::AgentFileReference {
                        path,
                        text: String::new(),
                    })
                    .collect(),
            },
        });
        self.agent_input.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
    }

    fn handle_agent_slash_command(
        &mut self,
        draft: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(command) = draft.strip_prefix('/') else {
            return false;
        };
        let mut parts = command.split_whitespace();
        match parts.next().unwrap_or_default() {
            "new" => {
                let requested = parts.next();
                let profile = requested.map_or_else(
                    || self.enabled_profiles().next(),
                    |id| {
                        self.enabled_profiles()
                            .find(|profile| profile.id.eq_ignore_ascii_case(id))
                    },
                );
                if let Some(profile) = profile.cloned() {
                    let cwd = parts.collect::<Vec<_>>().join(" ");
                    self.start_agent(profile, parse_cwd_override(&cwd));
                } else {
                    window.push_notification(
                        Notification::warning("Unknown or disabled ACP agent"),
                        cx,
                    );
                }
            }
            "next" => self.select_relative_agent(1),
            "prev" | "previous" => self.select_relative_agent(-1),
            "use" => self.select_agent(parts.next(), window, cx),
            "end" | "close" => {
                if let Some(agent) = self.active_agent() {
                    self.backend.send(CommandMessage::CloseAgent(agent.id));
                }
            }
            "cancel" => {
                if let Some(agent) = self.active_agent() {
                    self.backend.send(CommandMessage::CancelAgent(agent.id));
                }
            }
            "context" => {
                self.agent_context = match parts.next() {
                    Some("none" | "off") => AgentContextMode::None,
                    Some("tab" | "panes" | "pane" | "screen" | "on") | None => {
                        AgentContextMode::Tab
                    }
                    Some(_) => {
                        window.push_notification(
                            Notification::warning("Usage: /context tab|none"),
                            cx,
                        );
                        return true;
                    }
                };
            }
            "effort" | "reasoning" => {
                self.set_agent_option(AgentConfigCategory::ThoughtLevel, parts.next(), window, cx);
            }
            "model" => {
                self.set_agent_option(AgentConfigCategory::Model, parts.next(), window, cx);
            }
            "mode" => self.set_agent_mode(parts.next(), window, cx),
            "login" => self.authenticate_agent(parts.next(), window, cx),
            "allow" => self.resolve_agent_permission(true, parts.next(), window, cx),
            "deny" | "reject" => {
                self.resolve_agent_permission(false, parts.next(), window, cx);
            }
            "expand" | "details" => {
                self.set_agent_detail_expansion(true, parts.next() == Some("all"));
            }
            "collapse" => {
                self.set_agent_detail_expansion(false, parts.next() == Some("all"));
            }
            "help" => {
                if let Some(tab_id) = self.active_tab_id() {
                    self.agent_help_tabs.insert(tab_id);
                    self.follow_active_agent_tail();
                    cx.notify();
                }
            }
            _ => return false,
        }
        true
    }

    fn set_agent_detail_expansion(&mut self, expanded: bool, all: bool) {
        let Some(agent) = self.active_agent() else {
            return;
        };
        let mut keys = agent
            .timeline
            .iter()
            .enumerate()
            .filter(|(_, item)| is_expandable_agent_item(item))
            .map(|(index, item)| agent_item_key(agent, index, item))
            .collect::<Vec<_>>();
        if !all {
            keys = keys.into_iter().rev().take(1).collect();
        }
        for key in keys {
            if expanded {
                self.expanded_agent_items.insert(key);
            } else {
                self.expanded_agent_items.remove(&key);
            }
        }
    }

    fn select_relative_agent(&mut self, delta: isize) {
        let agents = self
            .agents_for_active_tab()
            .map(|agent| agent.id)
            .collect::<Vec<_>>();
        if agents.is_empty() {
            return;
        }
        let selected = self.active_agent().map(|agent| agent.id);
        let index = selected
            .and_then(|selected| agents.iter().position(|agent| *agent == selected))
            .unwrap_or(0);
        let next = if delta < 0 {
            index.checked_sub(1).unwrap_or(agents.len() - 1)
        } else if delta > 0 && index + 1 == agents.len() {
            0
        } else if delta > 0 {
            index + 1
        } else {
            index
        };
        self.select_active_tab_agent(Some(agents[next]));
        self.follow_active_agent_tail();
    }

    fn select_agent(
        &mut self,
        requested: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(requested) = requested else {
            let sessions = self
                .agents_for_active_tab()
                .enumerate()
                .map(|(index, agent)| format!("{}:{}", index + 1, agent.name))
                .collect::<Vec<_>>()
                .join(" · ");
            window.push_notification(Notification::info(format!("Sessions: {sessions}")), cx);
            return;
        };
        let agents = self.agents_for_active_tab().collect::<Vec<_>>();
        let index = requested
            .parse::<usize>()
            .ok()
            .and_then(|index| index.checked_sub(1))
            .filter(|index| *index < agents.len())
            .or_else(|| {
                agents.iter().position(|agent| {
                    agent.name.eq_ignore_ascii_case(requested)
                        || agent
                            .agent_name
                            .as_deref()
                            .is_some_and(|name| name.eq_ignore_ascii_case(requested))
                })
            });
        if let Some(index) = index {
            let session_id = agents[index].id;
            self.select_active_tab_agent(Some(session_id));
            self.follow_active_agent_tail();
        } else {
            window.push_notification(Notification::warning("Unknown agent session"), cx);
        }
    }

    fn authenticate_agent(
        &self,
        requested: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(agent) = self.active_agent() else {
            return;
        };
        let method = requested
            .and_then(|requested| {
                agent.auth_methods.iter().find(|method| {
                    method.id.eq_ignore_ascii_case(requested)
                        || method.name.eq_ignore_ascii_case(requested)
                })
            })
            .or_else(|| agent.auth_methods.first());
        if let Some(method) = method {
            self.backend.send(CommandMessage::AuthenticateAgent {
                session_id: agent.id,
                method_id: method.id.clone(),
            });
        } else {
            window.push_notification(
                Notification::warning("This agent is not asking for authentication"),
                cx,
            );
        }
    }

    fn resolve_agent_permission(
        &self,
        allow: bool,
        requested: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(agent) = self.active_agent() else {
            return;
        };
        let Some(permission) = agent.pending_permission() else {
            window.push_notification(Notification::warning("No permission is waiting"), cx);
            return;
        };
        let always = requested.is_some_and(|value| value.eq_ignore_ascii_case("always"));
        let kind = match (allow, always) {
            (true, false) => mux_acp::PermissionKind::AllowOnce,
            (true, true) => mux_acp::PermissionKind::AllowAlways,
            (false, false) => mux_acp::PermissionKind::RejectOnce,
            (false, true) => mux_acp::PermissionKind::RejectAlways,
        };
        let Some(option) = permission.options.iter().find(|option| option.kind == kind) else {
            window.push_notification(
                Notification::warning("The agent did not offer that permission choice"),
                cx,
            );
            return;
        };
        self.backend.send(CommandMessage::ResolveAgentPermission {
            session_id: agent.id,
            request_id: permission.request_id.clone(),
            option_id: Some(option.id.clone()),
        });
    }

    fn set_agent_option(
        &self,
        category: AgentConfigCategory,
        requested: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(agent) = self.active_agent() else {
            return;
        };
        let Some(option) = agent
            .config_options
            .iter()
            .find(|option| option.category == category)
        else {
            window.push_notification(
                Notification::warning("This agent does not expose that option"),
                cx,
            );
            return;
        };
        let Some(requested) = requested else {
            window.push_notification(Notification::info(describe_agent_option(option)), cx);
            return;
        };
        let selection = match &option.value {
            AgentConfigValue::Select { choices, .. } => choices
                .iter()
                .find(|choice| {
                    choice.id.eq_ignore_ascii_case(requested)
                        || choice.name.eq_ignore_ascii_case(requested)
                })
                .map(|choice| AgentConfigValueSelection::Choice(choice.id.clone())),
            AgentConfigValue::Boolean(_) => match requested {
                "on" | "true" | "1" => Some(AgentConfigValueSelection::Boolean(true)),
                "off" | "false" | "0" => Some(AgentConfigValueSelection::Boolean(false)),
                _ => None,
            },
        };
        if let Some(value) = selection {
            self.backend.send(CommandMessage::SetAgentConfig {
                session_id: agent.id,
                config_id: option.id.clone(),
                value,
            });
        } else {
            window.push_notification(Notification::warning(describe_agent_option(option)), cx);
        }
    }

    fn set_agent_mode(&self, requested: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(agent) = self.active_agent() else {
            return;
        };
        let Some(requested) = requested else {
            let values = agent
                .modes
                .iter()
                .map(|mode| mode.id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            window.push_notification(
                Notification::info(format!(
                    "Mode: {} · choices: {values}",
                    agent.current_mode.as_deref().unwrap_or("default")
                )),
                cx,
            );
            return;
        };
        if let Some(mode) = agent.modes.iter().find(|mode| {
            mode.id.eq_ignore_ascii_case(requested) || mode.name.eq_ignore_ascii_case(requested)
        }) {
            self.backend.send(CommandMessage::SetAgentMode {
                session_id: agent.id,
                mode_id: mode.id.clone(),
            });
        } else {
            window.push_notification(Notification::warning("Unknown agent mode"), cx);
        }
    }

    fn enabled_profiles(&self) -> impl Iterator<Item = &AgentProfile> {
        self.profiles
            .iter()
            .filter(|profile| self.settings.agent_enabled(&profile.id))
    }

    fn agent_command_arguments(&self) -> Vec<AgentCommandArgument> {
        let mut arguments = self
            .enabled_profiles()
            .map(|profile| AgentCommandArgument {
                command: "new".to_owned(),
                value: profile.id.clone(),
                detail: "Agent".to_owned(),
                description: profile.description.clone(),
            })
            .collect::<Vec<_>>();
        arguments.extend(["tab", "none"].into_iter().map(|value| {
            AgentCommandArgument {
                command: "context".to_owned(),
                value: value.to_owned(),
                detail: "Context".to_owned(),
                description: if value == "tab" {
                    "Attach terminal context from the other panes in this tab"
                } else {
                    "Do not attach terminal context"
                }
                .to_owned(),
            }
        }));
        arguments.extend(
            self.agents_for_active_tab()
                .enumerate()
                .map(|(index, agent)| AgentCommandArgument {
                    command: "use".to_owned(),
                    value: (index + 1).to_string(),
                    detail: "Session".to_owned(),
                    description: agent
                        .agent_name
                        .clone()
                        .unwrap_or_else(|| agent.name.clone()),
                }),
        );
        if let Some(agent) = self.active_agent() {
            arguments.extend(agent.modes.iter().map(|mode| {
                AgentCommandArgument {
                    command: "mode".to_owned(),
                    value: mode.id.clone(),
                    detail: "Mode".to_owned(),
                    description: mode
                        .description
                        .clone()
                        .unwrap_or_else(|| mode.name.clone()),
                }
            }));
            for option in &agent.config_options {
                let command = match option.category {
                    AgentConfigCategory::Model => "model",
                    AgentConfigCategory::ThoughtLevel => "effort",
                    AgentConfigCategory::Mode
                    | AgentConfigCategory::ModelConfig
                    | AgentConfigCategory::Other => continue,
                };
                let AgentConfigValue::Select { choices, .. } = &option.value else {
                    continue;
                };
                arguments.extend(choices.iter().map(|choice| {
                    AgentCommandArgument {
                        command: command.to_owned(),
                        value: choice.id.clone(),
                        detail: option.name.clone(),
                        description: choice
                            .description
                            .clone()
                            .unwrap_or_else(|| choice.name.clone()),
                    }
                }));
            }
            arguments.extend(agent.auth_methods.iter().map(|method| {
                AgentCommandArgument {
                    command: "login".to_owned(),
                    value: method.id.clone(),
                    detail: "Auth".to_owned(),
                    description: method
                        .description
                        .clone()
                        .unwrap_or_else(|| method.name.clone()),
                }
            }));
        }
        arguments
    }

    fn start_agent(&self, profile: AgentProfile, cwd_override: Option<PathBuf>) {
        if let Some(pane_id) = self.focused_pane_id() {
            self.backend.send(CommandMessage::StartAgent {
                spec: profile.spec,
                pane_id,
                cwd_override,
            });
        }
    }

    fn agent_prompt_context(&self) -> Result<Vec<AgentContext>> {
        if self.agent_context == AgentContextMode::None {
            return Ok(Vec::new());
        }
        let tab = self
            .session
            .as_ref()
            .and_then(Session::active_tab)
            .ok_or_else(|| anyhow!("No active tab is available"))?;
        let agent_pane = self.agent_panes.get(&tab.id).copied();
        let mut pane_ids = Vec::new();
        tab.layout.pane_ids(&mut pane_ids);
        let pane_ids = pane_ids
            .into_iter()
            .filter(|pane_id| Some(*pane_id) != agent_pane)
            .collect::<Vec<_>>();
        let pane_count = pane_ids.len();
        Ok(pane_ids
            .into_iter()
            .enumerate()
            .filter_map(|(index, pane_id)| {
                self.panes.get(&pane_id).map(|pane| AgentContext {
                    kind: AgentContextKind::TerminalViewport,
                    pane_id,
                    label: format!("terminal pane {} of {pane_count} in active tab", index + 1),
                    text: terminal_frame_text(&pane.frame),
                })
            })
            .collect())
    }

    fn selection_pointer(
        &self,
        rect: layout::Rect,
        frame: &RenderFrame,
        position: gpui::Point<gpui::Pixels>,
    ) -> SelectionPointer {
        let x = f32::from(position.x);
        let y = f32::from(position.y);
        let padding =
            self.metrics
                .balanced_padding(rect.width, rect.height, frame.cols, frame.rows);
        let grid_x = x - rect.x - padding.left;
        let grid_y = y - rect.y - padding.top;
        let max_column = frame.cols.saturating_sub(1);
        let max_row = frame.rows.saturating_sub(1);
        let clamped_point = TerminalPoint {
            column: (grid_x / self.metrics.cell_width)
                .floor()
                .clamp(0.0, f32::from(max_column)) as u16,
            row: (grid_y / self.metrics.cell_height)
                .floor()
                .clamp(0.0, f32::from(max_row)) as u16,
        };
        let inside = grid_x >= 0.0
            && grid_y >= 0.0
            && grid_x < self.metrics.cell_width * f32::from(frame.cols)
            && grid_y < self.metrics.cell_height * f32::from(frame.rows);
        SelectionPointer {
            point: inside.then_some(clamped_point),
            clamped_point,
            position: TerminalSurfacePosition {
                x: f64::from(x - rect.x),
                y: f64::from(y - rect.y),
            },
            geometry: TerminalSelectionGeometry {
                columns: u32::from(frame.cols),
                cell_width: self.metrics.cell_width.round().max(1.0) as u32,
                padding_left: padding.left.round().max(0.0) as u32,
                screen_height: rect.height.round().max(1.0) as u32,
            },
        }
    }

    fn apply_selection_gesture(
        &mut self,
        pane_id: PaneId,
        event: TerminalSelectionGestureEvent,
    ) -> Result<()> {
        let pane = self
            .panes
            .get_mut(&pane_id)
            .ok_or_else(|| anyhow!("selection pane is unavailable"))?;
        let status = pane.engine.selection_gesture(event)?;
        pane.publish_frame()?;
        self.selected_pane = status.has_selection.then_some(pane_id);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn report_mouse_event(
        &mut self,
        pane_id: PaneId,
        rect: layout::Rect,
        position: gpui::Point<gpui::Pixels>,
        action: TerminalMouseAction,
        button: Option<TerminalMouseButton>,
        modifiers: gpui::Modifiers,
        any_button_pressed: bool,
    ) -> bool {
        let Some(frame) = self.panes.get(&pane_id).map(|pane| Rc::clone(&pane.frame)) else {
            return false;
        };
        let padding =
            self.metrics
                .balanced_padding(rect.width, rect.height, frame.cols, frame.rows);
        let event = TerminalMouseEvent {
            action,
            button,
            modifiers: terminal_modifiers(modifiers),
            x: f32::from(position.x) - rect.x,
            y: f32::from(position.y) - rect.y,
            geometry: TerminalMouseGeometry {
                screen_width: rect.width.round().max(1.0) as u32,
                screen_height: rect.height.round().max(1.0) as u32,
                cell_width: self.metrics.cell_width.round().max(1.0) as u32,
                cell_height: self.metrics.cell_height.round().max(1.0) as u32,
                padding_top: padding.top.round().max(0.0) as u32,
                padding_bottom: padding.bottom.round().max(0.0) as u32,
                padding_right: padding.right.round().max(0.0) as u32,
                padding_left: padding.left.round().max(0.0) as u32,
            },
            any_button_pressed,
        };
        let bytes = self
            .panes
            .get_mut(&pane_id)
            .and_then(|pane| pane.engine.encode_mouse(&event).ok())
            .unwrap_or_default();
        if bytes.is_empty() {
            false
        } else {
            self.backend.send(CommandMessage::Write { pane_id, bytes });
            true
        }
    }

    fn pointer_down(&mut self, pane_id: PaneId, rect: layout::Rect, event: &gpui::MouseDownEvent) {
        self.request_pane_focus(pane_id);
        let button = terminal_mouse_button(event.button);
        if !event.modifiers.shift
            && self.report_mouse_event(
                pane_id,
                rect,
                event.position,
                TerminalMouseAction::Press,
                Some(button),
                event.modifiers,
                true,
            )
        {
            self.mouse_reporting = Some(TerminalPointerCapture { pane_id, rect });
            self.selection_drag = None;
            return;
        }
        if event.button == gpui::MouseButton::Left {
            self.begin_selection(pane_id, rect, event);
        }
    }

    fn pointer_move(&mut self, event: &gpui::MouseMoveEvent) -> bool {
        if let Some(capture) = self.mouse_reporting {
            let _ = self.report_mouse_event(
                capture.pane_id,
                capture.rect,
                event.position,
                TerminalMouseAction::Motion,
                event.pressed_button.map(terminal_mouse_button),
                event.modifiers,
                event.pressed_button.is_some(),
            );
            return true;
        }
        let Some(capture) = self.selection_drag else {
            return false;
        };
        self.drag_selection(capture, event);
        true
    }

    fn pointer_hover(
        &mut self,
        pane_id: PaneId,
        rect: layout::Rect,
        event: &gpui::MouseMoveEvent,
    ) -> bool {
        if self.mouse_reporting.is_some() || self.selection_drag.is_some() || event.modifiers.shift
        {
            return false;
        }
        self.report_mouse_event(
            pane_id,
            rect,
            event.position,
            TerminalMouseAction::Motion,
            event.pressed_button.map(terminal_mouse_button),
            event.modifiers,
            event.pressed_button.is_some(),
        )
    }

    fn pointer_up(&mut self, event: &gpui::MouseUpEvent) -> bool {
        if let Some(capture) = self.mouse_reporting.take() {
            let _ = self.report_mouse_event(
                capture.pane_id,
                capture.rect,
                event.position,
                TerminalMouseAction::Release,
                Some(terminal_mouse_button(event.button)),
                event.modifiers,
                false,
            );
            return true;
        }
        if event.button == gpui::MouseButton::Left
            && let Some(capture) = self.selection_drag.take()
        {
            self.end_selection(capture, event);
            return true;
        }
        false
    }

    fn begin_selection(
        &mut self,
        pane_id: PaneId,
        rect: layout::Rect,
        event: &gpui::MouseDownEvent,
    ) {
        if self.selected_pane != Some(pane_id)
            && let Some(previous) = self.selected_pane.take()
            && let Some(pane) = self.panes.get_mut(&previous)
        {
            let _ = pane.engine.set_selection(None);
            let _ = pane.publish_frame();
        }
        let Some(frame) = self.panes.get(&pane_id).map(|pane| Rc::clone(&pane.frame)) else {
            return;
        };
        let pointer = self.selection_pointer(rect, &frame, event.position);
        let Some(point) = pointer.point else {
            return;
        };
        let time_ns = Instant::now()
            .duration_since(self.selection_clock_origin)
            .as_nanos()
            .min(u128::from(u64::MAX)) as u64;
        if self
            .apply_selection_gesture(
                pane_id,
                TerminalSelectionGestureEvent::Press {
                    point,
                    position: pointer.position,
                    time_ns,
                    repeat_distance: f64::from(self.metrics.cell_width),
                    repeat_interval_ns: 500_000_000,
                },
            )
            .is_ok()
        {
            self.selection_drag = Some(TerminalPointerCapture { pane_id, rect });
        }
    }

    fn drag_selection(&mut self, capture: TerminalPointerCapture, event: &gpui::MouseMoveEvent) {
        if !event.dragging() {
            return;
        }
        let TerminalPointerCapture { pane_id, rect } = capture;
        let Some(frame) = self.panes.get(&pane_id).map(|pane| Rc::clone(&pane.frame)) else {
            return;
        };
        let pointer = self.selection_pointer(rect, &frame, event.position);
        let _ = self.apply_selection_gesture(
            pane_id,
            TerminalSelectionGestureEvent::Drag {
                point: pointer.clamped_point,
                position: pointer.position,
                rectangular: event.modifiers.alt,
                geometry: pointer.geometry,
            },
        );
    }

    fn end_selection(&mut self, capture: TerminalPointerCapture, event: &gpui::MouseUpEvent) {
        let TerminalPointerCapture { pane_id, rect } = capture;
        let point = self.panes.get(&pane_id).and_then(|pane| {
            self.selection_pointer(rect, &pane.frame, event.position)
                .point
        });
        let _ =
            self.apply_selection_gesture(pane_id, TerminalSelectionGestureEvent::Release { point });
    }

    fn scroll_pane(
        &mut self,
        pane_id: PaneId,
        rect: layout::Rect,
        event: &gpui::ScrollWheelEvent,
    ) -> bool {
        let (delta_rows, reset_fraction) = match event.delta {
            gpui::ScrollDelta::Lines(delta) => (-delta.y * 3.0, true),
            gpui::ScrollDelta::Pixels(delta) => (
                -f32::from(delta.y) / self.metrics.cell_height,
                matches!(event.touch_phase, gpui::TouchPhase::Started),
            ),
        };
        let rows = self
            .pane_scrolls
            .entry(pane_id)
            .or_default()
            .accumulate(delta_rows, reset_fraction);
        if rows == 0 {
            return false;
        }
        let wheel_button = if rows < 0 {
            TerminalMouseButton::Four
        } else {
            TerminalMouseButton::Five
        };
        if !event.modifiers.shift {
            let mut reported = false;
            for _ in 0..rows.unsigned_abs() {
                reported |= self.report_mouse_event(
                    pane_id,
                    rect,
                    event.position,
                    TerminalMouseAction::Press,
                    Some(wheel_button),
                    event.modifiers,
                    false,
                );
            }
            if reported {
                return false;
            }
        }

        let Some(pane) = self.panes.get_mut(&pane_id) else {
            self.pane_scrolls.remove(&pane_id);
            return false;
        };
        if let Err(error) = pane
            .engine
            .scroll_viewport(TerminalViewportScroll::Delta(rows))
        {
            error!(pane_id = %pane_id, %error, "could not scroll terminal viewport");
            return false;
        }
        if let Err(error) = pane.publish_frame() {
            error!(pane_id = %pane_id, %error, "could not render scrolled terminal viewport");
            return false;
        }
        true
    }

    fn sync_terminal_sizes(&mut self, width: f32, height: f32) -> layout::WorkspaceGeometry {
        let Some(session) = &self.session else {
            return layout::WorkspaceGeometry::default();
        };
        let geometry = layout::calculate(session, width, height);
        let sizes = terminal_sizes_for_geometry(&geometry, self.metrics);
        for (pane_id, size) in sizes {
            if self.sent_sizes.get(&pane_id) != Some(&size) {
                let Some(replica) = self.panes.get_mut(&pane_id) else {
                    continue;
                };
                let resized = replica.engine.resize(size);
                if let Err(error) = resized {
                    error!(%pane_id, %error, "could not resize terminal replica");
                    continue;
                }
                if let Err(error) = replica.publish_frame() {
                    error!(%pane_id, %error, "could not render resized terminal replica");
                    continue;
                }
                self.sent_sizes.insert(pane_id, size);
                self.backend.send(CommandMessage::Resize { pane_id, size });
            }
        }
        geometry
    }

    #[allow(clippy::too_many_lines)]
    fn render_agent_pane(
        &mut self,
        pane_id: PaneId,
        rect: layout::Rect,
        focused: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let Some(tab_id) = self.active_tab_id() else {
            return gpui::Empty.into_any_element();
        };
        let app = cx.weak_entity();
        let scroll = self.agent_scroll_for(tab_id);
        let follow_tail = self.agent_follow_tail.contains(&tab_id);
        let settle_scroll = self.agent_scroll_needs_settle.remove(&tab_id);
        let active_agent_id = self.active_agent().map(|agent| agent.id);
        let command_arguments = self.agent_command_arguments();
        self.agent_completion.set_agent_commands(
            active_agent_id
                .and_then(|id| self.agents.iter().find(|agent| agent.id == id))
                .map_or_else(Vec::new, |agent| agent.available_commands.clone()),
        );
        self.agent_completion
            .set_command_arguments(command_arguments);
        let show_help = self.agent_help_tabs.contains(&tab_id);
        let theme = BezelTheme::of(cx).clone();
        let other_panes = self
            .session
            .as_ref()
            .and_then(Session::active_tab)
            .map(|tab| {
                let mut pane_ids = Vec::new();
                tab.layout.pane_ids(&mut pane_ids);
                pane_ids.into_iter().filter(|id| *id != pane_id).count()
            })
            .unwrap_or_default();
        let expanded_items = self.expanded_agent_items.clone();
        let picker = (self.agents_for_active_tab().count() > 1)
            .then(|| agent_session_picker(&app, self, &theme).into_any_element());
        let completion_menu = self.agent_completion_menu.clone();
        let completion_open = completion_menu.is_some();
        let composer_value = self.agent_input.read(cx).value();
        let composer_width = rect.width.min(760.0);
        let composer_bottom = agent_composer_height(composer_value.as_ref(), composer_width);
        let composer_ready =
            !composer_value.trim().is_empty() && self.pending_agent_prompt.is_none();
        let agent = active_agent_id.and_then(|id| self.agents.iter().find(|agent| agent.id == id));

        let keyboard_app = app.clone();
        let cancel_app = app.clone();
        let navigate_left_app = app.clone();
        let navigate_right_app = app.clone();
        let navigate_up_app = app.clone();
        let navigate_down_app = app.clone();
        let focus_app = app.clone();
        let mut body = v_flex()
            .key_context("MuxAgentPane")
            .capture_key_down(move |event, window, cx| {
                handle_agent_pane_key_down(&keyboard_app, event, window, cx);
            })
            .on_action(move |_: &CancelAgentTurn, window, cx| {
                cancel_agent_turn(&cancel_app, window, cx);
            })
            .on_action(move |_: &NavigateAgentLeft, window, cx| {
                navigate_agent_pane(&navigate_left_app, Direction::Left, window, cx);
                cx.stop_propagation();
            })
            .on_action(move |_: &NavigateAgentRight, window, cx| {
                navigate_agent_pane(&navigate_right_app, Direction::Right, window, cx);
                cx.stop_propagation();
            })
            .on_action(move |_: &NavigateAgentUp, window, cx| {
                navigate_agent_pane(&navigate_up_app, Direction::Up, window, cx);
                cx.stop_propagation();
            })
            .on_action(move |_: &NavigateAgentDown, window, cx| {
                navigate_agent_pane(&navigate_down_app, Direction::Down, window, cx);
                cx.stop_propagation();
            })
            .on_action({
                let app = app.clone();
                move |_: &SelectPreviousAgentCompletion, _, cx| {
                    let handled = app
                        .update(cx, |this, cx| this.select_agent_completion(-1, cx))
                        .unwrap_or(false);
                    if handled {
                        cx.stop_propagation();
                    }
                }
            })
            .on_action({
                let app = app.clone();
                move |_: &SelectNextAgentCompletion, _, cx| {
                    let handled = app
                        .update(cx, |this, cx| this.select_agent_completion(1, cx))
                        .unwrap_or(false);
                    if handled {
                        cx.stop_propagation();
                    }
                }
            })
            .on_action({
                let app = app.clone();
                move |_: &InsertAgentCompletion, window, cx| {
                    let handled = app
                        .update(cx, |this, cx| {
                            this.accept_agent_completion(None, window, cx)
                        })
                        .unwrap_or(false);
                    if handled {
                        cx.stop_propagation();
                    }
                }
            })
            .on_action({
                let app = app.clone();
                move |_: &DismissAgentCompletion, _, cx| {
                    let handled = app
                        .update(cx, MuxApp::dismiss_agent_completion)
                        .unwrap_or(false);
                    if handled {
                        cx.stop_propagation();
                    }
                }
            })
            .on_action({
                let app = app.clone();
                move |_: &ToggleAgentPane, window, cx| {
                    return_agent_pane(&app, window, cx);
                    cx.stop_propagation();
                }
            })
            .relative()
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .bg(theme.bg)
            .font_family(theme.font_sans.clone())
            .text_color(theme.text)
            .when(!focused, move |body| {
                body.on_any_mouse_down(move |_, window, cx| {
                    let _ = focus_app.update(cx, |this, _cx| {
                        this.request_pane_focus(pane_id);
                        this.focus_agent_composer(window);
                    });
                })
            })
            .child(agent_pane_header(
                &app,
                pane_id,
                agent,
                other_panes + 1,
                &theme,
            ));
        if let Some(picker) = picker {
            body = body.child(picker);
        }
        if let Some(agent) = agent {
            body = body.child(agent_timeline(
                &app,
                tab_id,
                agent,
                &scroll,
                &expanded_items,
                show_help,
                follow_tail,
                settle_scroll,
                self.motion,
                window,
                cx,
            ));
            body = body.child(agent_auth_controls(&app, agent, &theme));
            body = body.child(agent_permission_controls(&app, agent, &theme));
        } else if show_help {
            body = body.child(agent_help_surface(None, &theme));
        } else {
            body = body.child(agent_empty_state(
                self,
                other_panes + 1,
                agent_pane_is_compact(rect.width),
                &theme,
            ));
        }
        body = body.child(
            div()
                .w_full()
                .max_w(px(760.0))
                .mx_auto()
                .min_w_0()
                .flex_none()
                .overflow_hidden()
                .px(px(14.0))
                .pt(px(6.0))
                .pb(px(12.0))
                .child(agent_composer(
                    &app,
                    self,
                    completion_open,
                    composer_ready,
                    &theme,
                )),
        );
        if let Some(menu) = completion_menu.as_ref() {
            body = body.child(agent_completion_overlay(
                &app,
                menu,
                composer_bottom,
                rect.width,
                self.motion,
                &theme,
            ));
        }

        let pane = div()
            .id(SharedString::from(format!("agent-pane-{pane_id}")))
            .absolute()
            .left(px(rect.x))
            .top(px(rect.y))
            .w(px(rect.width))
            .h(px(rect.height))
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .border_1()
            .border_color(if focused { theme.accent } else { theme.border })
            .child(body);
        if self.motion == MotionPreference::Reduced {
            pane.into_any_element()
        } else {
            pane.with_animation(
                SharedString::from(format!("agent-pane-enter-{pane_id}")),
                interface_animation(170),
                move |pane, delta| {
                    pane.left(px(rect.x + (1.0 - delta) * 4.0))
                        .opacity(0.72 + delta * 0.28)
                },
            )
            .into_any_element()
        }
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = BezelTheme::of(cx).clone();
        let mut bar = h_flex()
            .h(px(layout::TAB_BAR_HEIGHT))
            .w_full()
            .items_center()
            .gap_1()
            .px_1()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.glass())
            .font_family(theme.font_sans.clone());
        if cfg!(target_os = "macos") {
            bar = bar.child(div().w(px(70.0)).h_full());
        }
        if let Some(session) = &self.session {
            for tab in &session.tabs {
                let tab_id = tab.id;
                let active = tab_id == session.active_tab;
                let activity = agent_tab_activity(
                    self.agents
                        .iter()
                        .filter(|agent| agent.tab_id == Some(tab_id))
                        .map(|agent| agent.status),
                );
                let title = tab.title.clone();
                let mut button = Button::new(SharedString::from(format!("tab-{tab_id}")))
                    .label(title.clone())
                    .ghost()
                    .small()
                    .compact()
                    .selected(active)
                    .on_click(cx.listener(move |this, _, _, _| {
                        this.send_workspace(WorkspaceCommand::SelectTab(tab_id));
                    }));
                if let Some(activity) = activity {
                    button = button
                        .tooltip(format!("{title} · {}", agent_tab_activity_label(activity)))
                        .child(
                            div()
                                .flex_none()
                                .size(px(5.0))
                                .rounded_full()
                                .bg(agent_tab_activity_tone(activity, &theme)),
                        );
                }
                bar = bar.child(button);
            }
        }
        bar.child(
            Button::new("tab-new")
                .icon(IconName::Plus)
                .label("New")
                .ghost()
                .xsmall()
                .compact()
                .tooltip("New terminal tab")
                .on_click(cx.listener(|this, _, _, _| {
                    this.send_workspace(WorkspaceCommand::NewTab);
                })),
        )
        .child(
            div()
                .id("window-drag-region")
                .flex_1()
                .h_full()
                .on_mouse_down(gpui::MouseButton::Left, |_, window, cx| {
                    cx.stop_propagation();
                    window.start_window_move();
                })
                .on_double_click(|_, window, cx| {
                    cx.stop_propagation();
                    window.zoom_window();
                }),
        )
        .child(div().w(px(HEADER_ACTIONS_WIDTH)).h_full())
    }

    fn render_mode_bar(&self, theme: &BezelTheme) -> gpui::AnyElement {
        let (label, help) = match self.mode {
            InputMode::Normal => ("NORMAL", ""),
            InputMode::Pane => (
                "PANE",
                "d down · r right · arrows focus · a agent · x close · f zoom",
            ),
            InputMode::Tab => (
                "TAB",
                "n new · x close · r rename · 1–9 select · arrows switch",
            ),
            InputMode::Session => ("SESSION", "w switch · d detach"),
            InputMode::Resize => ("RESIZE", "arrows resize · Enter finish"),
        };
        let bar = h_flex()
            .absolute()
            .left_0()
            .right_0()
            .bottom_0()
            .h(px(30.0))
            .px_3()
            .gap_3()
            .bg(theme.glass())
            .border_t_1()
            .border_color(theme.border)
            .font_family(theme.font_sans.clone())
            .child(
                div()
                    .text_xs()
                    .font_semibold()
                    .text_color(theme.accent)
                    .child(label),
            )
            .child(div().text_xs().text_color(theme.text_muted).child(help));
        if self.motion == MotionPreference::Reduced {
            bar.into_any_element()
        } else {
            bar.with_animation(
                SharedString::from(format!("mode-bar-enter-{label}")),
                interface_animation(140),
                |bar, delta| {
                    bar.bottom(px(-6.0 + delta * 6.0))
                        .opacity(0.4 + delta * 0.6)
                },
            )
            .into_any_element()
        }
    }

    fn render_terminal_pane(
        &self,
        geometry: layout::PaneGeometry,
        pane_count: usize,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let pane = self.panes.get(&geometry.pane_id)?;
        let pane_id = geometry.pane_id;
        let rect = geometry.rect;
        let focused = geometry.focused;
        let pointer_app = cx.weak_entity();
        let hover_app = pointer_app.clone();
        let scroll_app = pointer_app.clone();
        let mut surface = div()
            .absolute()
            .left(px(rect.x))
            .top(px(rect.y))
            .w(px(rect.width))
            .h(px(rect.height))
            .overflow_hidden()
            .bg(rgb(SURFACE))
            .on_any_mouse_down(move |event, window, cx| {
                let _ = pointer_app.update(cx, |this, cx| {
                    this.focus_handle.focus(window, cx);
                    this.pointer_down(pane_id, rect, event);
                    cx.notify();
                });
                cx.stop_propagation();
            })
            .on_mouse_move(move |event, _, cx| {
                let handled = hover_app
                    .update(cx, |this, _| this.pointer_hover(pane_id, rect, event))
                    .unwrap_or(false);
                if handled {
                    cx.stop_propagation();
                }
            })
            .on_scroll_wheel(move |event, _, cx| {
                let _ = scroll_app.update(cx, |this, cx| {
                    if this.scroll_pane(pane_id, rect, event) {
                        cx.notify();
                    }
                });
                cx.stop_propagation();
            })
            .child(gpui_terminal::terminal_canvas(
                Rc::clone(&pane.frame),
                Rc::clone(&pane.render_cache),
                self.terminal_font.clone(),
                self.metrics,
                focused,
            ));
        if focused && pane_count > 1 {
            // A short "focus beam" is visible at a glance without boxing
            // every pane or stealing terminal pixels with permanent borders.
            surface = surface.child(pane_focus_beam(pane_id, self.motion));
        }
        Some(surface.into_any_element())
    }
}

impl UserEvent {
    const fn label(&self) -> &'static str {
        match self {
            Self::Attached(_) => "attached",
            Self::WorkspaceUpdated(_) => "workspace-updated",
            Self::Sessions(_) => "sessions",
            Self::Server(_) => "server",
            Self::Agents(_) => "agents",
            Self::AgentStarted(_) => "agent-started",
            Self::Agent(_) => "agent",
            Self::AgentFiles { .. } => "agent-files",
            Self::BackendError(_) => "backend-error",
        }
    }
}

fn open_rename_session_dialog(
    app: gpui::WeakEntity<MuxApp>,
    session: &SessionSummary,
    window: &mut Window,
    cx: &mut App,
) {
    let session_id = session.id;
    let session_name = session.name.clone();
    let input = cx.new(|cx| {
        let mut input = InputState::new(window, cx).placeholder("Session name");
        input.set_value(session_name, window, cx);
        input
    });
    let submit_input = input.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        let submit_app = app.clone();
        let input_for_button = submit_input.clone();
        dialog.title("Rename session").w(px(420.0)).child(
            v_flex().gap_3().child(Input::new(&submit_input)).child(
                Button::new("rename-session-submit")
                    .label("Rename")
                    .primary()
                    .on_click(move |_, window, cx| {
                        let name = input_for_button.read(cx).value().trim().to_owned();
                        if !name.is_empty() {
                            let _ = submit_app.update(cx, |this, _| {
                                this.backend
                                    .send(CommandMessage::RenameSession { session_id, name });
                            });
                            window.close_all_dialogs(cx);
                        }
                    }),
            ),
        )
    });
}

impl Render for MuxApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = BezelTheme::of(cx).clone();
        let viewport = window.viewport_size();
        let geometry =
            self.sync_terminal_sizes(f32::from(viewport.width), f32::from(viewport.height));
        let pane_count = geometry.panes.len();
        let move_app = cx.weak_entity();
        let release_app = move_app.clone();
        let release_out_app = move_app.clone();
        let mut root = div()
            .id("mux-root")
            .key_context("MuxTerminal")
            .relative()
            .size_full()
            .overflow_hidden()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_forward_terminal_tab))
            .on_action(cx.listener(Self::on_forward_terminal_backtab))
            .on_key_down(cx.listener(Self::handle_key_down))
            .on_key_up(cx.listener(Self::handle_key_up))
            .on_mouse_move(move |event, _, cx| {
                let handled = move_app
                    .update(cx, |this, cx| {
                        let handled = this.pointer_move(event);
                        if handled {
                            cx.notify();
                        }
                        handled
                    })
                    .unwrap_or(false);
                if handled {
                    cx.stop_propagation();
                }
            })
            .capture_any_mouse_up(move |event, _, cx| {
                let handled = release_app
                    .update(cx, |this, cx| {
                        let handled = this.pointer_up(event);
                        if handled {
                            cx.notify();
                        }
                        handled
                    })
                    .unwrap_or(false);
                if handled {
                    cx.stop_propagation();
                }
            })
            .on_mouse_up_out(gpui::MouseButton::Left, move |event, _, cx| {
                let handled = release_out_app
                    .update(cx, |this, cx| {
                        let handled = this.pointer_up(event);
                        if handled {
                            cx.notify();
                        }
                        handled
                    })
                    .unwrap_or(false);
                if handled {
                    cx.stop_propagation();
                }
            })
            .bg(theme.surface)
            .font_family(theme.font_sans.clone())
            .text_color(theme.text)
            .child(self.render_tabs(cx));

        for geometry in geometry.panes {
            let pane_id = geometry.pane_id;
            if self.active_agent_pane() == Some(pane_id) {
                let rect = geometry.rect;
                let focused = geometry.focused;
                root = root.child(self.render_agent_pane(pane_id, rect, focused, window, cx));
                continue;
            }
            if let Some(surface) = self.render_terminal_pane(geometry, pane_count, cx) {
                root = root.child(surface);
            }
        }

        if self.mode != InputMode::Normal {
            root = root.child(self.render_mode_bar(&theme));
        }
        let active_agents = self
            .agents_for_active_tab()
            .filter(|agent| agent.status != AgentSessionStatus::Closed)
            .count();
        root = root.child(header_actions(&cx.weak_entity(), active_agents));
        root
    }
}

fn pane_focus_beam(pane_id: PaneId, motion: MotionPreference) -> gpui::AnyElement {
    let beam = div()
        .absolute()
        .top_0()
        .left(px(8.0))
        .w(px(34.0))
        .h(px(2.0))
        .rounded_full()
        .bg(rgb(SIGNAL));
    if motion == MotionPreference::Reduced {
        beam.into_any_element()
    } else {
        beam.with_animation(
            SharedString::from(format!("pane-focus-{pane_id}")),
            interface_animation(120),
            |beam, delta| beam.w(px(10.0 + delta * 24.0)).opacity(0.35 + delta * 0.65),
        )
        .into_any_element()
    }
}

impl Render for MuxLayerHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut root = div()
            .relative()
            .size_full()
            .overflow_hidden()
            .child(self.view.clone());
        if let Some(sheet) = gpui_component::Root::render_sheet_layer(window, cx) {
            root = root.child(sheet);
        }
        if let Some(dialog) = gpui_component::Root::render_dialog_layer(window, cx) {
            root = root.child(dialog);
        }
        if let Some(notifications) = gpui_component::Root::render_notification_layer(window, cx) {
            root = root.child(notifications);
        }
        root
    }
}

fn header_actions(app: &gpui::WeakEntity<MuxApp>, active_agents: usize) -> impl IntoElement {
    let agents_app = app.clone();
    let settings_app = app.clone();
    let agents_label = if active_agents == 0 {
        "Agents".to_owned()
    } else {
        format!("Agents {active_agents}")
    };
    h_flex()
        .absolute()
        .top(px(2.0))
        .right(px(4.0))
        .gap_1()
        .child(
            Button::new("open-agents")
                .icon(IconName::Bot)
                .label(agents_label)
                .ghost()
                .xsmall()
                .compact()
                .tooltip("Open agent workspace · ⌃A")
                .on_click(move |_, window, cx| {
                    let _ = agents_app.update(cx, |this, cx| this.toggle_agents(window, cx));
                }),
        )
        .child(
            Button::new("open-settings")
                .icon(IconName::Settings2)
                .label("Settings")
                .ghost()
                .xsmall()
                .compact()
                .tooltip("Open Mux settings")
                .on_click(move |_, window, cx| {
                    let _ = settings_app.update(cx, |this, cx| this.open_settings(window, cx));
                }),
        )
}

fn return_agent_pane(app: &gpui::WeakEntity<MuxApp>, window: &mut Window, cx: &mut App) {
    let _ = app.update(cx, |this, cx| {
        this.return_agent_pane_to_terminal(window, cx);
    });
}

fn navigate_agent_pane(
    app: &gpui::WeakEntity<MuxApp>,
    direction: Direction,
    window: &mut Window,
    cx: &mut App,
) {
    let _ = app.update(cx, |this, cx| {
        this.navigate_from_agent_pane(direction, window, cx);
    });
}

fn handle_agent_pane_key_down(
    app: &gpui::WeakEntity<MuxApp>,
    event: &KeyDownEvent,
    window: &mut Window,
    cx: &mut App,
) {
    let control_a = event.keystroke.key.eq_ignore_ascii_case("a")
        && event.keystroke.modifiers.control
        && !event.keystroke.modifiers.alt
        && !event.keystroke.modifiers.shift
        && !event.keystroke.modifiers.platform;
    if control_a {
        return_agent_pane(app, window, cx);
        cx.stop_propagation();
        return;
    }
    let alt_direction = event
        .keystroke
        .modifiers
        .alt
        .then_some(match event.keystroke.key.as_str() {
            "left" | "h" => Some(Direction::Left),
            "right" | "l" => Some(Direction::Right),
            "up" | "k" => Some(Direction::Up),
            "down" | "j" => Some(Direction::Down),
            _ => None,
        })
        .flatten()
        .filter(|_| {
            !event.keystroke.modifiers.control
                && !event.keystroke.modifiers.shift
                && !event.keystroke.modifiers.platform
        });
    if let Some(direction) = alt_direction {
        let _ = app.update(cx, |this, cx| {
            this.navigate_from_agent_pane(direction, window, cx);
        });
        cx.stop_propagation();
        return;
    }
    if let Some(chord) = key_chord(&event.keystroke) {
        let action = app
            .update(cx, |this, _| this.keymap.resolve(this.mode, chord).cloned())
            .ok()
            .flatten();
        if let Some(action) = action {
            let _ = app.update(cx, |this, cx| {
                this.perform_action(action, window, cx);
            });
            cx.stop_propagation();
            return;
        }
    }
    if event.keystroke.key != "escape"
        || event.keystroke.modifiers.control
        || event.keystroke.modifiers.alt
        || event.keystroke.modifiers.shift
        || event.keystroke.modifiers.platform
    {
        return;
    }
    cancel_agent_turn(app, window, cx);
}

fn cancel_agent_turn(app: &gpui::WeakEntity<MuxApp>, _window: &mut Window, cx: &mut App) {
    let _ = app.update(cx, |this, cx| {
        let running = this
            .active_agent()
            .filter(|agent| {
                matches!(
                    agent.status,
                    AgentSessionStatus::Working | AgentSessionStatus::WaitingForPermission
                )
            })
            .map(|agent| agent.id);
        if let Some(session_id) = running {
            this.backend.send(CommandMessage::CancelAgent(session_id));
        }
        cx.notify();
    });
    cx.stop_propagation();
}

fn agent_settings_file_row(
    app: &gpui::WeakEntity<MuxApp>,
    settings_path: &Path,
    theme: &BezelTheme,
) -> gpui::AnyElement {
    let reveal_app = app.clone();
    let reveal_path = settings_path.to_path_buf();
    h_flex()
        .w_full()
        .min_w_0()
        .gap_2()
        .p_2()
        .rounded(px(BezelTheme::CONTROL_RADIUS))
        .border_1()
        .border_color(theme.border)
        .bg(theme.code_wash)
        .child(
            div()
                .min_w_0()
                .flex_1()
                .truncate()
                .font_family(theme.font_mono.clone())
                .text_xs()
                .text_color(theme.text_muted)
                .child(settings_path.display().to_string()),
        )
        .child(
            Button::new("reveal-agent-settings")
                .label("Reveal")
                .ghost()
                .small()
                .compact()
                .on_click(move |_, window, cx| {
                    let saved = reveal_app
                        .update(cx, |this, _| {
                            let Some(state_dir) = this.state_dir.as_ref() else {
                                return Err(anyhow!("settings directory unavailable"));
                            };
                            this.settings.save(state_dir)
                        })
                        .unwrap_or_else(|error| Err(anyhow!(error)));
                    if let Err(error) = saved {
                        window.push_notification(
                            Notification::error(format!(
                                "Could not create settings.json: {error:#}"
                            )),
                            cx,
                        );
                        return;
                    }
                    if let Err(error) = Command::new("/usr/bin/open")
                        .arg("-R")
                        .arg(&reveal_path)
                        .spawn()
                    {
                        window.push_notification(
                            Notification::error(format!("Could not reveal settings.json: {error}")),
                            cx,
                        );
                    }
                }),
        )
        .into_any_element()
}

fn agent_session_picker(
    app: &gpui::WeakEntity<MuxApp>,
    this: &MuxApp,
    theme: &BezelTheme,
) -> impl IntoElement {
    let agents = this.agents_for_active_tab().collect::<Vec<_>>();
    let selected_id = this.active_agent().map(|agent| agent.id);
    let new_app = app.clone();
    let mut rail = h_flex()
        .w_full()
        .min_w_0()
        .flex_none()
        .flex_wrap()
        .gap_1()
        .px(px(10.0))
        .py(px(5.0))
        .border_b_1()
        .border_color(theme.border)
        .bg(theme.band)
        .child(
            div()
                .flex_none()
                .pr(px(4.0))
                .text_size(px(9.0))
                .font_semibold()
                .text_color(theme.text_faint)
                .child("SESSIONS"),
        );
    for agent in agents {
        let session_id = agent.id;
        let select_app = app.clone();
        let selected = selected_id == Some(session_id);
        let label = agent_session_picker_label(agent);
        rail = rail.child(
            Button::new(SharedString::from(format!("agent-session-{session_id}")))
                .label(label.clone())
                .ghost()
                .small()
                .compact()
                .selected(selected)
                .tooltip(format!("{label} · ⌥←/⌥→ to navigate"))
                .on_click(move |_, window, cx| {
                    let _ = select_app.update(cx, |this, cx| {
                        this.select_active_tab_agent(Some(session_id));
                        this.follow_active_agent_tail();
                        cx.notify();
                    });
                    window.refresh();
                }),
        );
    }
    rail.child(div().flex_1()).child(
        Button::new("agent-new")
            .icon(IconName::Plus)
            .label("New")
            .ghost()
            .small()
            .compact()
            .tooltip("New agent session · /new")
            .on_click(move |_, window, cx| {
                let _ = new_app.update(cx, |this, cx| {
                    this.select_active_tab_agent(None);
                    cx.notify();
                });
                window.refresh();
            }),
    )
}

fn agent_session_picker_label(agent: &AgentSessionSnapshot) -> String {
    format!(
        "{} · {}",
        agent_display_name(agent),
        agent_status_label(agent.status)
    )
}

const fn agent_session_is_visible(status: AgentSessionStatus) -> bool {
    !matches!(status, AgentSessionStatus::Closed)
}

fn agent_display_name(agent: &AgentSessionSnapshot) -> &str {
    agent.agent_name.as_deref().unwrap_or(agent.name.as_str())
}

fn agent_context_usage_label(agent: &AgentSessionSnapshot) -> Option<String> {
    format_agent_context_usage(agent.context_used, agent.context_size)
}

fn format_agent_context_usage(used: Option<u64>, size: Option<u64>) -> Option<String> {
    let (used, size) = (used?, size?);
    (size > 0).then(|| format!("context {}%", used.min(size).saturating_mul(100) / size))
}

fn agent_pane_is_compact(pane_width: f32) -> bool {
    pane_width < 620.0
}

fn agent_header_metadata(agent: &AgentSessionSnapshot, context_panes: usize) -> Vec<String> {
    let mut metadata = vec![
        agent.cwd.file_name().map_or_else(
            || agent.cwd.display().to_string(),
            |name| name.to_string_lossy().into(),
        ),
        format_tab_pane_count(context_panes),
    ];
    if let Some(mode) = agent
        .current_mode
        .as_deref()
        .filter(|mode| !mode.trim().is_empty())
    {
        metadata.push(mode.to_owned());
    }
    if let Some(context) = agent_context_usage_label(agent) {
        metadata.push(context);
    }
    metadata
}

fn format_tab_pane_count(panes: usize) -> String {
    format!(
        "{panes} {} in tab",
        if panes == 1 { "pane" } else { "panes" }
    )
}

fn format_session_pane_count(panes: usize) -> String {
    format!("{panes} {}", if panes == 1 { "pane" } else { "panes" })
}

fn agent_status_badge(status: AgentSessionStatus, theme: &BezelTheme) -> impl IntoElement {
    let tone = agent_status_tone(status, theme);
    theme
        .badge(agent_status_label(status))
        .border_color(tone.opacity(0.28))
        .bg(tone.opacity(0.09))
        .text_color(tone)
}

fn agent_pane_header(
    app: &gpui::WeakEntity<MuxApp>,
    pane_id: PaneId,
    agent: Option<&AgentSessionSnapshot>,
    context_panes: usize,
    theme: &BezelTheme,
) -> impl IntoElement {
    let return_app = app.clone();
    let name = agent.map_or("Agent workspace", agent_display_name);
    let metadata = agent.map_or_else(
        || vec![format_tab_pane_count(context_panes)],
        |agent| agent_header_metadata(agent, context_panes),
    );
    let mut meta = h_flex()
        .min_w_0()
        .gap(px(6.0))
        .text_size(px(10.5))
        .text_color(theme.text_faint);
    for (index, item) in metadata.into_iter().enumerate() {
        if index > 0 {
            meta = meta.child(div().flex_none().opacity(0.45).child("·"));
        }
        meta = meta.child(div().min_w_0().truncate().child(item));
    }
    h_flex()
        .w_full()
        .min_w_0()
        .flex_none()
        .h(px(52.0))
        .px(px(10.0))
        .gap(px(10.0))
        .border_b_1()
        .border_color(theme.border)
        .bg(theme.glass())
        .child(theme.row_tile(bezel_icons::CHAT_ROUND_LINE))
        .child(
            v_flex()
                .min_w_0()
                .flex_1()
                .gap(px(2.0))
                .child(
                    h_flex()
                        .min_w_0()
                        .gap(px(8.0))
                        .child(theme.row_title(name))
                        .when_some(agent.map(|agent| agent.status), |row, status| {
                            row.child(agent_status_badge(status, theme))
                        }),
                )
                .child(meta),
        )
        .child(
            Button::new(SharedString::from(format!("agent-return-{pane_id}")))
                .icon(IconName::SquareTerminal)
                .label("Terminal")
                .ghost()
                .small()
                .compact()
                .tooltip("Return to terminal · ⌃A")
                .on_click(move |_, window, cx| {
                    return_agent_pane(&return_app, window, cx);
                }),
        )
}

fn agent_manifest_row(
    theme: &BezelTheme,
    first: bool,
    icon: &'static str,
    title: &'static str,
    detail: String,
    badge: &'static str,
) -> gpui::AnyElement {
    theme
        .card_row(first)
        .child(theme.row_tile(icon))
        .child(
            v_flex()
                .min_w_0()
                .flex_1()
                .child(theme.row_title(title))
                .child(theme.meta_line(vec![
                    div().min_w_0().truncate().child(detail).into_any_element(),
                ])),
        )
        .child(theme.badge(badge))
        .into_any_element()
}

fn agent_empty_hero(compact: bool, theme: &BezelTheme) -> gpui::AnyElement {
    h_flex()
        .w_full()
        .min_w_0()
        .items_start()
        .gap(px(14.0))
        .child(
            div()
                .flex_none()
                .size(px(46.0))
                .rounded(px(BezelTheme::SURFACE_RADIUS))
                .border_1()
                .border_color(theme.accent.opacity(0.25))
                .bg(theme.accent.opacity(0.08))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    bezel_icons::icon(bezel_icons::CHAT_ROUND_LINE)
                        .size(px(21.0))
                        .text_color(theme.accent),
                ),
        )
        .child(
            v_flex()
                .min_w_0()
                .flex_1()
                .gap(px(5.0))
                .child(
                    div()
                        .text_size(px(10.5))
                        .font_semibold()
                        .text_color(theme.accent)
                        .child("DURABLE AGENT"),
                )
                .child(
                    div()
                        .text_size(px(if compact { 18.0 } else { 21.0 }))
                        .font_semibold()
                        .child("Put this tab to work."),
                )
                .child(
                    div()
                        .whitespace_normal()
                        .text_sm()
                        .line_height(px(20.0))
                        .text_color(theme.text_muted)
                        .child("Send a task below. Mux keeps the agent, terminal context, and workspace attached while you move on."),
                ),
        )
        .into_any_element()
}

fn agent_empty_state(
    this: &MuxApp,
    context_panes: usize,
    compact: bool,
    theme: &BezelTheme,
) -> impl IntoElement {
    let mut profiles = this
        .enabled_profiles()
        .map(|profile| profile.name.as_str())
        .collect::<Vec<_>>();
    let profiles = match profiles.pop() {
        None => "an enabled agent".to_owned(),
        Some(last) if profiles.is_empty() => last.to_owned(),
        Some(last) if profiles.len() == 1 => format!("{} or {last}", profiles[0]),
        Some(last) => format!("{}, or {last}", profiles.join(", ")),
    };
    let panes = format!(
        "{context_panes} live {} · starts in the focused pane's directory",
        if context_panes == 1 { "pane" } else { "panes" }
    );
    let manifest = theme
        .group_box()
        .mt(px(if compact { 16.0 } else { 22.0 }))
        .child(agent_manifest_row(
            theme,
            true,
            bezel_icons::FOLDER_WITH_FILES,
            "Context follows this tab",
            panes,
            "TAB",
        ))
        .child(agent_manifest_row(
            theme,
            false,
            bezel_icons::CLOUD,
            "Durable by default",
            "Owned by muxd · closing this view leaves the session running".to_owned(),
            "MUXD",
        ))
        .child(agent_manifest_row(
            theme,
            false,
            bezel_icons::CPU,
            "Choose the right agent",
            format!("Enabled: {profiles}"),
            "READY",
        ));
    div()
        .w_full()
        .min_w_0()
        .flex_1()
        .min_h_0()
        .overflow_y_scrollbar()
        .flex()
        .justify_center()
        .child(
            div()
                .w_full()
                .max_w(px(660.0))
                .min_w_0()
                .px(px(if compact { 16.0 } else { 24.0 }))
                .py(px(if compact { 24.0 } else { 40.0 }))
                .child(agent_empty_hero(compact, theme))
                .child(manifest)
                .child(
                    h_flex()
                        .mt(px(12.0))
                        .gap(px(8.0))
                        .text_size(px(10.5))
                        .text_color(theme.text_faint)
                        .child("⌃A returns to terminal")
                        .child(div().opacity(0.45).child("·"))
                        .child("/help shows every command"),
                ),
        )
}

fn agent_help_surface(
    agent: Option<&AgentSessionSnapshot>,
    theme: &BezelTheme,
) -> impl IntoElement {
    v_flex()
        .w_full()
        .min_w_0()
        .flex_1()
        .min_h_0()
        .overflow_y_scrollbar()
        .px_3()
        .pt_2()
        .pb_3()
        .child(
            div()
                .w_full()
                .max_w(px(720.0))
                .mx_auto()
                .child(agent_help_card(agent, theme)),
        )
}

fn agent_help_card(agent: Option<&AgentSessionSnapshot>, theme: &BezelTheme) -> gpui::AnyElement {
    let mut card = v_flex()
        .w_full()
        .min_w_0()
        .flex_none()
        .gap_3()
        .p_3()
        .rounded(px(BezelTheme::PANEL_RADIUS))
        .border_1()
        .border_color(theme.border)
        .bg(theme.surface_card)
        .child(
            h_flex()
                .w_full()
                .min_w_0()
                .gap_2()
                .child(
                    bezel_icons::icon(bezel_icons::BOOK)
                        .size(px(16.0))
                        .text_color(theme.accent),
                )
                .child(div().font_semibold().child("Agent commands")),
        )
        .child(
            div()
                .w_full()
                .min_w_0()
                .whitespace_normal()
                .text_sm()
                .line_height(px(20.0))
                .text_color(theme.text_muted)
                .child("Mux commands stay local. Other slash commands are sent to the active ACP agent."),
        )
        .child(agent_help_section(
            theme,
            "Keyboard",
            &[
                "⌃A  toggle agent pane",
                "⌥arrows  navigate sessions and panes",
                "Esc  cancel run",
                "Return  send",
                "⇧Return  newline",
                "↑↓ + Tab  choose completion",
            ],
        ))
        .child(agent_help_section(
            theme,
            "Sessions",
            &["/new [agent] [cwd]", "/next", "/prev", "/use <session>", "/end", "/cancel"],
        ))
        .child(agent_help_section(
            theme,
            "Context + view",
            &[
                "@path  attach a project file",
                "/context tab|none",
                "/expand [all]",
                "/collapse [all]",
            ],
        ))
        .child(agent_help_section(
            theme,
            "Configure",
            &["/mode <id>", "/model <id>", "/effort <id>", "/login [method]", "/allow [always]", "/deny [always]"],
        ));

    if let Some(agent) = agent {
        let commands = agent
            .available_commands
            .iter()
            .filter(|command| !command.name.starts_with('$'))
            .take(10)
            .map(|command| format!("/{}", command.name))
            .collect::<Vec<_>>();
        if !commands.is_empty() {
            card = card.child(agent_help_owned_section(theme, "From agent", &commands));
        }
    }
    card.into_any_element()
}

fn agent_help_section(
    theme: &BezelTheme,
    label: &'static str,
    commands: &[&'static str],
) -> gpui::AnyElement {
    let commands = commands
        .iter()
        .map(|command| (*command).to_owned())
        .collect::<Vec<_>>();
    agent_help_owned_section(theme, label, &commands)
}

fn agent_help_owned_section(
    theme: &BezelTheme,
    label: &'static str,
    commands: &[String],
) -> gpui::AnyElement {
    let mut chips = h_flex().w_full().min_w_0().gap_1().flex_wrap();
    for command in commands {
        chips = chips.child(
            div()
                .flex_none()
                .whitespace_nowrap()
                .px_2()
                .py_1()
                .rounded(px(BezelTheme::CONTROL_RADIUS))
                .bg(theme.code_wash)
                .font_family(theme.font_mono.clone())
                .text_xs()
                .text_color(theme.code_text)
                .child(command.clone()),
        );
    }
    v_flex()
        .w_full()
        .min_w_0()
        .gap_1p5()
        .child(
            div()
                .text_size(px(10.5))
                .font_semibold()
                .text_color(theme.text_faint)
                .child(label),
        )
        .child(chips)
        .into_any_element()
}

fn agent_composer(
    app: &gpui::WeakEntity<MuxApp>,
    this: &MuxApp,
    completion_open: bool,
    ready: bool,
    theme: &BezelTheme,
) -> gpui::AnyElement {
    let prompt_app = app.clone();
    let send = div()
        .id("agent-send")
        .flex_none()
        .size(px(26.0))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .when(ready, |send| {
            send.bg(theme.solid)
                .cursor_pointer()
                .hover(|style| style.opacity(0.88))
                .on_click(move |_, window, cx| {
                    let _ = prompt_app.update(cx, |this, cx| {
                        this.submit_agent_prompt(window, cx);
                    });
                    window.refresh();
                })
        })
        .when(!ready, |send| send.bg(theme.element_hover))
        .child(
            bezel_icons::icon(bezel_icons::ARROW_UP)
                .size(px(14.0))
                .text_color(if ready {
                    theme.on_solid
                } else {
                    theme.text_faint
                }),
        );
    let context = match this.agent_context {
        AgentContextMode::None => "No pane context",
        AgentContextMode::Tab => "Tab context",
    };
    let card = v_flex()
        .w_full()
        .min_w_0()
        .overflow_hidden()
        .rounded(px(BezelTheme::SURFACE_RADIUS))
        .border_1()
        .border_color(theme.border_strong)
        .bg(theme.input_glass_bg())
        .shadow_sm()
        .px(px(5.0))
        .pt(px(4.0))
        .pb(px(6.0))
        .child(
            Textarea::new(&this.agent_input)
                .appearance(false)
                .bordered(false)
                .min_w_0()
                .w_full(),
        )
        .child(
            h_flex()
                .w_full()
                .min_w_0()
                .items_center()
                .px(px(7.0))
                .pt(px(2.0))
                .gap(px(8.0))
                .text_size(px(10.5))
                .text_color(theme.text_faint)
                .child(context)
                .child(div().ml_auto().child("Return send · ⇧Return newline"))
                .child(send),
        );
    let composer = v_flex()
        .when(completion_open, |composer| {
            composer.key_context("MuxAgentCompletion")
        })
        .w_full()
        .min_w_0()
        .flex_none()
        .overflow_hidden()
        .gap_0()
        .child(bezel_ui::material::material(
            BezelTheme::SURFACE_RADIUS,
            28.0,
            card,
        ));
    composer.into_any_element()
}

fn input_position_at(text: &str, offset: usize) -> Position {
    let offset = offset.min(text.len());
    let prefix = &text[..offset];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() as u32;
    let character = prefix
        .rsplit_once('\n')
        .map_or(prefix, |(_, line)| line)
        .encode_utf16()
        .count() as u32;
    Position::new(line, character)
}

fn agent_composer_height(value: &str, pane_width: f32) -> f32 {
    // InputState wraps with the same width as the composer minus padding and
    // the send button. This deliberately rounds up: a completion popup may
    // float a few pixels above the input, but it must never cover a wrapped
    // draft or fall outside the pane.
    let columns = ((pane_width - 76.0) / 8.0).floor().max(16.0) as usize;
    let rows = value
        .split('\n')
        .map(|line| line.chars().count().max(1).div_ceil(columns))
        .sum::<usize>()
        .clamp(1, 6);
    57.0 + (rows.saturating_sub(1) as f32 * 20.0)
}

fn agent_completion_overlay(
    app: &gpui::WeakEntity<MuxApp>,
    menu: &AgentCompletionMenu,
    bottom: f32,
    pane_width: f32,
    motion: MotionPreference,
    theme: &BezelTheme,
) -> gpui::AnyElement {
    let horizontal_inset = agent_surface_inset(pane_width);
    let mut items = v_flex().w_full().min_w_0().p_1().gap_0p5();
    for (index, completion) in menu.items.iter().enumerate() {
        items = items.child(agent_completion_row(
            app,
            index,
            completion,
            index == menu.selected,
            theme,
        ));
    }

    let popup = v_flex()
        .id("agent-completion-menu")
        .absolute()
        .left(px(horizontal_inset))
        .right(px(horizontal_inset))
        .bottom(px(bottom))
        .min_w_0()
        .overflow_hidden()
        .rounded(px(BezelTheme::PANEL_RADIUS))
        .border_1()
        .border_color(theme.border_strong)
        .bg(theme.glass_overlay())
        .shadow_lg()
        .child(items)
        .child(
            h_flex()
                .w_full()
                .h(px(24.0))
                .px_2()
                .gap_3()
                .border_t_1()
                .border_color(theme.border)
                .bg(theme.band)
                .text_size(px(9.0))
                .text_color(theme.text_faint)
                .child("↑↓ select")
                .child("Tab complete")
                .child("Esc close"),
        );

    let popup = if motion == MotionPreference::Reduced {
        popup.into_any_element()
    } else {
        popup
            .with_animation(
                "agent-completion-enter",
                interface_animation(100),
                move |popup, delta| {
                    popup
                        .bottom(px(bottom - (1.0 - delta) * 3.0))
                        .opacity(delta)
                },
            )
            .into_any_element()
    };
    bezel_ui::material::material(
        BezelTheme::PANEL_RADIUS,
        bezel_ui::material::MENU_BLUR,
        popup,
    )
    .into_any_element()
}

fn agent_surface_inset(pane_width: f32) -> f32 {
    ((pane_width - 760.0) / 2.0).max(8.0)
}

fn agent_completion_row(
    app: &gpui::WeakEntity<MuxApp>,
    index: usize,
    completion: &AgentCompletion,
    selected: bool,
    theme: &BezelTheme,
) -> gpui::AnyElement {
    let completion_app = app.clone();
    let icon = match completion.kind {
        AgentCompletionKind::Command => IconName::SquareTerminal,
        AgentCompletionKind::Value => IconName::Bot,
        AgentCompletionKind::File => IconName::File,
    };
    let text = match completion.kind {
        AgentCompletionKind::Command | AgentCompletionKind::Value => h_flex()
            .min_w_0()
            .flex_1()
            .gap_3()
            .child(
                div()
                    .flex_none()
                    .w(px(118.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .font_family(theme.font_mono.clone())
                    .text_size(px(12.0))
                    .font_semibold()
                    .child(completion.label.clone()),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(px(10.0))
                    .text_color(theme.text_muted)
                    .child(completion.description.clone()),
            )
            .into_any_element(),
        AgentCompletionKind::File => div()
            .min_w_0()
            .flex_1()
            .overflow_hidden()
            .whitespace_nowrap()
            .font_family(theme.font_mono.clone())
            .text_size(px(12.0))
            .font_semibold()
            .child(completion.label.clone())
            .into_any_element(),
    };
    h_flex()
        .id(SharedString::from(format!("agent-completion-{index}")))
        .group(SharedString::from("agent-completion-row"))
        .w_full()
        .min_w_0()
        .h(px(34.0))
        .px_2()
        .gap_2()
        .rounded(px(BezelTheme::CONTROL_RADIUS))
        .cursor_pointer()
        .when(selected, |row| row.bg(theme.element_active))
        .hover(|row| row.bg(theme.element_hover))
        .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
            cx.stop_propagation();
            let _ = completion_app.update(cx, |this, cx| {
                this.accept_agent_completion(Some(index), window, cx);
            });
        })
        .child(
            div()
                .flex_none()
                .w(px(22.0))
                .h(px(22.0))
                .rounded(px(BezelTheme::CONTROL_RADIUS))
                .bg(if selected {
                    theme.element_active
                } else {
                    theme.surface
                })
                .flex()
                .items_center()
                .justify_center()
                .text_color(if selected {
                    theme.accent
                } else {
                    theme.text_muted
                })
                .child(Icon::new(icon).xsmall()),
        )
        .child(text)
        .child(
            div()
                .flex_none()
                .text_size(px(9.0))
                .text_color(theme.text_faint)
                .child(completion.detail.clone()),
        )
        .into_any_element()
}

fn agent_scroll_is_near_bottom(scroll: &ScrollHandle) -> bool {
    let offset = f32::from(scroll.offset().y);
    let maximum = f32::from(scroll.max_offset().y);
    maximum + offset <= 48.0
}

fn agent_session_empty_copy(
    status: AgentSessionStatus,
    name: &str,
) -> (&'static str, String, &'static str) {
    match status {
        AgentSessionStatus::Starting => (
            "CONNECTING",
            format!("Starting {name}…"),
            "Mux is opening a durable ACP session. You can leave this view while it connects.",
        ),
        AgentSessionStatus::WaitingForAuthentication => (
            "SIGN-IN REQUIRED",
            format!("Finish signing in to {name}."),
            "Complete authentication below; the session will resume here when access is ready.",
        ),
        AgentSessionStatus::Authenticating => (
            "AUTHENTICATING",
            format!("Signing in to {name}…"),
            "Mux is completing the agent handshake and will keep this session attached.",
        ),
        AgentSessionStatus::Idle => (
            "SESSION READY",
            format!("{name} is ready."),
            "Give it a focused outcome. Mux keeps the agent attached while you work elsewhere.",
        ),
        AgentSessionStatus::Working => (
            "IN PROGRESS",
            format!("{name} is working…"),
            "Responses and tool activity will appear here as soon as they arrive.",
        ),
        AgentSessionStatus::WaitingForPermission => (
            "DECISION NEEDED",
            format!("{name} needs permission."),
            "Review the requested action below before the session continues.",
        ),
        AgentSessionStatus::Failed => (
            "SESSION INTERRUPTED",
            format!("{name} could not continue."),
            "Review the session controls below, or start a new durable agent with /new.",
        ),
        AgentSessionStatus::Closed => (
            "SESSION ENDED",
            format!("{name} has stopped."),
            "This session is no longer shown in the picker; use /new when you want another session.",
        ),
    }
}

fn agent_session_empty_state(agent: &AgentSessionSnapshot, theme: &BezelTheme) -> impl IntoElement {
    let name = agent_display_name(agent);
    let (eyebrow, title, description) = agent_session_empty_copy(agent.status, name);
    let tone = agent_status_tone(agent.status, theme);
    let manifest = theme
        .group_box()
        .mt(px(20.0))
        .child(agent_manifest_row(
            theme,
            true,
            bezel_icons::FOLDER_WITH_FILES,
            "Working directory",
            agent.cwd.display().to_string(),
            "CWD",
        ))
        .child(agent_manifest_row(
            theme,
            false,
            bezel_icons::CLOUD,
            "Durable session",
            "Owned by muxd · revisit the agent independently of its terminal".to_owned(),
            "MUXD",
        ));
    div()
        .size_full()
        .min_w_0()
        .min_h_0()
        .overflow_y_scrollbar()
        .flex()
        .justify_center()
        .child(
            div()
                .w_full()
                .max_w(px(600.0))
                .min_w_0()
                .px(px(20.0))
                .py(px(40.0))
                .child(
                    h_flex()
                        .w_full()
                        .min_w_0()
                        .items_start()
                        .gap(px(14.0))
                        .child(
                            div()
                                .flex_none()
                                .size(px(44.0))
                                .rounded(px(BezelTheme::SURFACE_RADIUS))
                                .border_1()
                                .border_color(tone.opacity(0.25))
                                .bg(tone.opacity(0.08))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    bezel_icons::icon(bezel_icons::CPU)
                                        .size(px(20.0))
                                        .text_color(tone),
                                ),
                        )
                        .child(
                            v_flex()
                                .min_w_0()
                                .flex_1()
                                .gap(px(5.0))
                                .child(
                                    div()
                                        .text_size(px(10.5))
                                        .font_semibold()
                                        .text_color(tone)
                                        .child(eyebrow),
                                )
                                .child(div().text_size(px(20.0)).font_semibold().child(title))
                                .child(
                                    div()
                                        .whitespace_normal()
                                        .text_sm()
                                        .line_height(px(20.0))
                                        .text_color(theme.text_muted)
                                        .child(description),
                                ),
                        ),
                )
                .child(manifest)
                .child(
                    h_flex()
                        .mt(px(12.0))
                        .gap(px(8.0))
                        .text_size(px(10.5))
                        .text_color(theme.text_faint)
                        .child("@ adds a file")
                        .child(div().opacity(0.45).child("·"))
                        .child("/help shows session controls"),
                ),
        )
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn agent_timeline(
    app: &gpui::WeakEntity<MuxApp>,
    tab_id: TabId,
    agent: &AgentSessionSnapshot,
    scroll: &ScrollHandle,
    expanded_items: &HashSet<String>,
    show_help: bool,
    follow_tail: bool,
    settle_scroll: bool,
    motion: MotionPreference,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement {
    let theme = BezelTheme::of(cx).clone();
    // Keep following streamed content until the user intentionally scrolls up.
    // GPUI applies this request after layout, so newly parsed Markdown height is
    // included instead of leaving the newest response below the viewport.
    if follow_tail {
        scroll.scroll_to_bottom();
    }
    if follow_tail && settle_scroll {
        let settled_scroll = scroll.clone();
        window.on_next_frame(move |window, _| {
            let maximum = settled_scroll.max_offset();
            settled_scroll.set_offset(gpui::point(px(0.0), -maximum.y));
            window.refresh();
        });
    }
    let scroll_app = app.clone();
    let wheel_scroll = scroll.clone();
    let scrollbar_scroll = scroll.clone();
    let scrollbar_app = app.clone();
    let release_app = app.clone();
    let release_scroll = scroll.clone();
    let mut timeline = v_flex()
        .id(SharedString::from(format!("agent-timeline-{}", agent.id)))
        .size_full()
        .min_w_0()
        .min_h_0()
        .track_scroll(scroll)
        .overflow_x_hidden()
        .overflow_y_scroll()
        .vertical_scrollbar(scroll)
        .on_any_mouse_down(move |event, _, cx| {
            let bounds = scrollbar_scroll.bounds();
            if event.position.x >= bounds.right() - px(18.0) {
                let _ = scrollbar_app.update(cx, |this, cx| {
                    this.agent_follow_tail.remove(&tab_id);
                    this.agent_scroll_needs_settle.remove(&tab_id);
                    cx.notify();
                });
            }
        })
        .on_mouse_up(gpui::MouseButton::Left, move |_, _, cx| {
            if agent_scroll_is_near_bottom(&release_scroll) {
                let _ = release_app.update(cx, |this, cx| {
                    this.agent_follow_tail.insert(tab_id);
                    cx.notify();
                });
            }
        })
        .on_scroll_wheel(move |event, _, cx| {
            let delta = event.delta.pixel_delta(px(20.0));
            let _ = scroll_app.update(cx, |this, cx| {
                if delta.y > px(0.0) {
                    this.agent_follow_tail.remove(&tab_id);
                    this.agent_scroll_needs_settle.remove(&tab_id);
                } else {
                    let remaining =
                        f32::from(wheel_scroll.max_offset().y) + f32::from(wheel_scroll.offset().y);
                    if remaining <= 48.0 + f32::from(delta.y.abs()) {
                        this.agent_follow_tail.insert(tab_id);
                    }
                }
                cx.notify();
            });
        });
    let mut content = v_flex()
        .w_full()
        .min_w_0()
        .flex_none()
        .gap(px(12.0))
        .pr(px(20.0))
        .pl(px(18.0))
        .pt(px(18.0))
        .pb(px(28.0));
    if agent.timeline.is_empty() && !show_help {
        timeline = timeline.child(agent_session_empty_state(agent, &theme));
    }
    for (index, item) in agent.timeline.iter().enumerate() {
        if matches!(item, AgentTimelineItem::Context { .. }) {
            continue;
        }
        let item_content = match item {
            AgentTimelineItem::Message { role, text, .. } if *role != AgentMessageRole::Thought => {
                agent_message_item(agent, index, *role, text, &theme, window, cx)
            }
            AgentTimelineItem::Message { text, .. } => thinking_item(
                app,
                agent,
                index,
                text,
                expanded_items,
                index + 1 == agent.timeline.len() && agent.status == AgentSessionStatus::Working,
                &theme,
                window,
                cx,
            ),
            AgentTimelineItem::Tool(tool) => {
                agent_tool_item(app, agent, index, tool, expanded_items, &theme)
            }
            _ => agent_event_item(item, &theme),
        };
        let row = div()
            .w_full()
            .max_w(px(720.0))
            .mx_auto()
            .min_w_0()
            .flex_none()
            .child(item_content);
        content = content.child(if motion == MotionPreference::Reduced {
            row.into_any_element()
        } else {
            row.with_animation(
                SharedString::from(format!(
                    "agent-item-enter-{}",
                    agent_item_key(agent, index, item)
                )),
                interface_animation(140),
                |row, delta| {
                    row.relative()
                        .top(px((1.0 - delta) * 3.0))
                        .opacity(0.35 + delta * 0.65)
                },
            )
            .into_any_element()
        });
    }
    if show_help {
        content = content.child(
            div()
                .w_full()
                .max_w(px(720.0))
                .mx_auto()
                .child(agent_help_card(Some(agent), &theme)),
        );
    }
    if !agent.timeline.is_empty() || show_help {
        timeline = timeline.child(content);
    }
    let latest_app = app.clone();
    div()
        .relative()
        .w_full()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .overflow_hidden()
        .child(timeline)
        .when(!follow_tail, |viewport| {
            viewport.child(
                Button::new(SharedString::from(format!("agent-latest-{tab_id}")))
                    .label("↓ Latest")
                    .small()
                    .compact()
                    .primary()
                    .absolute()
                    .right(px(14.0))
                    .bottom(px(12.0))
                    .on_click(move |_, window, cx| {
                        let _ = latest_app.update(cx, |this, cx| {
                            this.agent_follow_tail.insert(tab_id);
                            this.agent_scroll_for(tab_id).scroll_to_bottom();
                            cx.notify();
                        });
                        window.refresh();
                    }),
            )
        })
}

fn agent_message_item(
    _agent: &AgentSessionSnapshot,
    _index: usize,
    role: AgentMessageRole,
    text: &str,
    theme: &BezelTheme,
    window: &mut Window,
    cx: &mut App,
) -> gpui::AnyElement {
    if role == AgentMessageRole::User {
        return h_flex()
            .w_full()
            .min_w_0()
            .justify_end()
            .child(
                div()
                    .max_w(px(600.0))
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_normal()
                    .px(px(14.0))
                    .py(px(9.0))
                    .rounded(px(BezelTheme::SURFACE_RADIUS))
                    .bg(bezel_theme::user_bubble_bg())
                    .text_sm()
                    .line_height(px(20.0))
                    .text_color(theme.text)
                    .child(text.trim().to_owned()),
            )
            .into_any_element();
    }
    div()
        .w_full()
        .min_w_0()
        .overflow_hidden()
        .px(px(2.0))
        .child(bezel_markdown::markdown(text.trim(), window, cx))
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn thinking_item(
    app: &gpui::WeakEntity<MuxApp>,
    agent: &AgentSessionSnapshot,
    index: usize,
    text: &str,
    expanded_items: &HashSet<String>,
    running: bool,
    theme: &BezelTheme,
    window: &mut Window,
    cx: &mut App,
) -> gpui::AnyElement {
    let summary = thought_summary(text);
    let detail = thought_detail(text, &summary);
    let key = agent_item_key(agent, index, &agent.timeline[index]);
    let expanded = detail.is_some() && expanded_items.contains(&key);
    let mut header = theme
        .step_row(
            bezel_icons::CPU,
            "Thinking",
            Some(summary.into()),
            running.then(|| SharedString::from("working")),
            false,
            detail.map(|_| expanded),
        )
        .id(SharedString::from(format!("thought-{key}")))
        .w_full()
        .min_w_0();
    if detail.is_some() {
        let toggle_app = app.clone();
        let toggle_key = key.clone();
        header = header
            .hover(bezel_widgets::step_row_hover)
            .on_click(move |_, window, cx| {
                let toggle_key = toggle_key.clone();
                let _ = toggle_app.update(cx, |this, cx| {
                    if !this.expanded_agent_items.remove(&toggle_key) {
                        this.expanded_agent_items.insert(toggle_key);
                    }
                    cx.notify();
                });
                window.refresh();
            });
    }
    let mut item = v_flex()
        .w_full()
        .max_w_full()
        .min_w_0()
        .flex_none()
        .overflow_hidden()
        .border_l_1()
        .border_color(theme.border)
        .pl(px(6.0))
        .gap(px(4.0))
        .child(header);
    if expanded {
        let detail = bezel_markdown::markdown(detail.unwrap_or_default(), window, cx);
        item = item.child(
            div()
                .w_full()
                .min_w_0()
                .overflow_hidden()
                .pl(px(34.0))
                .pr(px(8.0))
                .pb(px(6.0))
                .child(
                    div()
                        .w_full()
                        .min_w_0()
                        .overflow_hidden()
                        .text_color(theme.text_muted)
                        .child(detail),
                ),
        );
    }
    item.into_any_element()
}

fn agent_tool_item(
    app: &gpui::WeakEntity<MuxApp>,
    agent: &AgentSessionSnapshot,
    index: usize,
    tool: &AgentTool,
    expanded_items: &HashSet<String>,
    theme: &BezelTheme,
) -> gpui::AnyElement {
    let key = agent_item_key(agent, index, &agent.timeline[index]);
    let expanded = expanded_items.contains(&key);
    let toggle_app = app.clone();
    let toggle_key = key.clone();
    let header = theme
        .step_row(
            tool_kind_icon(tool.kind),
            tool_kind_label(tool.kind),
            (!tool.title.trim().is_empty())
                .then(|| SharedString::from(tool.title.trim().to_owned())),
            Some(SharedString::from(tool_status_label(tool.status))),
            tool.status == ToolStatus::Failed,
            Some(expanded),
        )
        .id(SharedString::from(format!("tool-{key}")))
        .w_full()
        .min_w_0()
        .hover(bezel_widgets::step_row_hover)
        .on_click(move |_, window, cx| {
            let toggle_key = toggle_key.clone();
            let _ = toggle_app.update(cx, |this, cx| {
                if !this.expanded_agent_items.remove(&toggle_key) {
                    this.expanded_agent_items.insert(toggle_key);
                }
                cx.notify();
            });
            window.refresh();
        });

    let mut item = v_flex()
        .w_full()
        .max_w_full()
        .min_w_0()
        .flex_none()
        .overflow_hidden()
        .rounded(px(BezelTheme::PANEL_RADIUS))
        .border_1()
        .border_color(theme.border)
        .bg(theme.surface_card)
        .gap_0()
        .child(header);
    if expanded {
        item = item.child(agent_tool_details(tool, theme, &key));
    }
    item.into_any_element()
}

fn agent_tool_details(tool: &AgentTool, theme: &BezelTheme, key: &str) -> gpui::AnyElement {
    let mut sections = Vec::new();
    let input =
        tool.raw_input.as_ref().map(format_tool_value).or_else(|| {
            (tool.kind == AgentToolKind::Execute).then(|| tool.title.trim().to_owned())
        });
    if let Some(input) = input.filter(|input| !input.is_empty()) {
        sections.push(format!("INPUT\n{}", truncate_tool_detail(&input, 24_000)));
    }
    let output = tool.raw_output.as_ref().map(format_tool_value).or_else(|| {
        tool.detail
            .as_deref()
            .filter(|detail| !detail.starts_with("Terminal "))
            .map(ToOwned::to_owned)
    });
    if let Some(output) = output.filter(|output| !output.is_empty()) {
        sections.push(format!("OUTPUT\n{}", truncate_tool_detail(&output, 24_000)));
    }
    if sections.is_empty() {
        sections.push("This agent did not publish captured output over ACP.".to_owned());
    }
    theme
        .step_output(
            SharedString::from(format!("tool-output-{key}")),
            sections.join("\n\n"),
        )
        .into_any_element()
}

fn agent_event_item(item: &AgentTimelineItem, theme: &BezelTheme) -> gpui::AnyElement {
    let (label, text, color) = timeline_item(item);
    v_flex()
        .w_full()
        .max_w_full()
        .min_w_0()
        .flex_none()
        .overflow_hidden()
        .gap_1()
        .px_2()
        .py_1()
        .child(
            div()
                .text_size(px(10.0))
                .font_semibold()
                .text_color(if color == rgb(MUTED_TEXT) {
                    theme.text_muted
                } else {
                    color.into()
                })
                .child(label),
        )
        .child(
            div()
                .w_full()
                .max_w_full()
                .min_w_0()
                .whitespace_normal()
                .text_sm()
                .line_height(px(20.0))
                .child(text),
        )
        .into_any_element()
}

fn agent_item_key(agent: &AgentSessionSnapshot, index: usize, item: &AgentTimelineItem) -> String {
    match item {
        AgentTimelineItem::Tool(tool) => format!("{}:tool:{}", agent.id, tool.id),
        AgentTimelineItem::Message {
            role: AgentMessageRole::Thought,
            message_id,
            ..
        } => format!(
            "{}:thought:{}",
            agent.id,
            message_id.clone().unwrap_or_else(|| index.to_string())
        ),
        _ => format!("{}:item:{index}", agent.id),
    }
}

fn is_expandable_agent_item(item: &AgentTimelineItem) -> bool {
    match item {
        AgentTimelineItem::Tool(_) => true,
        AgentTimelineItem::Message {
            role: AgentMessageRole::Thought,
            text,
            ..
        } => thought_detail(text, &thought_summary(text)).is_some(),
        _ => false,
    }
}

fn thought_summary(text: &str) -> String {
    let line = text
        .trim()
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("Working")
        .trim();
    line.strip_prefix("**")
        .and_then(|line| line.strip_suffix("**"))
        .unwrap_or(line)
        .trim()
        .to_owned()
}

fn thought_detail<'a>(text: &'a str, summary: &str) -> Option<&'a str> {
    let text = text.trim();
    let single_emphasized_line = text
        .strip_prefix("**")
        .and_then(|text| text.strip_suffix("**"))
        .is_some_and(|text| text.trim() == summary);
    (!single_emphasized_line && text != summary && !text.is_empty()).then_some(text)
}

fn format_tool_value(value: &serde_json::Value) -> String {
    value.as_str().map_or_else(
        || serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()),
        ToOwned::to_owned,
    )
}

fn truncate_tool_detail(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_owned();
    }
    let boundary = (0..=limit)
        .rev()
        .find(|index| value.is_char_boundary(*index))
        .unwrap_or_default();
    format!("{}\n… output truncated in this view", &value[..boundary])
}

fn tool_kind_label(kind: AgentToolKind) -> &'static str {
    match kind {
        AgentToolKind::Read => "Read",
        AgentToolKind::Edit => "Edit",
        AgentToolKind::Delete => "Delete",
        AgentToolKind::Move => "Move",
        AgentToolKind::Search => "Search",
        AgentToolKind::Execute => "Run",
        AgentToolKind::Think => "Think",
        AgentToolKind::Fetch => "Fetch",
        AgentToolKind::SwitchMode => "Mode",
        AgentToolKind::Other => "Action",
    }
}

fn tool_kind_icon(kind: AgentToolKind) -> &'static str {
    match kind {
        AgentToolKind::Read => bezel_icons::BOOK,
        AgentToolKind::Edit => bezel_icons::PEN,
        AgentToolKind::Delete => bezel_icons::TRASH_BIN_MINIMALISTIC,
        AgentToolKind::Move => bezel_icons::ARROW_RIGHT,
        AgentToolKind::Search => bezel_icons::MAGNIFER,
        AgentToolKind::Execute => bezel_icons::TERMINAL,
        AgentToolKind::Think => bezel_icons::CPU,
        AgentToolKind::Fetch => bezel_icons::DOWNLOAD,
        AgentToolKind::SwitchMode => bezel_icons::TUNING,
        AgentToolKind::Other => bezel_icons::WIDGET,
    }
}

const fn tool_status_label(status: ToolStatus) -> &'static str {
    match status {
        ToolStatus::Pending => "queued",
        ToolStatus::Running => "running",
        ToolStatus::Completed => "done",
        ToolStatus::Failed => "failed",
    }
}

fn agent_permission_controls(
    app: &gpui::WeakEntity<MuxApp>,
    agent: &AgentSessionSnapshot,
    theme: &BezelTheme,
) -> impl IntoElement {
    let mut controls = v_flex().w_full().min_w_0().px(px(18.0)).pb(px(8.0));
    if let Some(permission) = agent.pending_permission() {
        let mut prompt = v_flex()
            .w_full()
            .max_w(px(720.0))
            .mx_auto()
            .gap_2()
            .child(theme.warning_strip(permission.title.clone()));
        let mut buttons = h_flex().w_full().min_w_0().gap_2().flex_wrap();
        for option in &permission.options {
            let resolve_app = app.clone();
            let session_id = agent.id;
            let request_id = permission.request_id.clone();
            let option_id = option.id.clone();
            buttons = buttons.child(
                Button::new(SharedString::from(format!("permission-{}", option.id)))
                    .label(option.label.clone())
                    .primary()
                    .small()
                    .on_click(move |_, _, cx| {
                        let request_id = request_id.clone();
                        let option_id = option_id.clone();
                        let _ = resolve_app.update(cx, |this, _| {
                            this.backend.send(CommandMessage::ResolveAgentPermission {
                                session_id,
                                request_id,
                                option_id: Some(option_id),
                            });
                        });
                    }),
            );
        }
        prompt = prompt.child(buttons);
        controls = controls.child(prompt);
    }
    controls
}

fn agent_auth_controls(
    app: &gpui::WeakEntity<MuxApp>,
    agent: &AgentSessionSnapshot,
    theme: &BezelTheme,
) -> impl IntoElement {
    let mut controls = v_flex()
        .w_full()
        .max_w(px(720.0))
        .mx_auto()
        .min_w_0()
        .gap_2()
        .px(px(18.0))
        .pb(px(8.0));
    if agent.status == AgentSessionStatus::WaitingForAuthentication {
        controls = controls.child(theme.warning_strip("This agent needs you to sign in."));
        let mut methods = h_flex().w_full().min_w_0().gap_2().flex_wrap();
        for method in &agent.auth_methods {
            let auth_app = app.clone();
            let session_id = agent.id;
            let method_id = method.id.clone();
            methods = methods.child(
                Button::new(SharedString::from(format!("agent-auth-{}", method.id)))
                    .label(format!("Sign in with {}", method.name))
                    .primary()
                    .small()
                    .on_click(move |_, _, cx| {
                        let method_id = method_id.clone();
                        let _ = auth_app.update(cx, |this, _| {
                            this.backend.send(CommandMessage::AuthenticateAgent {
                                session_id,
                                method_id,
                            });
                        });
                    }),
            );
        }
        controls = controls.child(methods);
    }
    controls
}

fn timeline_item(item: &AgentTimelineItem) -> (&'static str, String, gpui::Rgba) {
    match item {
        AgentTimelineItem::Message { role, text, .. } => match role {
            mux_acp::AgentMessageRole::User => ("YOU", text.clone(), rgb(SIGNAL)),
            mux_acp::AgentMessageRole::Agent => ("AGENT", text.clone(), rgb(0x0078_d6a3)),
            mux_acp::AgentMessageRole::Thought => ("THINKING", text.clone(), rgb(MUTED_TEXT)),
        },
        AgentTimelineItem::Tool(tool) => (
            "TOOL",
            format!("{} · {:?}", tool.title, tool.status),
            rgb(0x00d6_ad6b),
        ),
        AgentTimelineItem::Plan(entries) => (
            "PLAN",
            entries
                .iter()
                .map(|entry| format!("• {}", entry.text))
                .collect::<Vec<_>>()
                .join("\n"),
            rgb(0x00b8_9cf2),
        ),
        AgentTimelineItem::Permission(permission) => {
            ("PERMISSION", permission.title.clone(), rgb(0x00d9_9bea))
        }
        AgentTimelineItem::Context { label, characters } => (
            "CONTEXT",
            format!("{label} · {characters} characters"),
            rgb(MUTED_TEXT),
        ),
        AgentTimelineItem::Error(message) => ("ERROR", message.clone(), rgb(0x00ef_7d7d)),
    }
}

fn agent_status_tone(status: AgentSessionStatus, theme: &BezelTheme) -> Hsla {
    match status {
        AgentSessionStatus::Idle => theme.success,
        AgentSessionStatus::Working | AgentSessionStatus::Starting => theme.accent,
        AgentSessionStatus::WaitingForAuthentication
        | AgentSessionStatus::Authenticating
        | AgentSessionStatus::WaitingForPermission => theme.warning,
        AgentSessionStatus::Failed => theme.danger,
        AgentSessionStatus::Closed => theme.text_faint,
    }
}

fn agent_tab_activity(
    statuses: impl IntoIterator<Item = AgentSessionStatus>,
) -> Option<AgentTabActivity> {
    let mut activity = None;
    for status in statuses {
        let candidate = match status {
            AgentSessionStatus::WaitingForAuthentication
            | AgentSessionStatus::WaitingForPermission
            | AgentSessionStatus::Failed => AgentTabActivity::Attention,
            AgentSessionStatus::Starting
            | AgentSessionStatus::Authenticating
            | AgentSessionStatus::Working => AgentTabActivity::Working,
            AgentSessionStatus::Idle => AgentTabActivity::Idle,
            AgentSessionStatus::Closed => continue,
        };
        if candidate == AgentTabActivity::Attention {
            return Some(candidate);
        }
        if candidate == AgentTabActivity::Working || activity.is_none() {
            activity = Some(candidate);
        }
    }
    activity
}

const fn agent_tab_activity_label(activity: AgentTabActivity) -> &'static str {
    match activity {
        AgentTabActivity::Idle => "agent ready",
        AgentTabActivity::Working => "agent working",
        AgentTabActivity::Attention => "agent needs attention",
    }
}

fn agent_tab_activity_tone(activity: AgentTabActivity, theme: &BezelTheme) -> Hsla {
    match activity {
        AgentTabActivity::Idle => theme.success,
        AgentTabActivity::Working => theme.accent,
        AgentTabActivity::Attention => theme.warning,
    }
}

fn interface_animation(duration_ms: u64) -> Animation {
    Animation::new(Duration::from_millis(duration_ms))
        .with_easing(cubic_bezier(0.16, 1.0, 0.3, 1.0))
}

fn pane_needs_live_frame(
    session: &Session,
    active_agent_pane: Option<PaneId>,
    pane_id: PaneId,
) -> bool {
    let Some(tab) = session.active_tab() else {
        return false;
    };
    let included = tab
        .zoomed_pane
        .map_or_else(|| tab.layout.contains(pane_id), |zoomed| zoomed == pane_id);
    included && active_agent_pane != Some(pane_id)
}

const fn agent_status_label(status: AgentSessionStatus) -> &'static str {
    match status {
        AgentSessionStatus::Starting => "starting",
        AgentSessionStatus::WaitingForAuthentication => "sign-in required",
        AgentSessionStatus::Authenticating => "signing in",
        AgentSessionStatus::Idle => "idle",
        AgentSessionStatus::Working => "working",
        AgentSessionStatus::WaitingForPermission => "permission required",
        AgentSessionStatus::Failed => "failed",
        AgentSessionStatus::Closed => "ended",
    }
}

fn terminal_frame_text(frame: &RenderFrame) -> String {
    let columns = usize::from(frame.cols);
    let mut text = String::new();
    for row in 0..usize::from(frame.rows) {
        let start = row * columns;
        let mut line = String::new();
        for cell in &frame.cells[start..start + columns] {
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

fn describe_agent_option(option: &mux_acp::AgentConfigOption) -> String {
    match &option.value {
        AgentConfigValue::Select { current, choices } => format!(
            "{}: {} · choices: {}",
            option.name,
            current,
            choices
                .iter()
                .map(|choice| choice.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        AgentConfigValue::Boolean(current) => format!(
            "{}: {} · choices: on, off",
            option.name,
            if *current { "on" } else { "off" }
        ),
    }
}

fn merge_agent_profiles(settings: &AppSettings) -> Vec<AgentProfile> {
    let custom = settings.custom_agent_profiles();
    let custom_ids = custom
        .iter()
        .map(|profile| profile.id.as_str())
        .collect::<HashSet<_>>();
    let mut profiles = built_in_agent_profiles()
        .into_iter()
        .filter(|profile| !custom_ids.contains(profile.id.as_str()))
        .collect::<Vec<_>>();
    profiles.extend(custom);
    profiles
}

fn parse_cwd_override(value: &str) -> Option<PathBuf> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if value == "~" {
        return directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf());
    }
    if let Some(relative) = value.strip_prefix("~/")
        && let Some(dirs) = directories::BaseDirs::new()
    {
        return Some(dirs.home_dir().join(relative));
    }
    Some(PathBuf::from(value))
}

fn key_chord(keystroke: &gpui::Keystroke) -> Option<KeyChord> {
    let key = match keystroke.key.as_str() {
        "escape" => MuxKey::Escape,
        "enter" => MuxKey::Enter,
        "tab" => MuxKey::Tab,
        "backspace" => MuxKey::Backspace,
        "left" => MuxKey::ArrowLeft,
        "right" => MuxKey::ArrowRight,
        "up" => MuxKey::ArrowUp,
        "down" => MuxKey::ArrowDown,
        value => MuxKey::Character(value.chars().next()?.to_ascii_lowercase()),
    };
    Some(KeyChord {
        key,
        modifiers: mux_modifiers(keystroke.modifiers),
    })
}

fn terminal_input_pane(pending: Option<PaneId>, focused: Option<PaneId>) -> Option<PaneId> {
    pending.or(focused)
}

fn terminal_key_down_target(
    presses: &HashMap<String, PaneId>,
    key: &str,
    held: bool,
    focused: Option<PaneId>,
) -> Option<PaneId> {
    if held {
        presses.get(key).copied()
    } else {
        focused
    }
}

fn take_terminal_key_release(presses: &mut HashMap<String, PaneId>, key: &str) -> Option<PaneId> {
    presses.remove(key)
}

fn mux_modifiers(modifiers: gpui::Modifiers) -> Modifiers {
    let mut result = Modifiers::EMPTY;
    if modifiers.control {
        result = result.union(Modifiers::CONTROL);
    }
    if modifiers.alt {
        result = result.union(Modifiers::ALT);
    }
    if modifiers.shift {
        result = result.union(Modifiers::SHIFT);
    }
    if modifiers.platform {
        result = result.union(Modifiers::SUPER);
    }
    result
}

fn terminal_modifiers(modifiers: gpui::Modifiers) -> TerminalModifiers {
    TerminalModifiers {
        shift: modifiers.shift,
        control: modifiers.control,
        alt: modifiers.alt,
        super_key: modifiers.platform,
    }
}

fn terminal_mouse_button(button: gpui::MouseButton) -> TerminalMouseButton {
    match button {
        gpui::MouseButton::Left => TerminalMouseButton::Left,
        gpui::MouseButton::Right => TerminalMouseButton::Right,
        gpui::MouseButton::Middle => TerminalMouseButton::Middle,
        gpui::MouseButton::Navigate(gpui::NavigationDirection::Back) => TerminalMouseButton::Four,
        gpui::MouseButton::Navigate(gpui::NavigationDirection::Forward) => {
            TerminalMouseButton::Five
        }
    }
}

fn terminal_key_event(
    keystroke: &gpui::Keystroke,
    release: bool,
    held: bool,
    caps_lock: bool,
) -> TerminalKeyEvent {
    let raw_text = (!release)
        .then(|| keystroke.key_char.clone())
        .flatten()
        .filter(|text| !text.is_empty());
    let text = raw_text
        .as_deref()
        .map(|text| terminal_text_with_caps_lock(text, &keystroke.key, caps_lock));
    let key = terminal_key(&keystroke.key);
    let modifiers = terminal_modifiers(keystroke.modifiers);
    let consumed_modifiers = TerminalModifiers {
        shift: raw_text
            .as_ref()
            .is_some_and(|text| text != &keystroke.key && keystroke.modifiers.shift),
        alt: raw_text.is_some() && keystroke.modifiers.alt,
        ..TerminalModifiers::default()
    };
    TerminalKeyEvent {
        action: if release {
            TerminalKeyAction::Release
        } else if held {
            TerminalKeyAction::Repeat
        } else {
            TerminalKeyAction::Press
        },
        key,
        modifiers,
        consumed_modifiers,
        unshifted_codepoint: terminal_codepoint(&keystroke.key, key),
        text,
        composing: false,
    }
}

fn terminal_tab_keystroke(shift: bool) -> gpui::Keystroke {
    gpui::Keystroke {
        modifiers: gpui::Modifiers {
            shift,
            ..gpui::Modifiers::default()
        },
        key: "tab".to_owned(),
        key_char: (!shift).then(|| "\t".to_owned()),
    }
}

#[cfg(target_os = "macos")]
fn terminal_text_with_caps_lock(text: &str, key: &str, caps_lock: bool) -> String {
    if !caps_lock
        || text.len() != 1
        || !text.as_bytes()[0].is_ascii_alphabetic()
        || key.len() != 1
        || !key.as_bytes()[0].is_ascii_alphabetic()
    {
        return text.to_owned();
    }
    let character = char::from(text.as_bytes()[0]);
    if character.is_ascii_lowercase() {
        character.to_ascii_uppercase().to_string()
    } else {
        character.to_ascii_lowercase().to_string()
    }
}

#[cfg(not(target_os = "macos"))]
fn terminal_text_with_caps_lock(text: &str, _key: &str, _caps_lock: bool) -> String {
    text.to_owned()
}

fn terminal_key(key: &str) -> TerminalKey {
    match key {
        "backspace" => TerminalKey::Backspace,
        "enter" => TerminalKey::Enter,
        "tab" => TerminalKey::Tab,
        "space" => TerminalKey::Space,
        "delete" => TerminalKey::Delete,
        "insert" => TerminalKey::Insert,
        "home" => TerminalKey::Home,
        "end" => TerminalKey::End,
        "pageup" => TerminalKey::PageUp,
        "pagedown" => TerminalKey::PageDown,
        "up" => TerminalKey::ArrowUp,
        "down" => TerminalKey::ArrowDown,
        "left" => TerminalKey::ArrowLeft,
        "right" => TerminalKey::ArrowRight,
        "escape" => TerminalKey::Escape,
        "`" => TerminalKey::Backquote,
        "\\" => TerminalKey::Backslash,
        "[" => TerminalKey::BracketLeft,
        "]" => TerminalKey::BracketRight,
        "," => TerminalKey::Comma,
        "=" => TerminalKey::Equal,
        "-" => TerminalKey::Minus,
        "." => TerminalKey::Period,
        "'" => TerminalKey::Quote,
        ";" => TerminalKey::Semicolon,
        "/" => TerminalKey::Slash,
        value if value.len() == 1 => {
            let character = value.chars().next().unwrap_or_default();
            if character.is_ascii_alphabetic() {
                TerminalKey::Letter(character.to_ascii_lowercase())
            } else if character.is_ascii_digit() {
                TerminalKey::Digit(character.to_digit(10).unwrap_or_default() as u8)
            } else {
                TerminalKey::Unidentified
            }
        }
        value if value.starts_with('f') => value[1..]
            .parse::<u8>()
            .map_or(TerminalKey::Unidentified, TerminalKey::Function),
        _ => TerminalKey::Unidentified,
    }
}

fn terminal_codepoint(key: &str, terminal_key: TerminalKey) -> Option<char> {
    match terminal_key {
        TerminalKey::Backspace => Some('\u{8}'),
        TerminalKey::Enter => Some('\r'),
        TerminalKey::Tab => Some('\t'),
        TerminalKey::Space => Some(' '),
        TerminalKey::Escape => Some('\u{1b}'),
        _ => key.chars().next(),
    }
}

fn quit_mux(_: &QuitMux, cx: &mut App) {
    // The daemon owns terminal and ACP process lifetime. Quitting this native
    // client deliberately detaches the GUI without touching the workspace.
    cx.quit();
}

fn configure_application_menu(cx: &mut App) {
    cx.set_menus(vec![Menu {
        name: "Mux".into(),
        disabled: false,
        items: vec![
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Quit Mux", QuitMux),
        ],
    }]);
}

fn configure_application_actions(cx: &mut App) {
    cx.on_action(quit_mux);
    cx.bind_keys([KeyBinding::new("cmd-q", QuitMux, None)]);
    // Terminal panes own Tab; the component root must not turn it into focus traversal.
    cx.bind_keys([
        KeyBinding::new("tab", ForwardTerminalTab, Some("MuxTerminal")),
        KeyBinding::new("shift-tab", ForwardTerminalBacktab, Some("MuxTerminal")),
    ]);
    // gpui-component's Input owns some Option-arrow combinations for text
    // navigation. Bind all four explicitly in the agent pane and composer so
    // pane navigation behaves identically regardless of which child has focus.
    for context in ["MuxAgentPane", "MuxAgentPane > Input"] {
        cx.bind_keys([
            KeyBinding::new("alt-left", NavigateAgentLeft, Some(context)),
            KeyBinding::new("alt-right", NavigateAgentRight, Some(context)),
            KeyBinding::new("alt-up", NavigateAgentUp, Some(context)),
            KeyBinding::new("alt-down", NavigateAgentDown, Some(context)),
        ]);
    }
    cx.bind_keys([KeyBinding::new(
        "ctrl-a",
        ToggleAgentPane,
        Some("MuxAgentPane"),
    )]);
    cx.bind_keys([KeyBinding::new(
        "ctrl-a",
        ToggleAgentPane,
        Some("MuxAgentPane > Input"),
    )]);
    cx.bind_keys([
        KeyBinding::new(
            "up",
            SelectPreviousAgentCompletion,
            Some("MuxAgentPane > MuxAgentCompletion > Input"),
        ),
        KeyBinding::new(
            "down",
            SelectNextAgentCompletion,
            Some("MuxAgentPane > MuxAgentCompletion > Input"),
        ),
        KeyBinding::new(
            "tab",
            InsertAgentCompletion,
            Some("MuxAgentPane > MuxAgentCompletion > Input"),
        ),
        KeyBinding::new(
            "enter",
            InsertAgentCompletion,
            Some("MuxAgentPane > MuxAgentCompletion > Input"),
        ),
        KeyBinding::new(
            "escape",
            DismissAgentCompletion,
            Some("MuxAgentPane > MuxAgentCompletion > Input"),
        ),
    ]);
    configure_application_menu(cx);
}

fn mux_bezel_palette(appearance: BezelAppearance) -> BezelTheme {
    let mut theme = BezelTheme::for_appearance(appearance);
    theme.accent = color(if appearance.is_dark() {
        SIGNAL
    } else {
        0x0018_719b
    });
    theme.accent_strong = theme.accent;
    theme.on_accent = color(if appearance.is_dark() {
        0x0006_151c
    } else {
        0x00f6_fbfe
    });
    theme.caret = theme.accent;
    theme.selection = theme
        .accent
        .opacity(if appearance.is_dark() { 0.28 } else { 0.18 });
    theme
}

fn configure_theme(cx: &mut App) {
    ComponentTheme::change(ThemeMode::Dark, None, cx);
    let bezel = BezelTheme::of(cx).clone();
    let theme = ComponentTheme::global_mut(cx);
    theme.font_family = bezel.font_sans.clone();
    theme.font_size = px(14.0);
    theme.radius = px(BezelTheme::CONTROL_RADIUS);
    theme.radius_lg = px(BezelTheme::SURFACE_RADIUS);
    theme.background = bezel.surface;
    theme.foreground = bezel.text;
    theme.muted = bezel.surface_raised;
    theme.muted_foreground = bezel.text_muted;
    theme.primary = bezel.accent_strong;
    theme.primary_hover = bezel.accent_strong.opacity(0.88);
    theme.primary_active = bezel.accent_strong.opacity(0.72);
    theme.primary_foreground = bezel.on_accent;
    theme.secondary = bezel.surface_raised;
    theme.secondary_hover = bezel.surface_raised_hover;
    theme.secondary_active = bezel.element_active;
    theme.secondary_foreground = bezel.text;
    theme.accent = bezel.element_hover;
    theme.accent_foreground = bezel.text;
    theme.popover = bezel.surface_overlay;
    theme.popover_foreground = bezel.text;
    theme.border = bezel.border;
    theme.input = bezel.border_strong;
    theme.ring = bezel.accent;
    theme.switch = bezel.border_strong;
    theme.switch_thumb = bezel.text;
    theme.tab_bar = bezel.glass();
    theme.tab = bezel.glass();
    theme.tab_active = bezel.element_active;
    theme.tab_foreground = bezel.text_muted;
    theme.tab_active_foreground = bezel.text;
    theme.title_bar = bezel.glass();
    theme.title_bar_border = bezel.border;
    theme.overlay = bezel.scrim();
    theme.danger = bezel.danger;
    theme.danger_foreground = bezel.on_accent;
    theme.warning = bezel.warning;
    theme.warning_foreground = bezel.on_accent;
    theme.success = bezel.success;
    theme.success_foreground = bezel.on_accent;
    theme.info = bezel.accent;
    theme.info_foreground = bezel.on_accent;
}

fn color(value: u32) -> Hsla {
    rgb(value).into()
}

fn terminal_sizes_for_geometry(
    geometry: &layout::WorkspaceGeometry,
    metrics: GridMetrics,
) -> Vec<(PaneId, TerminalSize)> {
    geometry
        .panes
        .iter()
        .map(|pane| {
            let usable_width = (pane.rect.width - metrics.padding_x * 2.0).max(1.0);
            let usable_height = (pane.rect.height - metrics.padding_y * 2.0).max(1.0);
            (
                pane.pane_id,
                TerminalSize {
                    cols: (usable_width / metrics.cell_width)
                        .floor()
                        .clamp(1.0, f32::from(u16::MAX)) as u16,
                    rows: (usable_height / metrics.cell_height)
                        .floor()
                        .clamp(1.0, f32::from(u16::MAX)) as u16,
                    cell_width_px: metrics.cell_width.round() as u32,
                    cell_height_px: metrics.cell_height.round() as u32,
                },
            )
        })
        .collect()
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

fn application_state_dir() -> Option<PathBuf> {
    parse_state_dir()
        .or_else(|| {
            option_env!("MUX_DEFAULT_STATE_APPLICATION").and_then(mux_client::state_dir_for)
        })
        .or_else(mux_client::default_state_dir)
}

fn initialize_graphical_application(cx: &mut App) {
    gpui_component::init(cx);
    bezel_theme::set_palette(mux_bezel_palette, cx);
    bezel_theme::appearance::init(bezel_theme::appearance::AppearanceMode::Dark, cx);
    if let Err(error) = bezel_ui::register_fonts(cx) {
        error!(%error, "failed to register Bezel interface fonts");
    }
    configure_application_actions(cx);
    cx.bind_keys([KeyBinding::new(
        "shift-enter",
        Enter {
            secondary: true,
            shift: true,
        },
        Some("Input"),
    )]);
    cx.bind_keys([KeyBinding::new(
        "escape",
        CancelAgentTurn,
        Some("MuxAgentPane"),
    )]);
    configure_theme(cx);
    if let Err(error) = cx.text_system().add_fonts(vec![
        Cow::Borrowed(
            include_bytes!("../assets/fonts/JetBrainsMonoNerdFontMono-Regular.ttf").as_slice(),
        ),
        Cow::Borrowed(
            include_bytes!("../assets/fonts/JetBrainsMonoNerdFontMono-Bold.ttf").as_slice(),
        ),
        Cow::Borrowed(
            include_bytes!("../assets/fonts/JetBrainsMonoNerdFontMono-Italic.ttf").as_slice(),
        ),
        Cow::Borrowed(
            include_bytes!("../assets/fonts/JetBrainsMonoNerdFontMono-BoldItalic.ttf").as_slice(),
        ),
    ]) {
        error!(%error, "failed to register embedded terminal fonts");
    }
}

fn run_graphical_application(
    state_dir: Option<PathBuf>,
    settings: AppSettings,
    settings_error: Option<String>,
) {
    gpui_platform::application()
        .with_assets(MuxAssets)
        .run(move |cx: &mut App| {
            initialize_graphical_application(cx);
            let bounds = Bounds::centered(None, size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), cx);
            let window = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(560.0), px(360.0))),
                    // Keep a native titled/resizable AppKit window while drawing content through
                    // its transparent titlebar. A missing GPUI titlebar drops the standard
                    // resizable/minimizable/closable style masks on macOS, which also prevents
                    // window managers such as Rectangle from applying a requested frame.
                    titlebar: Some(TitleBar::title_bar_options()),
                    window_background: BezelTheme::of(cx).window_background_appearance(),
                    app_id: Some("dev.mux.terminal".to_owned()),
                    ..Default::default()
                },
                {
                    let state_dir = state_dir.clone();
                    move |window, cx| {
                        bezel_theme::appearance::observe_window(window, cx).detach();
                        let view = cx
                            .new(|cx| MuxApp::new(window, cx, state_dir, settings, settings_error));
                        let host = cx.new(|_| MuxLayerHost { view });
                        cx.new(|cx| gpui_component::Root::new(host, window, cx))
                    }
                },
            );
            match window {
                Ok(window) => {
                    window
                        .update(cx, |_, window, _| window.set_window_title("Mux"))
                        .ok();
                    cx.activate(true);
                }
                Err(error) => {
                    error!(%error, "failed to open GPUI window");
                    cx.quit();
                }
            }
        });
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "mux=info".into()),
        )
        .init();

    if std::env::args_os().any(|argument| argument == "--daemon") {
        let state_dir =
            application_state_dir().ok_or_else(|| anyhow!("no application data directory"))?;
        info!(state_dir = %state_dir.display(), "starting persistent workspace daemon");
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        return runtime.block_on(backend::run_daemon(state_dir));
    }

    let state_dir = application_state_dir();
    let (settings, settings_error) = state_dir.as_deref().map_or_else(
        || {
            (
                AppSettings::default(),
                Some("No application data directory for settings".to_owned()),
            )
        },
        |state_dir| match AppSettings::load(state_dir) {
            Ok(settings) => (settings, None),
            Err(error) => (
                AppSettings::default(),
                Some(format!("Could not load settings: {error:#}")),
            ),
        },
    );

    run_graphical_application(state_dir, settings, settings_error);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::rc::Rc;

    use mux_protocol::PaneAttachment;
    use mux_terminal::{
        TerminalEngine as _, TerminalInteraction as _, TerminalRenderer as _, TerminalSelection,
        TerminalSize,
    };
    use mux_terminal_ghostty::{GhosttyEngine, GhosttyTheme};
    use mux_workspace::{PaneId, Session};

    use super::{
        AgentTabActivity, GridMetrics, PaneOutputUpdate, PaneReplica, PaneScrollState,
        agent_composer_height, agent_pane_is_compact, agent_session_empty_copy,
        agent_session_is_visible, agent_surface_inset, agent_tab_activity,
        agent_tab_activity_label, format_agent_context_usage, format_session_pane_count,
        format_tab_pane_count, input_position_at, layout, pane_needs_live_frame,
        reconcile_pane_replicas, take_terminal_key_release, terminal_frame_text,
        terminal_input_pane, terminal_key_down_target, terminal_key_event,
        terminal_sizes_for_geometry, terminal_tab_keystroke,
    };
    use mux_acp::AgentSessionStatus;

    #[test]
    fn agent_completion_cursor_positions_use_utf16_columns() {
        let position = input_position_at("first\n😀x", "first\n😀".len());
        assert_eq!(position.line, 1);
        assert_eq!(position.character, 2);
    }

    #[test]
    fn agent_completion_overlay_stays_above_multiline_drafts() {
        assert!((agent_composer_height("one line", 800.0) - 57.0).abs() < f32::EPSILON);
        assert!((agent_composer_height("one\ntwo\nthree", 800.0) - 97.0).abs() < f32::EPSILON);
        assert!((agent_composer_height("a\nb\nc\nd\ne\nf\ng", 800.0) - 157.0).abs() < f32::EPSILON);
    }

    #[test]
    fn agent_surfaces_share_a_readable_responsive_measure() {
        assert!((agent_surface_inset(500.0) - 8.0).abs() < f32::EPSILON);
        assert!((agent_surface_inset(760.0) - 8.0).abs() < f32::EPSILON);
        assert!((agent_surface_inset(1_000.0) - 120.0).abs() < f32::EPSILON);
        assert!(agent_pane_is_compact(619.0));
        assert!(!agent_pane_is_compact(620.0));
    }

    #[test]
    fn agent_context_usage_is_bounded_and_omits_unknown_capacity() {
        assert_eq!(
            format_agent_context_usage(Some(25), Some(100)).as_deref(),
            Some("context 25%")
        );
        assert_eq!(format_agent_context_usage(Some(25), Some(0)), None);
        assert_eq!(format_agent_context_usage(None, Some(100)), None);
        assert_eq!(
            format_agent_context_usage(Some(u64::MAX), Some(1)).as_deref(),
            Some("context 100%")
        );
    }

    #[test]
    fn agent_header_metadata_describes_tab_membership_without_implying_context_sharing() {
        assert_eq!(format_tab_pane_count(1), "1 pane in tab");
        assert_eq!(format_tab_pane_count(3), "3 panes in tab");
    }

    #[test]
    fn session_switcher_uses_human_pane_counts() {
        assert_eq!(format_session_pane_count(1), "1 pane");
        assert_eq!(format_session_pane_count(4), "4 panes");
    }

    #[test]
    fn tab_agent_activity_rolls_up_the_most_urgent_state() {
        assert_eq!(agent_tab_activity([]), None);
        assert_eq!(agent_tab_activity([AgentSessionStatus::Closed]), None);
        assert_eq!(
            agent_tab_activity([AgentSessionStatus::Idle, AgentSessionStatus::Working]),
            Some(AgentTabActivity::Working)
        );
        assert_eq!(
            agent_tab_activity([
                AgentSessionStatus::Working,
                AgentSessionStatus::WaitingForPermission,
                AgentSessionStatus::Idle,
            ]),
            Some(AgentTabActivity::Attention)
        );
        assert_eq!(
            agent_tab_activity_label(AgentTabActivity::Attention),
            "agent needs attention"
        );
    }

    #[test]
    fn ended_agent_sessions_are_hidden_from_the_picker() {
        assert!(agent_session_is_visible(AgentSessionStatus::Idle));
        assert!(agent_session_is_visible(AgentSessionStatus::Failed));
        assert!(!agent_session_is_visible(AgentSessionStatus::Closed));
    }

    #[test]
    fn empty_agent_sessions_explain_their_operational_state() {
        let (ready_label, ready_title, _) =
            agent_session_empty_copy(AgentSessionStatus::Idle, "Codex");
        assert_eq!(ready_label, "SESSION READY");
        assert_eq!(ready_title, "Codex is ready.");

        let (permission_label, permission_title, _) =
            agent_session_empty_copy(AgentSessionStatus::WaitingForPermission, "Claude");
        assert_eq!(permission_label, "DECISION NEEDED");
        assert_eq!(permission_title, "Claude needs permission.");
    }

    #[test]
    fn terminal_tab_actions_still_use_libghostty_encoding() {
        let engine = GhosttyEngine::new(TerminalSize::default()).expect("new terminal");
        let tab = terminal_key_event(&terminal_tab_keystroke(false), false, false, false);
        let backtab = terminal_key_event(&terminal_tab_keystroke(true), false, false, false);

        assert_eq!(engine.encode_key(&tab).expect("encode Tab"), b"\t");
        assert_eq!(
            engine.encode_key(&backtab).expect("encode Backtab"),
            b"\x1b[Z",
        );
    }

    #[test]
    fn terminal_releases_return_to_the_pane_that_owned_the_press() {
        let original = PaneId::new();
        let newly_focused = PaneId::new();
        let mut presses = HashMap::from([("a".to_owned(), original)]);

        assert_eq!(
            terminal_key_down_target(&presses, "a", true, Some(newly_focused)),
            Some(original),
        );
        assert_eq!(take_terminal_key_release(&mut presses, "a"), Some(original));
        assert_eq!(take_terminal_key_release(&mut presses, "a"), None);
    }

    #[test]
    fn unmatched_mux_key_releases_never_acquire_a_terminal_target() {
        let mut presses = HashMap::new();
        let focused = PaneId::new();

        assert_eq!(
            terminal_key_down_target(&presses, "a", true, Some(focused)),
            None
        );
        assert_eq!(take_terminal_key_release(&mut presses, "a"), None);
    }

    #[test]
    fn a_clicked_pane_owns_input_while_its_focus_update_is_in_flight() {
        let focused = PaneId::new();
        let clicked = PaneId::new();

        assert_eq!(
            terminal_input_pane(Some(clicked), Some(focused)),
            Some(clicked)
        );
        assert_eq!(terminal_input_pane(None, Some(focused)), Some(focused));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn terminal_text_applies_macos_caps_lock_without_inventing_shift() {
        let engine = GhosttyEngine::new(TerminalSize::default()).expect("new terminal");
        let lower = gpui::Keystroke {
            modifiers: gpui::Modifiers::default(),
            key: "a".to_owned(),
            key_char: Some("a".to_owned()),
        };
        let upper = gpui::Keystroke {
            modifiers: gpui::Modifiers {
                shift: true,
                ..gpui::Modifiers::default()
            },
            key: "a".to_owned(),
            key_char: Some("A".to_owned()),
        };

        let caps = terminal_key_event(&lower, false, false, true);
        assert_eq!(caps.text.as_deref(), Some("A"));
        assert!(!caps.modifiers.shift);
        assert_eq!(engine.encode_key(&caps).expect("encode Caps+A"), b"A");

        let shift_caps = terminal_key_event(&upper, false, false, true);
        assert_eq!(shift_caps.text.as_deref(), Some("a"));
        assert!(shift_caps.modifiers.shift);
        assert!(shift_caps.consumed_modifiers.shift);
        assert_eq!(
            engine.encode_key(&shift_caps).expect("encode Shift+Caps+A"),
            b"a",
        );
    }

    #[test]
    fn pane_splits_get_independent_terminal_sizes() {
        let pane = PaneId::new();
        let mut session = Session::with_panes("release-check", &[pane]).expect("session");
        session
            .active_tab_mut()
            .expect("tab")
            .split_focused(PaneId::new(), mux_workspace::SplitAxis::Vertical)
            .expect("split pane");
        let geometry = layout::calculate(&session, 1_120.0, 720.0);
        let sizes = terminal_sizes_for_geometry(
            &geometry,
            GridMetrics {
                cell_width: 9.6,
                cell_height: 22.5,
                font_size: 16.0,
                padding_x: 4.0,
                padding_y: 3.0,
            },
        );

        assert_eq!(sizes.len(), 2);
        assert!(sizes.iter().all(|(_, size)| size.cols == 115));
        assert!(sizes.iter().all(|(_, size)| size.rows == 15));
    }

    #[test]
    fn terminal_output_is_published_as_one_immutable_snapshot() {
        let mut engine = GhosttyEngine::new(TerminalSize {
            cols: 20,
            rows: 4,
            ..TerminalSize::default()
        })
        .expect("new terminal");
        let frame = engine.render_frame().expect("initial frame");
        let mut pane = PaneReplica::new(engine, frame);
        let displayed = Rc::clone(&pane.frame);

        assert_eq!(
            pane.apply_output(1, b"one ").expect("first output"),
            PaneOutputUpdate::Applied
        );
        assert_eq!(
            pane.apply_output(2, b"two").expect("second output"),
            PaneOutputUpdate::Applied
        );

        assert!(Rc::ptr_eq(&displayed, &pane.frame));
        assert!(!terminal_frame_text(&displayed).contains("one two"));

        pane.publish_frame().expect("publish frame");

        assert!(!Rc::ptr_eq(&displayed, &pane.frame));
        assert!(!terminal_frame_text(&displayed).contains("one two"));
        assert!(terminal_frame_text(&pane.frame).contains("one two"));
    }

    #[test]
    fn workspace_updates_preserve_gui_local_terminal_selection() {
        let pane_id = PaneId::new();
        let mut local_engine = GhosttyEngine::new(TerminalSize {
            cols: 20,
            rows: 4,
            ..TerminalSize::default()
        })
        .expect("local terminal");
        local_engine
            .apply_output(1, b"hello world")
            .expect("local output");
        local_engine
            .set_selection(Some(TerminalSelection {
                anchor: mux_terminal::TerminalPoint { column: 0, row: 0 },
                focus: mux_terminal::TerminalPoint { column: 4, row: 0 },
                rectangular: false,
            }))
            .expect("local selection");
        let local_frame = local_engine.render_frame().expect("local frame");
        let local_replica = PaneReplica::new(local_engine, local_frame);
        let displayed = Rc::clone(&local_replica.frame);

        let mut daemon_engine = GhosttyEngine::new(TerminalSize {
            cols: 20,
            rows: 4,
            ..TerminalSize::default()
        })
        .expect("daemon terminal");
        daemon_engine
            .apply_output(1, b"hello world")
            .expect("daemon output");
        let attachment = PaneAttachment {
            pane_id,
            terminal: daemon_engine.attachment().expect("daemon attachment"),
            exit_status: None,
        };
        let reconciled = reconcile_pane_replicas(
            HashMap::from([(pane_id, local_replica)]),
            &[attachment],
            &GhosttyTheme::default(),
        )
        .expect("reconcile workspace update");
        let preserved = reconciled.get(&pane_id).expect("preserved pane");

        assert!(Rc::ptr_eq(&displayed, &preserved.frame));
        assert_eq!(
            preserved.engine.selected_text().expect("selected text"),
            Some("hello".to_owned())
        );
    }

    #[test]
    fn workspace_updates_rebuild_a_terminal_replica_that_fell_behind() {
        let pane_id = PaneId::new();
        let size = TerminalSize {
            cols: 20,
            rows: 4,
            ..TerminalSize::default()
        };
        let mut local_engine = GhosttyEngine::new(size).expect("local terminal");
        local_engine.apply_output(1, b"one ").expect("local output");
        let local_frame = local_engine.render_frame().expect("local frame");
        let local_replica = PaneReplica::new(local_engine, local_frame);
        let displayed = Rc::clone(&local_replica.frame);

        let mut daemon_engine = GhosttyEngine::new(size).expect("daemon terminal");
        daemon_engine
            .apply_output(1, b"one ")
            .expect("first daemon output");
        daemon_engine
            .apply_output(2, b"two")
            .expect("second daemon output");
        let attachment = PaneAttachment {
            pane_id,
            terminal: daemon_engine.attachment().expect("daemon attachment"),
            exit_status: None,
        };

        let mut reconciled = reconcile_pane_replicas(
            HashMap::from([(pane_id, local_replica)]),
            &[attachment],
            &GhosttyTheme::default(),
        )
        .expect("reconcile workspace update");
        let repaired = reconciled.get_mut(&pane_id).expect("repaired pane");

        assert!(!Rc::ptr_eq(&displayed, &repaired.frame));
        assert!(terminal_frame_text(&repaired.frame).contains("one two"));
        assert_eq!(
            repaired
                .apply_output(3, b" three")
                .expect("next live output"),
            PaneOutputUpdate::Applied
        );
    }

    #[test]
    fn terminal_replicas_ignore_already_applied_output() {
        let mut engine = GhosttyEngine::new(TerminalSize::default()).expect("terminal");
        engine.apply_output(1, b"once").expect("initial output");
        let frame = engine.render_frame().expect("frame");
        let mut pane = PaneReplica::new(engine, frame);

        assert_eq!(
            pane.apply_output(1, b"once").expect("duplicate output"),
            PaneOutputUpdate::Duplicate
        );
        assert_eq!(pane.engine.next_output_sequence(), 2);
    }

    #[test]
    fn terminal_replicas_report_a_gap_without_advancing_the_engine() {
        let mut engine = GhosttyEngine::new(TerminalSize::default()).expect("terminal");
        let frame = engine.render_frame().expect("frame");
        let mut pane = PaneReplica::new(engine, frame);

        assert_eq!(
            pane.apply_output(2, b"late").expect("detect gap"),
            PaneOutputUpdate::Gap {
                expected: 1,
                actual: 2,
            }
        );
        assert_eq!(pane.engine.next_output_sequence(), 1);
        assert_eq!(
            pane.apply_output(1, b"on time").expect("recover in order"),
            PaneOutputUpdate::Applied
        );
        assert_eq!(pane.engine.next_output_sequence(), 2);
    }

    #[test]
    fn inconsistent_attachment_sequence_is_rejected() {
        let pane_id = PaneId::new();
        let mut engine = GhosttyEngine::new(TerminalSize::default()).expect("terminal");
        engine.apply_output(1, b"checkpointed").expect("output");
        let mut terminal = engine.attachment().expect("attachment");
        terminal.next_sequence = 1;
        let pane = PaneAttachment {
            pane_id,
            terminal,
            exit_status: None,
        };

        let Err(error) = super::restore_pane_replica(&pane, &GhosttyTheme::default()) else {
            panic!("an attachment cursor that disagrees with its checkpoint must be rejected");
        };
        assert!(error.to_string().contains("invalid terminal attachment"));
    }

    #[test]
    fn precise_scroll_accumulates_sub_row_motion() {
        let mut scroll = PaneScrollState::default();

        assert_eq!(scroll.accumulate(0.35, true), 0);
        assert_eq!(scroll.accumulate(0.35, false), 0);
        assert_eq!(scroll.accumulate(0.35, false), 1);
        assert!((scroll.fractional_rows - 0.05).abs() < f32::EPSILON * 4.0);
    }

    #[test]
    fn precise_scroll_reversal_drops_opposing_residue() {
        let mut scroll = PaneScrollState::default();

        assert_eq!(scroll.accumulate(0.8, true), 0);
        assert_eq!(scroll.accumulate(-0.25, false), 0);
        assert!((scroll.fractional_rows + 0.25).abs() < f32::EPSILON);
        assert_eq!(scroll.accumulate(-0.8, false), -1);
    }

    #[test]
    fn a_new_scroll_gesture_does_not_inherit_old_residue() {
        let mut scroll = PaneScrollState::default();

        assert_eq!(scroll.accumulate(0.8, true), 0);
        assert_eq!(scroll.accumulate(0.25, true), 0);
        assert!((scroll.fractional_rows - 0.25).abs() < f32::EPSILON);
    }

    #[test]
    fn hidden_and_replaced_panes_do_not_request_live_frames() {
        let left = mux_workspace::PaneId::new();
        let right = mux_workspace::PaneId::new();
        let mut session =
            mux_workspace::Session::with_panes("performance", &[left, right]).expect("session");

        assert!(!pane_needs_live_frame(&session, Some(left), left));
        assert!(pane_needs_live_frame(&session, Some(left), right));

        session.active_tab_mut().expect("active tab").zoomed_pane = Some(left);
        assert!(!pane_needs_live_frame(&session, None, right));
        assert!(pane_needs_live_frame(&session, None, left));
    }
}
