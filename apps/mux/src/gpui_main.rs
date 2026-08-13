#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

mod backend;
mod gpui_terminal;
mod layout;
mod settings;

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context as _, Result, anyhow};
use backend::{BackendHandle, CommandMessage};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, Application, Bounds, Context, Entity, FocusHandle, Hsla,
    InteractiveElement as _, IntoElement, KeyDownEvent, KeyUpEvent, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled, Window, WindowBackgroundAppearance,
    WindowBounds, WindowOptions, div, px, rgb, size,
};
use gpui_component::{
    Icon, IconName, InteractiveElementExt as _, Selectable as _, Sizable as _, StyledExt as _,
    Theme, ThemeMode, TitleBar, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputState},
    notification::Notification,
    scroll::ScrollableElement as _,
    switch::Switch,
    v_flex,
};
use gpui_terminal::GridMetrics;
use mux_acp::{
    AgentConfigCategory, AgentConfigValue, AgentConfigValueSelection, AgentContext,
    AgentContextKind, AgentEvent, AgentProfile, AgentPrompt, AgentSessionSnapshot,
    AgentSessionStatus, AgentTimelineItem, built_in_agent_profiles,
};
use mux_protocol::{ServerEvent, SessionAttachment, SessionSummary};
use mux_terminal::{
    CellWidth, RenderFrame, Rgb, TerminalEngine, TerminalInteraction, TerminalKey,
    TerminalKeyAction, TerminalKeyEvent, TerminalModifiers, TerminalMouseAction,
    TerminalMouseButton, TerminalMouseEvent, TerminalMouseGeometry, TerminalPoint,
    TerminalRenderer, TerminalSelectionGeometry, TerminalSelectionGestureEvent, TerminalSize,
    TerminalSurfacePosition, TerminalViewportScroll,
};
use mux_terminal_ghostty::{GhosttyEngine, GhosttyFont, GhosttyTheme};
use mux_workspace::{
    Action, InputMode, Key as MuxKey, KeyChord, Keymap, Modifiers, PaneId, Session,
    WorkspaceCommand,
};
use settings::AppSettings;
use tracing::{error, info};

const WINDOW_WIDTH: f32 = 1120.0;
const WINDOW_HEIGHT: f32 = 720.0;
const SURFACE: u32 = 0x0011_131a;
const CHROME: u32 = 0x0017_1a22;
const CHROME_RAISED: u32 = 0x001d_222c;
const BORDER: u32 = 0x002a_303c;
const TEXT: u32 = 0x00e8_ecf3;
const MUTED_TEXT: u32 = 0x008c_96a8;
const SIGNAL: u32 = 0x005e_b6e8;
const EMBEDDED_TERMINAL_FONT: &str = "JetBrainsMono Nerd Font Mono";

enum UserEvent {
    Attached(SessionAttachment),
    Sessions(Vec<SessionSummary>),
    Server(ServerEvent),
    Agents(Vec<AgentSessionSnapshot>),
    AgentStarted(AgentSessionSnapshot),
    Agent(AgentEvent),
    CompatibilityMode {
        daemon_protocol: u16,
        app_protocol: u16,
    },
    BackendError(String),
}

struct PaneReplica {
    engine: GhosttyEngine,
    frame: Arc<RenderFrame>,
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
    Pane,
}

struct MuxApp {
    focus_handle: FocusHandle,
    backend: BackendHandle,
    state_dir: Option<PathBuf>,
    settings: AppSettings,
    profiles: Vec<AgentProfile>,
    session: Option<Session>,
    panes: HashMap<PaneId, PaneReplica>,
    sent_sizes: HashMap<PaneId, TerminalSize>,
    sessions: Vec<SessionSummary>,
    agents: Vec<AgentSessionSnapshot>,
    selected_agent: usize,
    agent_input: Entity<InputState>,
    agent_cwd_input: Entity<InputState>,
    agent_context: AgentContextMode,
    selected_pane: Option<PaneId>,
    selection_drag: Option<PaneId>,
    mouse_reporting_pane: Option<PaneId>,
    selection_clock_origin: Instant,
    keymap: Keymap,
    mode: InputMode,
    metrics: GridMetrics,
    terminal_font: String,
    ghostty_theme: GhosttyTheme,
    clipboard: Option<arboard::Clipboard>,
}

struct MuxLayerHost {
    view: Entity<MuxApp>,
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
        focus_handle.focus(window);
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
        let agent_input = cx.new(|cx| {
            InputState::new(window, cx)
                .auto_grow(1, 5)
                .placeholder("Ask an agent, or type /help…")
        });
        let agent_cwd_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Focused pane cwd (default)"));
        let (events, receiver) = async_channel::unbounded();
        let backend = backend::spawn(events, state_dir.clone());
        backend.send(CommandMessage::ListAgents);
        info!("GPUI workspace view initialized");

        cx.spawn_in(window, async move |entity, cx| {
            while let Ok(event) = receiver.recv().await {
                info!(event = event.label(), "GPUI received backend event");
                let _ = cx.update(|window, app| {
                    let _ = entity.update(app, |this, cx| {
                        this.apply_user_event(event, window, cx);
                        cx.notify();
                    });
                    window.refresh();
                });
            }
        })
        .detach();

        if let Some(message) = settings_error {
            window.push_notification(Notification::warning(message), cx);
        }

        Self {
            focus_handle,
            backend,
            state_dir,
            settings,
            profiles: built_in_agent_profiles(),
            session: None,
            panes: HashMap::new(),
            sent_sizes: HashMap::new(),
            sessions: Vec::new(),
            agents: Vec::new(),
            selected_agent: 0,
            agent_input,
            agent_cwd_input,
            agent_context: AgentContextMode::Pane,
            selected_pane: None,
            selection_drag: None,
            mouse_reporting_pane: None,
            selection_clock_origin: Instant::now(),
            keymap: Keymap::zellij_default(),
            mode: InputMode::Normal,
            metrics: GridMetrics::from_font(font_size),
            terminal_font,
            ghostty_theme: GhosttyTheme::load_user().unwrap_or_default(),
            clipboard: arboard::Clipboard::new().ok(),
        }
    }

    fn apply_user_event(&mut self, event: UserEvent, window: &mut Window, cx: &mut Context<Self>) {
        let result = match event {
            UserEvent::Attached(attachment) => self.attach(attachment),
            UserEvent::Sessions(sessions) => {
                self.sessions = sessions;
                Ok(())
            }
            UserEvent::Server(event) => self.apply_server_event(event),
            UserEvent::Agents(agents) => {
                self.agents = agents;
                self.selected_agent = self.selected_agent.min(self.agents.len().saturating_sub(1));
                Ok(())
            }
            UserEvent::AgentStarted(agent) => {
                self.agents.push(agent);
                self.selected_agent = self.agents.len().saturating_sub(1);
                Ok(())
            }
            UserEvent::Agent(event) => {
                if let Some(agent) = self
                    .agents
                    .iter_mut()
                    .find(|agent| agent.id == event.session_id())
                {
                    agent.apply(&event);
                } else {
                    self.backend.send(CommandMessage::ListAgents);
                }
                Ok(())
            }
            UserEvent::CompatibilityMode {
                daemon_protocol,
                app_protocol,
            } => {
                window.push_notification(
                    Notification::warning(format!(
                        "Attached safely to your older workspace (protocol {daemon_protocol}). Terminal sessions remain available; ACP is paused until this workspace can move to protocol {app_protocol}."
                    )),
                    cx,
                );
                Ok(())
            }
            UserEvent::BackendError(message) => Err(anyhow!(message)),
        };
        if let Err(error) = result {
            error!(%error, "Mux UI update failed");
            window.push_notification(Notification::error(error.to_string()), cx);
        }
    }

    fn attach(&mut self, attachment: SessionAttachment) -> Result<()> {
        let mut panes = HashMap::with_capacity(attachment.panes.len());
        for pane in attachment.panes {
            let checkpoint =
                pane.terminal.checkpoint.as_ref().ok_or_else(|| {
                    anyhow!("daemon returned a non-libghostty terminal attachment")
                })?;
            let mut engine = GhosttyEngine::restore(checkpoint)
                .with_context(|| format!("restore terminal pane {}", pane.pane_id))?;
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
            panes.insert(
                pane.pane_id,
                PaneReplica {
                    engine,
                    frame: Arc::new(frame),
                },
            );
        }
        self.session = Some(attachment.session);
        self.sent_sizes
            .retain(|pane_id, _| panes.contains_key(pane_id));
        self.panes = panes;
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
                    pane.engine
                        .render_frame_into(Arc::make_mut(&mut pane.frame))?;
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

    fn send_workspace(&self, command: WorkspaceCommand) {
        if let Some(session) = &self.session {
            self.backend.send(CommandMessage::Workspace {
                session_id: session.id,
                command,
            });
        }
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
        if window.has_active_dialog(cx) || window.has_active_sheet(cx) {
            cx.propagate();
            return;
        }

        let Some(chord) = key_chord(&event.keystroke) else {
            self.send_terminal_key(&event.keystroke, false, event.is_held);
            cx.stop_propagation();
            return;
        };
        if let Some(action) = self.keymap.resolve(self.mode, chord).cloned() {
            self.perform_action(action, window, cx);
            cx.stop_propagation();
        } else {
            self.send_terminal_key(&event.keystroke, false, event.is_held);
            cx.stop_propagation();
        }
    }

    fn handle_key_up(&mut self, event: &KeyUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) || window.has_active_sheet(cx) {
            cx.propagate();
            return;
        }
        self.send_terminal_key(&event.keystroke, true, false);
        cx.stop_propagation();
    }

    fn send_terminal_key(&mut self, keystroke: &gpui::Keystroke, release: bool, held: bool) {
        if !release && keystroke.modifiers.platform && keystroke.key == "c" {
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
        if !release && keystroke.modifiers.platform && keystroke.key == "v" {
            if let Some(clipboard) = &mut self.clipboard
                && let Ok(text) = clipboard.get_text()
                && let Some(pane_id) = self.focused_pane_id()
                && let Some(pane) = self.panes.get(&pane_id)
                && let Ok(bytes) = pane.engine.encode_paste(&text)
            {
                self.write_focused(bytes);
            }
            return;
        }

        let Some(pane_id) = self.focused_pane_id() else {
            return;
        };
        let Some(pane) = self.panes.get(&pane_id) else {
            return;
        };
        let terminal_event = terminal_key_event(keystroke, release, held);
        match pane.engine.encode_key(&terminal_event) {
            Ok(bytes) => self.write_focused(bytes),
            Err(error) => error!(%error, "encode terminal key"),
        }
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
            Action::OpenAgentSurface => self.open_agents(window, cx),
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
        let profiles = self.profiles.clone();
        let app = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _window, cx| {
            let mut content = v_flex().gap_3();
            content = content.child(
                div()
                    .text_sm()
                    .text_color(rgb(MUTED_TEXT))
                    .child("ACP agents run out of process. Disable integrations you do not use."),
            );
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
                        .rounded_lg()
                        .bg(rgb(CHROME_RAISED))
                        .child(
                            v_flex()
                                .gap_1()
                                .child(div().text_sm().font_semibold().child(title))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(MUTED_TEXT))
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

    #[allow(clippy::too_many_lines)]
    fn open_sessions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.backend.send(CommandMessage::ListSessions);
        let app = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _window, cx| {
            let create_app = app.clone();
            let mut content = v_flex().gap_2().child(
                Button::new("session-new")
                    .label("＋ New session")
                    .primary()
                    .compact()
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
                                this.backend
                                    .send(CommandMessage::CreateSessionForPane { name, pane_id });
                            }
                        });
                        window.close_dialog(cx);
                    }),
            );
            let sessions = app
                .upgrade()
                .map(|entity| entity.read(cx).sessions.clone())
                .unwrap_or_default();
            if sessions.is_empty() {
                content = content.child(
                    div()
                        .text_sm()
                        .text_color(rgb(MUTED_TEXT))
                        .child("Loading sessions…"),
                );
            }
            for session in sessions {
                let attach_app = app.clone();
                let rename_app = app.clone();
                let kill_app = app.clone();
                let session_id = session.id;
                let session_name = session.name.clone();
                let pane_count = session.pane_count;
                let rename_session = session.clone();
                let kill_name = session.name.clone();
                content = content.child(
                    h_flex()
                        .gap_1()
                        .child(
                            Button::new(SharedString::from(format!("session-{session_id}")))
                                .label(format!("{session_name}  ·  {pane_count} panes"))
                                .ghost()
                                .flex_1()
                                .on_click(move |_, window, cx| {
                                    let _ = attach_app.update(cx, |this, _| {
                                        this.backend
                                            .send(CommandMessage::AttachSession(session_id));
                                    });
                                    window.close_dialog(cx);
                                }),
                        )
                        .child(
                            Button::new(SharedString::from(format!("rename-session-{session_id}")))
                                .icon(IconName::ALargeSmall)
                                .ghost()
                                .small()
                                .tooltip("Rename session")
                                .on_click(move |_, window, cx| {
                                    open_rename_session_dialog(
                                        rename_app.clone(),
                                        &rename_session,
                                        window,
                                        cx,
                                    );
                                }),
                        )
                        .child(
                            Button::new(SharedString::from(format!("kill-session-{session_id}")))
                                .icon(IconName::Delete)
                                .ghost()
                                .small()
                                .tooltip("Kill session")
                                .on_click(move |_, window, cx| {
                                    let confirm_app = kill_app.clone();
                                    let kill_name = kill_name.clone();
                                    window.open_dialog(cx, move |dialog, _, _| {
                                        let confirm_app = confirm_app.clone();
                                        dialog
                                            .title(format!("Kill {kill_name}?"))
                                            .confirm()
                                            .on_ok(move |_, window, cx| {
                                                let _ = confirm_app.update(cx, |this, _| {
                                                    this.backend.send(CommandMessage::KillSession(
                                                        session_id,
                                                    ));
                                                });
                                                window.close_all_dialogs(cx);
                                                true
                                            })
                                            .child(
                                                div().text_sm().child(
                                                    "All processes in this session will exit.",
                                                ),
                                            )
                                    });
                                }),
                        ),
                );
            }
            dialog.title("Sessions").w(px(460.0)).child(content)
        });
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

    fn open_agents(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.backend.send(CommandMessage::ListAgents);
        let app = cx.weak_entity();
        window.open_sheet(cx, move |sheet, _window, cx| {
            let Some(entity) = app.upgrade() else {
                return sheet;
            };
            let this = entity.read(cx);
            let mut content = v_flex().gap_3();

            content = content.child(agent_session_picker(&app, this));
            if let Some(agent) = this.agents.get(this.selected_agent) {
                content = content.child(agent_timeline(agent));
                content = content.child(agent_configuration(&app, agent));
                content = content.child(agent_auth_controls(&app, agent));
                content = content.child(agent_permission_controls(&app, agent));
                let prompt_app = app.clone();
                content = content.child(
                    v_flex()
                        .gap_2()
                        .mt_2()
                        .child(Input::new(&this.agent_input).h(px(92.0)))
                        .child(
                            h_flex()
                                .justify_between()
                                .child(div().text_xs().text_color(rgb(MUTED_TEXT)).child(
                                    if this.agent_context == AgentContextMode::Pane {
                                        "Context: focused pane"
                                    } else {
                                        "Context: none"
                                    },
                                ))
                                .child(
                                    Button::new("agent-send")
                                        .label("Send")
                                        .primary()
                                        .compact()
                                        .on_click(move |_, window, cx| {
                                            let _ = prompt_app.update(cx, |this, cx| {
                                                this.submit_agent_prompt(window, cx);
                                            });
                                            window.refresh();
                                        }),
                                ),
                        ),
                );
            } else {
                content = content.child(agent_launcher(&app, this));
            }

            sheet
                .title("Agents")
                .size(px(430.0))
                .margin_top(px(layout::TAB_BAR_HEIGHT))
                .child(content)
        });
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
        let Some(agent) = self.agents.get(self.selected_agent) else {
            return;
        };
        let context = self.agent_prompt_context().unwrap_or_default();
        self.backend.send(CommandMessage::PromptAgent {
            session_id: agent.id,
            prompt: AgentPrompt {
                text: draft.to_owned(),
                context,
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
                if let Some(profile) = requested
                    .and_then(|id| self.enabled_profiles().find(|profile| profile.id == id))
                    .or_else(|| self.enabled_profiles().next())
                    .cloned()
                {
                    let value = self.agent_cwd_input.read(cx).value().to_string();
                    self.start_agent(profile, parse_cwd_override(&value));
                }
            }
            "end" | "close" => {
                if let Some(agent) = self.agents.get(self.selected_agent) {
                    self.backend.send(CommandMessage::CloseAgent(agent.id));
                }
            }
            "cancel" => {
                if let Some(agent) = self.agents.get(self.selected_agent) {
                    self.backend.send(CommandMessage::CancelAgent(agent.id));
                }
            }
            "context" => {
                self.agent_context = match parts.next() {
                    Some("none" | "off") => AgentContextMode::None,
                    Some("pane" | "screen" | "on") | None => AgentContextMode::Pane,
                    Some(_) => {
                        window.push_notification(
                            Notification::warning("Usage: /context pane|none"),
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
            "help" => window.push_notification(
                Notification::info(
                    "/new [agent] · /end · /cancel · /context pane|none · /mode <id> · /model <id> · /effort <id>",
                )
                .autohide(false),
                cx,
            ),
            _ => return false,
        }
        true
    }

    fn set_agent_option(
        &self,
        category: AgentConfigCategory,
        requested: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(agent) = self.agents.get(self.selected_agent) else {
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
        let Some(agent) = self.agents.get(self.selected_agent) else {
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
        let pane_id = self
            .focused_pane_id()
            .ok_or_else(|| anyhow!("No focused pane is available"))?;
        let pane = self
            .panes
            .get(&pane_id)
            .ok_or_else(|| anyhow!("Focused pane is unavailable"))?;
        Ok(vec![AgentContext {
            kind: AgentContextKind::TerminalViewport,
            pane_id,
            label: "focused terminal pane".to_owned(),
            text: terminal_frame_text(&pane.frame),
        }])
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
        pane.engine
            .render_frame_into(Arc::make_mut(&mut pane.frame))?;
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
        let Some(frame) = self.panes.get(&pane_id).map(|pane| &pane.frame) else {
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
        self.send_workspace(WorkspaceCommand::SetFocusedPane(pane_id));
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
            self.mouse_reporting_pane = Some(pane_id);
            self.selection_drag = None;
            return;
        }
        if event.button == gpui::MouseButton::Left {
            self.begin_selection(pane_id, rect, event);
        }
    }

    fn pointer_move(&mut self, pane_id: PaneId, rect: layout::Rect, event: &gpui::MouseMoveEvent) {
        if self.mouse_reporting_pane == Some(pane_id)
            && self.report_mouse_event(
                pane_id,
                rect,
                event.position,
                TerminalMouseAction::Motion,
                event.pressed_button.map(terminal_mouse_button),
                event.modifiers,
                event.pressed_button.is_some(),
            )
        {
            return;
        }
        self.drag_selection(pane_id, rect, event);
    }

    fn pointer_up(&mut self, pane_id: PaneId, rect: layout::Rect, event: &gpui::MouseUpEvent) {
        if self.mouse_reporting_pane == Some(pane_id) {
            let _ = self.report_mouse_event(
                pane_id,
                rect,
                event.position,
                TerminalMouseAction::Release,
                Some(terminal_mouse_button(event.button)),
                event.modifiers,
                false,
            );
            self.mouse_reporting_pane = None;
            return;
        }
        if event.button == gpui::MouseButton::Left {
            self.end_selection(pane_id, rect, event);
        }
    }

    fn begin_selection(
        &mut self,
        pane_id: PaneId,
        rect: layout::Rect,
        event: &gpui::MouseDownEvent,
    ) {
        self.send_workspace(WorkspaceCommand::SetFocusedPane(pane_id));
        if self.selected_pane != Some(pane_id)
            && let Some(previous) = self.selected_pane.take()
            && let Some(pane) = self.panes.get_mut(&previous)
        {
            let _ = pane.engine.set_selection(None);
            let _ = pane
                .engine
                .render_frame_into(Arc::make_mut(&mut pane.frame));
        }
        let Some(frame) = self.panes.get(&pane_id).map(|pane| Arc::clone(&pane.frame)) else {
            return;
        };
        let pointer = self.selection_pointer(rect, &frame, event.position);
        let Some(point) = pointer.point else {
            return;
        };
        self.selection_drag = Some(pane_id);
        let time_ns = Instant::now()
            .duration_since(self.selection_clock_origin)
            .as_nanos()
            .min(u128::from(u64::MAX)) as u64;
        let _ = self.apply_selection_gesture(
            pane_id,
            TerminalSelectionGestureEvent::Press {
                point,
                position: pointer.position,
                time_ns,
                repeat_distance: f64::from(self.metrics.cell_width),
                repeat_interval_ns: 500_000_000,
            },
        );
    }

    fn drag_selection(
        &mut self,
        pane_id: PaneId,
        rect: layout::Rect,
        event: &gpui::MouseMoveEvent,
    ) {
        if self.selection_drag != Some(pane_id) || !event.dragging() {
            return;
        }
        let Some(frame) = self.panes.get(&pane_id).map(|pane| Arc::clone(&pane.frame)) else {
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

    fn end_selection(&mut self, pane_id: PaneId, rect: layout::Rect, event: &gpui::MouseUpEvent) {
        if self.selection_drag != Some(pane_id) {
            return;
        }
        let point = self.panes.get(&pane_id).and_then(|pane| {
            self.selection_pointer(rect, &pane.frame, event.position)
                .point
        });
        let _ =
            self.apply_selection_gesture(pane_id, TerminalSelectionGestureEvent::Release { point });
        self.selection_drag = None;
    }

    fn scroll_pane(&mut self, pane_id: PaneId, rect: layout::Rect, event: &gpui::ScrollWheelEvent) {
        let rows = match event.delta {
            gpui::ScrollDelta::Lines(delta) => -delta.y * 3.0,
            gpui::ScrollDelta::Pixels(delta) => -f32::from(delta.y) / self.metrics.cell_height,
        };
        let rows = rows.trunc() as i64;
        if rows == 0 {
            return;
        }
        let wheel_button = if rows < 0 {
            TerminalMouseButton::Four
        } else {
            TerminalMouseButton::Five
        };
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
        if reported && !event.modifiers.shift {
            return;
        }
        if let Some(pane) = self.panes.get_mut(&pane_id)
            && pane
                .engine
                .scroll_viewport(TerminalViewportScroll::Delta(rows))
                .is_ok()
        {
            let _ = pane
                .engine
                .render_frame_into(Arc::make_mut(&mut pane.frame));
        }
    }

    fn sync_terminal_sizes(&mut self, width: f32, height: f32) -> layout::WorkspaceGeometry {
        let Some(session) = &self.session else {
            return layout::WorkspaceGeometry::default();
        };
        let geometry = layout::calculate(session, width, height);
        for pane in &geometry.panes {
            let usable_width = (pane.rect.width - self.metrics.padding_x * 2.0).max(1.0);
            let usable_height = (pane.rect.height - self.metrics.padding_y * 2.0).max(1.0);
            let size = TerminalSize {
                cols: (usable_width / self.metrics.cell_width)
                    .floor()
                    .clamp(1.0, f32::from(u16::MAX)) as u16,
                rows: (usable_height / self.metrics.cell_height)
                    .floor()
                    .clamp(1.0, f32::from(u16::MAX)) as u16,
                cell_width_px: self.metrics.cell_width.round() as u32,
                cell_height_px: self.metrics.cell_height.round() as u32,
            };
            if self.sent_sizes.get(&pane.pane_id) != Some(&size) {
                self.sent_sizes.insert(pane.pane_id, size);
                self.backend.send(CommandMessage::Resize {
                    pane_id: pane.pane_id,
                    size,
                });
            }
        }
        geometry
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut bar = h_flex()
            .h(px(layout::TAB_BAR_HEIGHT))
            .w_full()
            .items_center()
            .gap_1()
            .px_1()
            .bg(rgb(CHROME));
        if cfg!(target_os = "macos") {
            bar = bar.child(div().w(px(70.0)).h_full());
        }
        if let Some(session) = &self.session {
            for tab in &session.tabs {
                let tab_id = tab.id;
                let active = tab_id == session.active_tab;
                bar = bar.child(
                    Button::new(SharedString::from(format!("tab-{tab_id}")))
                        .label(tab.title.clone())
                        .ghost()
                        .small()
                        .compact()
                        .selected(active)
                        .on_click(cx.listener(move |this, _, _, _| {
                            this.send_workspace(WorkspaceCommand::SelectTab(tab_id));
                        })),
                );
            }
        }
        bar.child(
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
        .child(div().w(px(52.0)).h_full())
    }

    fn render_mode_bar(&self) -> impl IntoElement {
        let (label, help) = match self.mode {
            InputMode::Normal => ("NORMAL", ""),
            InputMode::Pane => ("PANE", "d down · n right · arrows focus · x close · f zoom"),
            InputMode::Tab => (
                "TAB",
                "n new · x close · r rename · 1–9 select · arrows switch",
            ),
            InputMode::Session => ("SESSION", "w switch · d detach"),
            InputMode::Resize => ("RESIZE", "arrows resize · Enter finish"),
        };
        h_flex()
            .absolute()
            .left_0()
            .right_0()
            .bottom_0()
            .h(px(30.0))
            .px_3()
            .gap_3()
            .bg(rgb(CHROME))
            .border_t_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .text_xs()
                    .font_semibold()
                    .text_color(rgb(SIGNAL))
                    .child(label),
            )
            .child(div().text_xs().text_color(rgb(MUTED_TEXT)).child(help))
    }
}

impl UserEvent {
    const fn label(&self) -> &'static str {
        match self {
            Self::Attached(_) => "attached",
            Self::Sessions(_) => "sessions",
            Self::Server(_) => "server",
            Self::Agents(_) => "agents",
            Self::AgentStarted(_) => "agent-started",
            Self::Agent(_) => "agent",
            Self::CompatibilityMode { .. } => "compatibility-mode",
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
        let viewport = window.viewport_size();
        let geometry =
            self.sync_terminal_sizes(f32::from(viewport.width), f32::from(viewport.height));
        let pane_count = geometry.panes.len();
        let mut root = div()
            .id("mux-root")
            .relative()
            .size_full()
            .overflow_hidden()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::handle_key_down))
            .on_key_up(cx.listener(Self::handle_key_up))
            .bg(rgb(SURFACE))
            .text_color(rgb(TEXT))
            .child(self.render_tabs(cx));

        for geometry in geometry.panes {
            let pane_id = geometry.pane_id;
            let Some(pane) = self.panes.get(&pane_id) else {
                continue;
            };
            let pointer_app = cx.weak_entity();
            let move_app = pointer_app.clone();
            let release_app = pointer_app.clone();
            let scroll_app = pointer_app.clone();
            let rect = geometry.rect;
            let focused = geometry.focused;
            let mut surface = div()
                .absolute()
                .left(px(geometry.rect.x))
                .top(px(geometry.rect.y))
                .w(px(geometry.rect.width))
                .h(px(geometry.rect.height))
                .overflow_hidden()
                .bg(rgb(SURFACE))
                .on_any_mouse_down(move |event, window, cx| {
                    let _ = pointer_app.update(cx, |this, cx| {
                        this.focus_handle.focus(window);
                        this.pointer_down(pane_id, rect, event);
                        cx.notify();
                    });
                    cx.stop_propagation();
                })
                .on_mouse_move(move |event, _, cx| {
                    let _ = move_app.update(cx, |this, cx| {
                        this.pointer_move(pane_id, rect, event);
                        cx.notify();
                    });
                })
                .capture_any_mouse_up(move |event, _, cx| {
                    let _ = release_app.update(cx, |this, cx| {
                        this.pointer_up(pane_id, rect, event);
                        cx.notify();
                    });
                    cx.stop_propagation();
                })
                .on_scroll_wheel(move |event, _, cx| {
                    let _ = scroll_app.update(cx, |this, cx| {
                        this.scroll_pane(pane_id, rect, event);
                        cx.notify();
                    });
                    cx.stop_propagation();
                })
                .child(gpui_terminal::terminal_canvas(
                    Arc::clone(&pane.frame),
                    self.terminal_font.clone(),
                    self.metrics,
                    focused,
                ));
            if focused && pane_count > 1 {
                // A short "focus beam" is visible at a glance without boxing
                // every pane or stealing terminal pixels with permanent borders.
                surface = surface.child(
                    div()
                        .absolute()
                        .top_0()
                        .left(px(8.0))
                        .w(px(34.0))
                        .h(px(2.0))
                        .rounded_full()
                        .bg(rgb(SIGNAL)),
                );
            }
            root = root.child(surface);
        }

        if self.mode != InputMode::Normal {
            root = root.child(self.render_mode_bar());
        }
        if cfg!(target_os = "macos") {
            root = root.child(macos_window_controls());
        }
        let active_agents = self
            .agents
            .iter()
            .filter(|agent| agent.status != AgentSessionStatus::Closed)
            .count();
        root = root.child(header_actions(cx.weak_entity(), active_agents));
        root
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

fn header_actions(app: gpui::WeakEntity<MuxApp>, active_agents: usize) -> impl IntoElement {
    let settings_app = app.clone();
    h_flex()
        .absolute()
        .top(px(2.0))
        .right(px(4.0))
        .gap_1()
        .child(
            div()
                .id("open-agents")
                .relative()
                .size(px(24.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded_md()
                .text_color(rgb(MUTED_TEXT))
                .hover(|style| style.bg(rgb(CHROME_RAISED)).text_color(rgb(TEXT)))
                .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                    cx.stop_propagation();
                    let _ = app.update(cx, |this, cx| this.open_agents(window, cx));
                })
                .child(Icon::new(IconName::Bot).small())
                .when(active_agents > 0, |button| {
                    button.child(
                        div()
                            .absolute()
                            .top(px(3.0))
                            .right(px(3.0))
                            .size(px(4.0))
                            .rounded_full()
                            .bg(rgb(SIGNAL)),
                    )
                }),
        )
        .child(
            div()
                .id("open-settings")
                .size(px(24.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded_md()
                .text_color(rgb(MUTED_TEXT))
                .hover(|style| style.bg(rgb(CHROME_RAISED)).text_color(rgb(TEXT)))
                .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                    cx.stop_propagation();
                    let _ = settings_app.update(cx, |this, cx| this.open_settings(window, cx));
                })
                .child(Icon::new(IconName::Settings2).small()),
        )
}

fn macos_window_controls() -> impl IntoElement {
    h_flex()
        .absolute()
        .top(px(8.0))
        .left(px(12.0))
        .gap(px(8.0))
        .child(window_control_dot(
            "window-close",
            0x00ff_5f57,
            |window, cx| {
                window.remove_window();
                cx.quit();
            },
        ))
        .child(window_control_dot(
            "window-minimize",
            0x00fe_bc2e,
            |window, _| window.minimize_window(),
        ))
        .child(window_control_dot(
            "window-zoom",
            0x0028_c840,
            |window, _| {
                window.zoom_window();
            },
        ))
}

fn window_control_dot(
    id: &'static str,
    color: u32,
    action: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .size(px(12.0))
        .rounded_full()
        .bg(rgb(color))
        .hover(|style| style.opacity(0.82))
        .on_mouse_down(gpui::MouseButton::Left, |_, window, cx| {
            window.prevent_default();
            cx.stop_propagation();
        })
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            action(window, cx);
        })
}

fn agent_session_picker(app: &gpui::WeakEntity<MuxApp>, this: &MuxApp) -> impl IntoElement {
    let mut picker = h_flex().gap_1().flex_wrap();
    for (index, agent) in this.agents.iter().enumerate() {
        let select_app = app.clone();
        picker = picker.child(
            Button::new(SharedString::from(format!("agent-session-{}", agent.id)))
                .label(agent.name.clone())
                .ghost()
                .small()
                .compact()
                .selected(index == this.selected_agent)
                .on_click(move |_, window, cx| {
                    let _ = select_app.update(cx, |this, cx| {
                        this.selected_agent = index;
                        cx.notify();
                    });
                    window.refresh();
                }),
        );
    }
    let new_app = app.clone();
    picker.child(
        Button::new("agent-new")
            .label("＋ New")
            .ghost()
            .small()
            .compact()
            .on_click(move |_, window, cx| {
                let _ = new_app.update(cx, |this, cx| {
                    this.selected_agent = this.agents.len();
                    cx.notify();
                });
                window.refresh();
            }),
    )
}

fn agent_launcher(app: &gpui::WeakEntity<MuxApp>, this: &MuxApp) -> impl IntoElement {
    let mut launcher = v_flex()
        .gap_2()
        .child(
            div()
                .text_sm()
                .text_color(rgb(MUTED_TEXT))
                .child("Choose an ACP agent. It starts in the focused pane’s current directory."),
        )
        .child(
            v_flex()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(MUTED_TEXT))
                        .child("Working directory"),
                )
                .child(Input::new(&this.agent_cwd_input)),
        );
    for profile in this.enabled_profiles() {
        let profile = profile.clone();
        let start_app = app.clone();
        let name = profile.name.clone();
        let description = profile.description.clone();
        launcher = launcher.child(
            Button::new(SharedString::from(format!("launch-{}", profile.id)))
                .ghost()
                .w_full()
                .child(
                    v_flex()
                        .w(px(370.0))
                        .items_start()
                        .gap_1()
                        .child(div().text_sm().font_semibold().child(name))
                        .child(
                            div()
                                .w_full()
                                .text_xs()
                                .text_color(rgb(MUTED_TEXT))
                                .child(description),
                        ),
                )
                .on_click(move |_, _, cx| {
                    let profile = profile.clone();
                    let _ = start_app.update(cx, |this, app_cx| {
                        let value = this.agent_cwd_input.read(app_cx).value().to_string();
                        this.start_agent(profile, parse_cwd_override(&value));
                    });
                }),
        );
    }
    launcher
}

fn agent_timeline(agent: &AgentSessionSnapshot) -> impl IntoElement {
    let mut timeline = v_flex()
        .gap_2()
        .max_h(px(390.0))
        .overflow_y_scrollbar()
        .p_3()
        .rounded_lg()
        .bg(rgb(CHROME_RAISED))
        .child(
            h_flex()
                .justify_between()
                .child(div().font_semibold().child(agent.name.clone()))
                .child(
                    div()
                        .text_xs()
                        .text_color(status_color(agent.status))
                        .child(format!("{:?}", agent.status)),
                ),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(MUTED_TEXT))
                .child(agent.cwd.display().to_string()),
        );
    for item in &agent.timeline {
        let (label, text, color) = timeline_item(item);
        timeline = timeline.child(
            v_flex()
                .gap_1()
                .py_1()
                .child(
                    div()
                        .text_xs()
                        .font_semibold()
                        .text_color(color)
                        .child(label),
                )
                .child(div().text_sm().child(text)),
        );
    }
    timeline
}

fn agent_configuration(
    app: &gpui::WeakEntity<MuxApp>,
    agent: &AgentSessionSnapshot,
) -> impl IntoElement {
    let mut controls = h_flex().gap_1().flex_wrap();
    for option in &agent.config_options {
        if matches!(
            option.category,
            AgentConfigCategory::Model | AgentConfigCategory::ThoughtLevel
        ) {
            let label = match &option.value {
                AgentConfigValue::Select { current, .. } => format!("{} · {current}", option.name),
                AgentConfigValue::Boolean(value) => {
                    format!("{} · {}", option.name, if *value { "on" } else { "off" })
                }
            };
            let update_app = app.clone();
            let session_id = agent.id;
            let config_id = option.id.clone();
            let next_value = next_config_value(&option.value);
            controls = controls.child(
                Button::new(SharedString::from(format!("agent-option-{}", option.id)))
                    .label(label)
                    .ghost()
                    .small()
                    .compact()
                    .tooltip("Click to cycle, or use /model and /effort")
                    .on_click(move |_, _, cx| {
                        if let Some(value) = next_value.clone() {
                            let config_id = config_id.clone();
                            let _ = update_app.update(cx, |this, _| {
                                this.backend.send(CommandMessage::SetAgentConfig {
                                    session_id,
                                    config_id,
                                    value,
                                });
                            });
                        }
                    }),
            );
        }
    }
    if !agent.modes.is_empty() {
        let mode_app = app.clone();
        let session_id = agent.id;
        let current = agent.current_mode.as_deref();
        let next_mode = agent
            .modes
            .iter()
            .position(|mode| Some(mode.id.as_str()) == current)
            .map_or(0, |index| (index + 1) % agent.modes.len());
        let mode_id = agent.modes[next_mode].id.clone();
        controls = controls.child(
            Button::new("agent-mode")
                .label(format!("Mode · {}", current.unwrap_or("default")))
                .ghost()
                .small()
                .compact()
                .tooltip("Click to cycle, or use /mode")
                .on_click(move |_, _, cx| {
                    let mode_id = mode_id.clone();
                    let _ = mode_app.update(cx, |this, _| {
                        this.backend.send(CommandMessage::SetAgentMode {
                            session_id,
                            mode_id,
                        });
                    });
                }),
        );
    }
    if agent.status == AgentSessionStatus::Working {
        let cancel_app = app.clone();
        let session_id = agent.id;
        controls = controls.child(
            Button::new("agent-cancel")
                .label("Cancel")
                .ghost()
                .small()
                .compact()
                .on_click(move |_, _, cx| {
                    let _ = cancel_app.update(cx, |this, _| {
                        this.backend.send(CommandMessage::CancelAgent(session_id));
                    });
                }),
        );
    }
    let close_app = app.clone();
    let session_id = agent.id;
    controls.child(
        Button::new("agent-end")
            .label("End")
            .danger()
            .small()
            .compact()
            .on_click(move |_, _, cx| {
                let _ = close_app.update(cx, |this, _| {
                    this.backend.send(CommandMessage::CloseAgent(session_id));
                });
            }),
    )
}

fn agent_permission_controls(
    app: &gpui::WeakEntity<MuxApp>,
    agent: &AgentSessionSnapshot,
) -> impl IntoElement {
    let mut controls = v_flex();
    if let Some(permission) = agent.pending_permission() {
        controls = controls
            .gap_2()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(rgb(0x009f_7aea))
            .child(div().font_semibold().child(permission.title.clone()));
        let mut buttons = h_flex().gap_2().flex_wrap();
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
        controls = controls.child(buttons);
    }
    controls
}

fn agent_auth_controls(
    app: &gpui::WeakEntity<MuxApp>,
    agent: &AgentSessionSnapshot,
) -> impl IntoElement {
    let mut controls = h_flex().gap_2().flex_wrap();
    if agent.status == AgentSessionStatus::WaitingForAuthentication {
        for method in &agent.auth_methods {
            let auth_app = app.clone();
            let session_id = agent.id;
            let method_id = method.id.clone();
            controls = controls.child(
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

fn status_color(status: AgentSessionStatus) -> gpui::Rgba {
    match status {
        AgentSessionStatus::Idle => rgb(0x0078_d6a3),
        AgentSessionStatus::Working | AgentSessionStatus::Starting => rgb(SIGNAL),
        AgentSessionStatus::WaitingForPermission => rgb(0x00d9_9bea),
        AgentSessionStatus::Failed => rgb(0x00ef_7d7d),
        _ => rgb(MUTED_TEXT),
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

fn next_config_value(value: &AgentConfigValue) -> Option<AgentConfigValueSelection> {
    match value {
        AgentConfigValue::Select { current, choices } => {
            if choices.is_empty() {
                return None;
            }
            let next = choices
                .iter()
                .position(|choice| choice.id == *current)
                .map_or(0, |index| (index + 1) % choices.len());
            Some(AgentConfigValueSelection::Choice(choices[next].id.clone()))
        }
        AgentConfigValue::Boolean(current) => Some(AgentConfigValueSelection::Boolean(!current)),
    }
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

fn terminal_key_event(keystroke: &gpui::Keystroke, release: bool, held: bool) -> TerminalKeyEvent {
    let text = (!release)
        .then(|| keystroke.key_char.clone())
        .flatten()
        .filter(|text| !text.is_empty());
    let key = terminal_key(&keystroke.key);
    let modifiers = terminal_modifiers(keystroke.modifiers);
    let consumed_modifiers = TerminalModifiers {
        shift: text
            .as_ref()
            .is_some_and(|text| text != &keystroke.key && keystroke.modifiers.shift),
        alt: text.is_some() && keystroke.modifiers.alt,
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

fn configure_theme(cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);
    let theme = Theme::global_mut(cx);
    theme.font_family = ".SystemUIFont".into();
    theme.font_size = px(14.0);
    theme.radius = px(7.0);
    theme.radius_lg = px(11.0);
    theme.background = color(CHROME);
    theme.foreground = color(TEXT);
    theme.muted = color(CHROME_RAISED);
    theme.muted_foreground = color(MUTED_TEXT);
    theme.primary = color(SIGNAL);
    theme.primary_hover = color(0x0072_c4ef);
    theme.primary_active = color(0x0049_9ccb);
    theme.primary_foreground = color(0x0007_1017);
    theme.secondary = color(CHROME_RAISED);
    theme.secondary_hover = color(0x0027_2d39);
    theme.secondary_active = color(0x0031_3846);
    theme.secondary_foreground = color(TEXT);
    theme.accent = color(0x0025_2b36);
    theme.accent_foreground = color(TEXT);
    theme.popover = color(CHROME);
    theme.popover_foreground = color(TEXT);
    theme.border = color(BORDER);
    theme.input = color(0x0034_3a48);
    theme.ring = color(SIGNAL);
    theme.switch = color(0x0034_3a48);
    theme.switch_thumb = color(0x00e9_eef5);
    theme.tab_bar = color(CHROME);
    theme.tab = color(CHROME);
    theme.tab_active = color(CHROME_RAISED);
    theme.tab_foreground = color(MUTED_TEXT);
    theme.tab_active_foreground = color(TEXT);
    theme.title_bar = color(CHROME);
    theme.title_bar_border = color(BORDER);
    theme.overlay = gpui::rgba(0x0000_009e).into();
    theme.danger = color(0x00ef_7d7d);
    theme.danger_foreground = color(0x0016_0808);
    theme.warning = color(0x00d6_ad6b);
    theme.warning_foreground = color(0x0017_1005);
    theme.success = color(0x0078_d6a3);
    theme.success_foreground = color(0x0007_130c);
    theme.info = color(SIGNAL);
    theme.info_foreground = color(0x0007_1017);
}

fn color(value: u32) -> Hsla {
    rgb(value).into()
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

    Application::new()
        .with_assets(gpui_component_assets::Assets)
        .run(move |cx: &mut App| {
            gpui_component::init(cx);
            configure_theme(cx);
            if let Err(error) = cx.text_system().add_fonts(vec![
                Cow::Borrowed(
                    include_bytes!("../assets/fonts/JetBrainsMonoNerdFontMono-Regular.ttf")
                        .as_slice(),
                ),
                Cow::Borrowed(
                    include_bytes!("../assets/fonts/JetBrainsMonoNerdFontMono-Bold.ttf").as_slice(),
                ),
                Cow::Borrowed(
                    include_bytes!("../assets/fonts/JetBrainsMonoNerdFontMono-Italic.ttf")
                        .as_slice(),
                ),
                Cow::Borrowed(
                    include_bytes!("../assets/fonts/JetBrainsMonoNerdFontMono-BoldItalic.ttf")
                        .as_slice(),
                ),
            ]) {
                error!(%error, "failed to register embedded terminal fonts");
            }
            let bounds = Bounds::centered(None, size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), cx);
            let window = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(560.0), px(360.0))),
                    titlebar: (!cfg!(target_os = "macos")).then(TitleBar::title_bar_options),
                    window_background: WindowBackgroundAppearance::Opaque,
                    app_id: Some("dev.mux.terminal".to_owned()),
                    ..Default::default()
                },
                {
                    let state_dir = state_dir.clone();
                    move |window, cx| {
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
    Ok(())
}
