//! The ⌃a agent pane.
//!
//! An ACP session (Claude Code, Codex, GitHub Copilot or Gemini) drawn the way
//! gofer draws a thread: one column grid, a 64px time gutter and then the text.
//! Messages carry a time and a name; steps are single rows (when, what kind,
//! what, how long) that open into a well; the one thing that needs you opens
//! the slab onto the ground. Mono throughout, no icons, lowercase and terse.

use std::path::Path;

use bezel_markdown::{BlockKind, Mark, Text as MarkdownText};
use gpui::{
    AnyElement, Focusable as _, FontStyle, InteractiveText, StrikethroughStyle, StyledText,
    TextRun, UnderlineStyle, font,
};
use mux_acp::{
    AgentDiff, AgentPermission, AgentPlanEntry, AgentStopReason, PermissionKind, PlanStatus,
};

use super::*;

/// gofer's time gutter and text column.
const GUTTER: f32 = 64.0;
const COLUMN: f32 = 80.0;
/// Narrow panes keep the grid but give the gutter back.
const COMPACT_GUTTER: f32 = 40.0;
const COMPACT_COLUMN: f32 = 52.0;
const COMPACT_BELOW: f32 = 520.0;
const SIDE: f32 = 16.0;
const LINE: f32 = 20.0;
const PROSE_SIZE: f32 = 14.0;
const PROSE_LINE: f32 = 22.0;
const DURATION_WIDTH: f32 = 64.0;
/// The longest detail a step's well shows before it keeps only the tail.
const DETAIL_LINES: usize = 60;
const OPTION_KEYS: [&str; 4] = ["a", "b", "c", "d"];
/// The padding gpui-component's text field keeps around its text.
const COMPOSER_PAD_X: f32 = 10.0;
const COMPOSER_PAD_Y: f32 = 8.0;

/// The grid for a pane of this width.
#[derive(Clone, Copy)]
struct Grid {
    gutter: f32,
    column: f32,
    compact: bool,
}

impl Grid {
    fn for_width(width: f32) -> Self {
        if width < COMPACT_BELOW {
            Self {
                gutter: COMPACT_GUTTER,
                column: COMPACT_COLUMN,
                compact: true,
            }
        } else {
            Self {
                gutter: GUTTER,
                column: COLUMN,
                compact: false,
            }
        }
    }

    /// The text column's distance from the gutter's left edge.
    fn gap(self) -> f32 {
        self.column - self.gutter
    }
}

/// Everything one render of the pane needs from the app, gathered up front so
/// the element builders below can borrow it freely.
struct PaneView<'a> {
    app: gpui::WeakEntity<MuxApp>,
    agent: Option<&'a AgentSessionSnapshot>,
    grid: Grid,
    pulse: f32,
    now: u64,
    expanded: &'a HashSet<String>,
    /// The option a pending permission card highlights.
    choice: Option<usize>,
    /// The pulse's step, for marks that blink rather than breathe.
    tick: u8,
}

impl MuxApp {
    /// The agent pane over a slab's body. `frame` is the whole slab.
    #[allow(clippy::too_many_lines)]
    pub(super) fn render_agent_pane(
        &mut self,
        pane_id: PaneId,
        frame: layout::Rect,
        focused: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(tab_id) = self.active_tab_id() else {
            return gpui::Empty.into_any_element();
        };
        // Inside the slab's border, below its head and the head's hairline.
        let rect = layout::Rect {
            x: frame.x + 1.0,
            y: frame.y + 1.0 + layout::PANE_HEAD_HEIGHT,
            width: (frame.width - 2.0).max(1.0),
            height: (frame.height - layout::PANE_HEAD_HEIGHT - 2.0).max(1.0),
        };
        let app = cx.weak_entity();
        let scroll = self.agent_scroll_for(tab_id);
        let follow_tail = self.agent_follow_tail.contains(&tab_id);
        let settle_scroll = self.agent_scroll_needs_settle.remove(&tab_id);
        let active_agent_id = self.active_agent().map(|agent| agent.id);
        if let Some(id) = active_agent_id {
            self.agent_unseen.remove(&id);
        }
        let command_arguments = self.agent_command_arguments();
        self.agent_completion.set_agent_commands(
            active_agent_id
                .and_then(|id| self.agents.iter().find(|agent| agent.id == id))
                .map_or_else(Vec::new, |agent| agent.available_commands.clone()),
        );
        self.agent_completion
            .set_command_arguments(command_arguments);
        self.ensure_agent_pulse(cx);
        self.sync_agent_placeholder(window, cx);
        let draft = self.agent_input.read(cx).value().to_string();
        let composer_focused = self
            .agent_input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        let show_help = self.agent_help_tabs.contains(&tab_id);
        let agent = active_agent_id.and_then(|id| self.agents.iter().find(|agent| agent.id == id));
        let choice = agent
            .and_then(AgentSessionSnapshot::pending_permission)
            .map(|permission| self.permission_choice(permission));
        let view = PaneView {
            app: app.clone(),
            agent,
            grid: Grid::for_width(rect.width),
            pulse: self.agent_pulse_level(),
            now: mux_acp::unix_millis(),
            expanded: &self.expanded_agent_items,
            choice,
            tick: self.agent_pulse,
        };

        let mut body = v_flex()
            .key_context("MuxAgentPane")
            .capture_key_down({
                let app = app.clone();
                move |event, window, cx| {
                    handle_agent_pane_key_down(&app, event, window, cx);
                }
            })
            .on_action({
                let app = app.clone();
                move |_: &CancelAgentTurn, window, cx| {
                    cancel_agent_turn(&app, window, cx);
                }
            })
            .on_action({
                let app = app.clone();
                move |_: &NavigateAgentLeft, window, cx| {
                    navigate_agent_pane(&app, Direction::Left, window, cx);
                    cx.stop_propagation();
                }
            })
            .on_action({
                let app = app.clone();
                move |_: &NavigateAgentRight, window, cx| {
                    navigate_agent_pane(&app, Direction::Right, window, cx);
                    cx.stop_propagation();
                }
            })
            .on_action({
                let app = app.clone();
                move |_: &NavigateAgentUp, window, cx| {
                    navigate_agent_pane(&app, Direction::Up, window, cx);
                    cx.stop_propagation();
                }
            })
            .on_action({
                let app = app.clone();
                move |_: &NavigateAgentDown, window, cx| {
                    navigate_agent_pane(&app, Direction::Down, window, cx);
                    cx.stop_propagation();
                }
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
                move |_: &PreviousAgentChoice, _, cx| {
                    let _ = app.update(cx, |this, cx| {
                        if this.move_agent_choice(-1) {
                            cx.notify();
                        }
                    });
                }
            })
            .on_action({
                let app = app.clone();
                move |_: &NextAgentChoice, _, cx| {
                    let _ = app.update(cx, |this, cx| {
                        if this.move_agent_choice(1) {
                            cx.notify();
                        }
                    });
                }
            })
            .on_action({
                let app = app.clone();
                move |_: &AcceptAgentCompletion, window, cx| {
                    let handled = app
                        .update(cx, |this, cx| {
                            this.accept_agent_completion_on_enter(window, cx)
                        })
                        .unwrap_or(false);
                    if handled {
                        cx.stop_propagation();
                    } else {
                        cx.propagate();
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
            .font_family(EMBEDDED_TERMINAL_FONT)
            .text_size(px(13.0))
            .line_height(px(LINE))
            .text_color(color(TEXT))
            .whitespace_normal()
            // A click anywhere in the pane, opening a step included, leaves
            // the keyboard in the composer so typing and ⌃a carry on.
            .on_any_mouse_down({
                let app = app.clone();
                move |_, window, cx| {
                    let _ = app.update(cx, |this, _cx| {
                        if !focused {
                            this.request_pane_focus(pane_id);
                        }
                        this.focus_agent_composer(window);
                    });
                }
            });

        body = body.child(self.agent_transcript(
            &view,
            tab_id,
            &scroll,
            follow_tail,
            settle_scroll,
            show_help,
            window,
        ));
        if let Some(agent) = agent {
            if let Some(permission) = agent.pending_permission() {
                body = body.child(needs_you_card(&view, agent, permission));
            } else {
                if agent.status == AgentSessionStatus::WaitingForAuthentication {
                    body = body.child(sign_in_card(&view, agent));
                }
                if let Some(line) = working_line(&view, agent) {
                    body = body.child(line);
                }
            }
        }
        if let Some(menu) = self.agent_completion_menu.as_ref() {
            body = body.child(completion_sheet(&view, menu));
        }
        body = body.child(self.agent_composer_view(&view, &draft, composer_focused));

        // The ground, aligned with the window's, under everything: only the
        // needs-you card leaves gaps for it to show through.
        let viewport = window.viewport_size();
        let ground = div()
            .absolute()
            .left(px(-rect.x))
            .top(px(-rect.y))
            .w(viewport.width)
            .h(viewport.height)
            .child(
                img(self.active_ink().ground())
                    .size_full()
                    .object_fit(ObjectFit::Cover),
            );
        div()
            .id(SharedString::from(format!("agent-pane-{pane_id}")))
            .absolute()
            .left(px(rect.x))
            .top(px(rect.y))
            .w(px(rect.width))
            .h(px(rect.height))
            .rounded_b(px(11.0))
            .overflow_hidden()
            .child(ground)
            .child(body)
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn agent_transcript(
        &self,
        view: &PaneView<'_>,
        tab_id: TabId,
        scroll: &ScrollHandle,
        follow_tail: bool,
        settle_scroll: bool,
        show_help: bool,
        window: &mut Window,
    ) -> AnyElement {
        // Keep following streamed content until the user scrolls up. GPUI
        // applies this after layout, so freshly wrapped text is included.
        if follow_tail {
            scroll.scroll_to_bottom();
        }
        if follow_tail && settle_scroll {
            let settled = scroll.clone();
            window.on_next_frame(move |window, _| {
                let maximum = settled.max_offset();
                settled.set_offset(gpui::point(px(0.0), -maximum.y));
                window.refresh();
            });
        }
        let mut content = match view.agent {
            Some(agent) => thread_blocks(view, agent),
            None => self.launcher(view),
        };
        if show_help {
            content.push(help_block(view.grid, view.agent));
        }
        let wheel_app = view.app.clone();
        let wheel_scroll = scroll.clone();
        let release_app = view.app.clone();
        let release_scroll = scroll.clone();
        let thread = div()
            .id(SharedString::from(format!("agent-thread-{tab_id}")))
            .size_full()
            .min_w_0()
            .min_h_0()
            .track_scroll(scroll)
            .overflow_x_hidden()
            .overflow_y_scroll()
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
                let _ = wheel_app.update(cx, |this, cx| {
                    if delta.y > px(0.0) {
                        this.agent_follow_tail.remove(&tab_id);
                        this.agent_scroll_needs_settle.remove(&tab_id);
                    } else {
                        let remaining = f32::from(wheel_scroll.max_offset().y)
                            + f32::from(wheel_scroll.offset().y);
                        if remaining <= 48.0 + f32::from(delta.y.abs()) {
                            this.agent_follow_tail.insert(tab_id);
                        }
                    }
                    cx.notify();
                });
            })
            .child(
                v_flex()
                    .w_full()
                    .min_w_0()
                    .flex_none()
                    .px(px(SIDE))
                    .pt(px(16.0))
                    .pb(px(20.0))
                    .children(content),
            );
        let latest_app = view.app.clone();
        div()
            .relative()
            .w_full()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .bg(color(SURFACE))
            .child(thread)
            .when(!follow_tail, |viewport| {
                viewport.child(
                    h_flex()
                        .absolute()
                        .bottom(px(10.0))
                        .w_full()
                        .justify_center()
                        .child(
                            h_flex()
                                .id(SharedString::from(format!("agent-latest-{tab_id}")))
                                .h(px(28.0))
                                .px(px(14.0))
                                .gap(px(8.0))
                                .rounded(px(14.0))
                                .border_1()
                                .border_color(hairline(0.12))
                                .bg(color(SURFACE))
                                .text_color(color(MUTED_TEXT))
                                .cursor_pointer()
                                .hover(|pill| {
                                    pill.text_color(color(TEXT)).border_color(hairline(0.2))
                                })
                                .on_click(move |_, window, cx| {
                                    let _ = latest_app.update(cx, |this, cx| {
                                        this.agent_follow_tail.insert(tab_id);
                                        this.agent_scroll_for(tab_id).scroll_to_bottom();
                                        cx.notify();
                                    });
                                    window.refresh();
                                })
                                .child("↓ latest"),
                        ),
                )
            })
            .into_any_element()
    }

    /// No agent in this tab yet: where one would start, in dot-matrix, and the
    /// agents this machine can run. ↑↓ choose, enter starts, typing asks the
    /// chosen one straight away.
    fn launcher(&self, view: &PaneView<'_>) -> Vec<AnyElement> {
        let grid = view.grid;
        let cwd = self
            .focused_pane_id()
            .and_then(|pane_id| self.panes.get(&pane_id))
            .and_then(|pane| pane.title.title())
            .map(chrome::terminal_place)
            .filter(|place| place.starts_with('~') || place.starts_with('/'))
            .map(str::to_owned);
        let place = cwd
            .as_deref()
            .map_or_else(|| "agent".to_owned(), place_name);
        let chosen = self.launcher_profile().map(|profile| profile.id.clone());
        let ready = self
            .enabled_profiles()
            .filter(|profile| self.agent_availability.get(&profile.id) == Some(&true))
            .count();
        let mut meta = vec![cwd.map_or_else(
            || "in this pane's directory".to_owned(),
            |cwd| format!("in {cwd}"),
        )];
        if ready > 0 {
            meta.push(format!("{ready} ready"));
        }
        let mut rows = v_flex()
            .mt(px(16.0))
            .ml(px(grid.column - 24.0))
            .items_start()
            .gap(px(2.0));
        for profile in self.enabled_profiles() {
            let available = self.agent_availability.get(&profile.id).copied();
            let selected = chosen.as_deref() == Some(profile.id.as_str());
            rows = rows.child(launcher_row(view, profile, available, selected));
        }
        let to = self
            .launcher_profile()
            .map_or_else(|| "an agent".to_owned(), profile_title);
        vec![
            thread_head(
                grid,
                &place,
                div()
                    .font_weight(FontWeight::BOLD)
                    .child("new agent")
                    .into_any_element(),
                meta.join(" · "),
            ),
            rows.into_any_element(),
            h_flex()
                .ml(px(grid.column))
                .mt(px(16.0))
                .gap_x(px(16.0))
                .gap_y(px(2.0))
                .flex_wrap()
                .text_color(color(FAINT_TEXT))
                .children(self.launcher_hint(&to))
                .into_any_element(),
        ]
    }

    /// The line under the launcher's agents. It reads for the chosen one: how
    /// to start it, or, when it isn't installed, how to get it.
    fn launcher_hint(&self, to: &str) -> Vec<AnyElement> {
        let chosen = self.launcher_profile().map(|profile| profile.id.as_str());
        let command = chosen
            .filter(|_| self.launcher_profile_missing())
            .map(install_command);
        match command {
            None => vec![
                key_hint("↑↓", "choose").into_any_element(),
                key_hint("enter", "start").into_any_element(),
                div()
                    .child(format!("or type to ask {to} straight away"))
                    .into_any_element(),
            ],
            Some(Some(command)) if self.launcher_copied.as_deref() == chosen => vec![
                h_flex()
                    .gap(px(8.0))
                    .child("copied")
                    .child(inline_command(command))
                    .into_any_element(),
                key_hint("⌃a", "paste it in the shell").into_any_element(),
            ],
            Some(Some(command)) => vec![
                key_hint("↑↓", "choose").into_any_element(),
                key_hint("enter", "copy").into_any_element(),
                inline_command(command),
                div().child(format!("to install {to}")).into_any_element(),
            ],
            Some(None) => vec![
                key_hint("↑↓", "choose").into_any_element(),
                div()
                    .child(format!(
                        "{to} isn't on your PATH; set its command in settings"
                    ))
                    .into_any_element(),
            ],
        }
    }

    #[allow(clippy::too_many_lines)]
    fn agent_composer_view(&self, view: &PaneView<'_>, draft: &str, focused: bool) -> AnyElement {
        let grid = view.grid;
        let agent = view.agent;
        let busy = agent.is_some_and(|agent| {
            matches!(
                agent.status,
                AgentSessionStatus::Working | AgentSessionStatus::WaitingForPermission
            )
        });
        let to_name = agent.map_or_else(
            || {
                self.launcher_profile()
                    .map_or_else(|| "agent".to_owned(), profile_title)
            },
            agent_title,
        );
        let mut to_parts = Vec::new();
        if let Some(agent) = agent {
            if let Some(model) = agent_option_label(agent, AgentConfigCategory::Model) {
                to_parts.push(model);
            }
            if let Some(mode) = agent_mode_label(agent) {
                to_parts.push(mode);
            }
            if let Some(context) =
                format_agent_context_usage(agent.context_used, agent.context_size)
            {
                to_parts.push(context);
            }
        }
        let sessions = self.agents_for_active_tab().count();
        let to_app = view.app.clone();
        let to_line = h_flex()
            .id("agent-to")
            .h(px(LINE))
            .max_w_full()
            .min_w_0()
            .flex_none()
            .mx(px(-6.0))
            .px(px(6.0))
            .gap(px(8.0))
            .rounded(px(4.0))
            .text_color(color(FAINT_TEXT))
            .cursor_pointer()
            .hover(|to| to.bg(wash(0.05)))
            .on_click(move |_, window, cx| {
                let _ = to_app.update(cx, |this, cx| {
                    this.prefill_agent_draft(
                        if sessions > 1 { "/use " } else { "/new " },
                        window,
                        cx,
                    );
                });
            })
            .child("to")
            .child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::BOLD)
                    .text_color(color(TEXT))
                    .child(to_name),
            )
            .when(!to_parts.is_empty(), |to| {
                to.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(color(MUTED_TEXT))
                        .child(to_parts.join(" · ")),
                )
            })
            .when(sessions > 1, |to| {
                // One dot per session in this tab: yours in ink, the others
                // in whatever they're doing, so a session working behind this
                // one stays in sight.
                let current = agent.map(|agent| agent.id);
                let dots = self.agents_for_active_tab().map(|candidate| {
                    let tone = if Some(candidate.id) == current {
                        color(TEXT)
                    } else if candidate.pending_permission().is_some() {
                        color(SIGNAL)
                    } else if matches!(
                        candidate.status,
                        AgentSessionStatus::Starting | AgentSessionStatus::Working
                    ) {
                        color(SIGNAL).opacity(view.pulse)
                    } else if self.agent_unseen.contains(&candidate.id) {
                        color(SAGE)
                    } else {
                        hairline(0.28)
                    };
                    div().flex_none().size(px(5.0)).rounded_full().bg(tone)
                });
                to.child(h_flex().flex_none().ml(px(2.0)).gap(px(4.0)).children(dots))
            });

        // An empty composer lends its arrows to the choice above it.
        let choosing =
            draft.is_empty() && agent.is_none_or(|agent| agent.pending_permission().is_some());
        let copies = agent.is_none()
            && self.launcher_profile_missing()
            && self
                .launcher_profile()
                .is_some_and(|profile| install_command(&profile.id).is_some());
        let send_label = if busy {
            "queue"
        } else if copies {
            "copy"
        } else {
            "send"
        };
        let ready = !draft.trim().is_empty();
        let send_app = view.app.clone();
        let send = h_flex()
            .id("agent-send")
            .flex_none()
            .h(px(PROSE_LINE))
            .px(px(8.0))
            .mr(px(-8.0))
            .gap(px(8.0))
            .rounded(px(4.0))
            .text_color(color(FAINT_TEXT))
            .when(ready, |send| {
                send.cursor_pointer()
                    .hover(|send| send.bg(wash(0.05)).text_color(color(TEXT)))
                    .on_click(move |_, window, cx| {
                        let _ = send_app.update(cx, |this, cx| {
                            this.submit_agent_prompt(window, cx);
                        });
                        window.refresh();
                    })
            })
            .child(kbd("enter", color(if ready { TEXT } else { FAINT_TEXT })))
            .child(send_label);

        let note = self
            .agent_note
            .as_ref()
            .filter(|note| Some(note.tab_id) == self.active_tab_id());
        let hint = composer_hint(
            agent,
            busy,
            draft,
            self.agent_completion_menu.is_some(),
            note,
        );
        let prompt_mark = div()
            .absolute()
            .left(px(-16.0))
            .top_0()
            .w(px(16.0))
            .h(px(PROSE_LINE))
            .font_weight(FontWeight::BOLD)
            .text_size(px(PROSE_SIZE))
            .line_height(px(PROSE_LINE))
            .text_color(color(if focused { SIGNAL } else { TEXT }))
            .child("›");
        v_flex()
            .w_full()
            .flex_none()
            .px(px(SIDE))
            .pt(px(10.0))
            .pb(px(10.0))
            .border_t_1()
            .border_color(hairline(if focused { 0.2 } else { 0.07 }))
            .bg(color(SURFACE))
            .key_context(if self.agent_completion_menu.is_some() {
                "MuxAgentCompletion"
            } else if choosing {
                "MuxAgentChoice"
            } else {
                ""
            })
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .pl(px(grid.column))
                    .mb(px(6.0))
                    .child(to_line),
            )
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_start()
                    .gap(px(16.0))
                    .child(
                        div()
                            .relative()
                            .ml(px(grid.column))
                            .flex_1()
                            .min_w_0()
                            .child(prompt_mark)
                            .child(
                                // The field pads its text on every side; pull
                                // it back so the text sits on the column.
                                Textarea::new(&self.agent_input)
                                    .appearance(false)
                                    .bordered(false)
                                    .ml(px(-COMPOSER_PAD_X))
                                    .mr(px(-COMPOSER_PAD_X))
                                    .my(px(-COMPOSER_PAD_Y))
                                    .min_w_0()
                                    .font_family(EMBEDDED_TERMINAL_FONT)
                                    .text_size(px(PROSE_SIZE))
                                    .line_height(px(PROSE_LINE)),
                            ),
                    )
                    .child(send),
            )
            .child(hint_line(hint).ml(px(grid.column)).mt(px(6.0)))
            .into_any_element()
    }
}

/// The blocks of one session's thread, top to bottom.
#[allow(clippy::too_many_lines)]
fn thread_blocks(view: &PaneView<'_>, agent: &AgentSessionSnapshot) -> Vec<AnyElement> {
    let grid = view.grid;
    let working = agent.status == AgentSessionStatus::Working;
    let busy = matches!(
        agent.status,
        AgentSessionStatus::Working | AgentSessionStatus::WaitingForPermission
    );
    let mut blocks: Vec<AnyElement> = Vec::new();
    let mut lines: Vec<AnyElement> = Vec::new();
    let mut turn_started: Option<u64> = None;
    let mut turn_start_index = 0;
    let mut contexts: Vec<(String, usize)> = Vec::new();
    let flush = |blocks: &mut Vec<AnyElement>, lines: &mut Vec<AnyElement>| {
        if !lines.is_empty() {
            blocks.push(
                v_flex()
                    .mt(px(8.0))
                    .w_full()
                    .min_w_0()
                    .children(std::mem::take(lines))
                    .into_any_element(),
            );
        }
    };
    let flush_contexts = |blocks: &mut Vec<AnyElement>, contexts: &mut Vec<(String, usize)>| {
        if !contexts.is_empty() {
            blocks.push(context_line(grid, contexts));
            contexts.clear();
        }
    };
    let last = agent.timeline.len().saturating_sub(1);
    blocks.push(session_head(view, agent));
    if agent.timeline.is_empty() {
        blocks.extend(session_intro(view, agent));
    }
    for (index, item) in agent.timeline.iter().enumerate() {
        if !matches!(item, AgentTimelineItem::Context { .. }) {
            flush_contexts(&mut blocks, &mut contexts);
        }
        match item {
            AgentTimelineItem::Message {
                role: AgentMessageRole::User,
                text,
                at,
                ..
            } => {
                flush(&mut blocks, &mut lines);
                turn_started = *at;
                turn_start_index = index;
                blocks.push(you_block(grid, *at, text));
            }
            AgentTimelineItem::Message {
                role: AgentMessageRole::Agent,
                text,
                at,
                ..
            } => {
                flush(&mut blocks, &mut lines);
                // A blinking cursor while the reply is still arriving.
                let cursor = (working && index == last).then_some(view.tick % 4 < 2);
                blocks.push(says_block(
                    grid,
                    *at,
                    &agent_short_name(agent),
                    text,
                    cursor,
                ));
            }
            AgentTimelineItem::Message {
                role: AgentMessageRole::Thought,
                text,
                at,
                ..
            } => {
                let key = format!("{}:thought:{index}", agent.id);
                let open = view.expanded.contains(&key);
                let running = working && index == last;
                let end = agent
                    .timeline
                    .get(index + 1)
                    .and_then(item_time)
                    .or(running.then_some(view.now));
                let duration = at.zip(end).map(|(start, end)| end.saturating_sub(start));
                lines.push(step_row(
                    view,
                    &key,
                    "think",
                    first_line(text).into(),
                    Vec::new(),
                    if running {
                        StepEnd::Running
                    } else {
                        duration.map_or(StepEnd::Blank, StepEnd::Took)
                    },
                    open,
                ));
                if open {
                    lines.push(detail_well(grid, &[(Tint::Plain, text.trim().to_owned())]));
                }
            }
            AgentTimelineItem::Tool(tool) => {
                let key = format!("{}:{}", agent.id, tool.id);
                let open = view.expanded.contains(&key);
                let waiting = agent
                    .pending_permission()
                    .is_some_and(|permission| permission.tool_call_id == tool.id);
                // An answered question about this step reads as a word on it.
                let answered = agent.timeline.iter().find_map(|item| match item {
                    AgentTimelineItem::Permission(permission)
                        if permission.tool_call_id == tool.id
                            && permission.answered_at.is_some() =>
                    {
                        Some(permission)
                    }
                    _ => None,
                });
                // A step that waited on you is timed from your answer, and one
                // you turned down never ran.
                let ran_for = match answered {
                    Some(permission) if was_declined(permission) => None,
                    Some(permission) => permission
                        .answered_at
                        .zip(tool.finished_at)
                        .map(|(start, end)| end.saturating_sub(start)),
                    None => tool_duration(tool),
                };
                let end = if waiting {
                    StepEnd::NeedsYou
                } else {
                    match (tool.status, turn_end(agent, index)) {
                        // Agents don't always close a step when a turn ends
                        // under it; it stops with the turn either way.
                        (ToolStatus::Running | ToolStatus::Pending, Some(stop)) => {
                            if stop == AgentStopReason::Cancelled {
                                StepEnd::Stopped(None)
                            } else {
                                StepEnd::Blank
                            }
                        }
                        (ToolStatus::Running, None) => StepEnd::Running,
                        (ToolStatus::Pending, None) if working => StepEnd::Running,
                        (ToolStatus::Pending, None) => StepEnd::Blank,
                        // A step you turned down didn't fail; the badge says
                        // what happened.
                        (ToolStatus::Failed, _) if answered.is_some_and(was_declined) => {
                            StepEnd::Blank
                        }
                        (ToolStatus::Failed, _) if cut_short(agent, index, tool) => {
                            StepEnd::Stopped(ran_for)
                        }
                        (ToolStatus::Failed, _) => StepEnd::Failed(ran_for),
                        (ToolStatus::Completed, _) => ran_for.map_or(StepEnd::Blank, StepEnd::Took),
                    }
                };
                lines.push(step_row(
                    view,
                    &key,
                    tool_label(tool),
                    tool_preview(tool, &agent.cwd).into(),
                    diff_badge(tool)
                        .into_iter()
                        .chain(answered.map(decision_badge))
                        .collect(),
                    end,
                    open,
                ));
                if open {
                    lines.push(detail_well(grid, &tool_detail(tool, &agent.cwd)));
                }
            }
            AgentTimelineItem::Plan(entries) => {
                // The plan is one more entry on the turn's ledger.
                let ended = agent.timeline[index..]
                    .iter()
                    .any(|item| matches!(item, AgentTimelineItem::TurnEnded { .. }))
                    || !busy;
                lines.push(plan_block(view, agent, index, entries, ended));
            }
            AgentTimelineItem::Permission(permission) if permission.answered_at.is_some() => {
                // Questions about a step are told on that step's row.
                let about_a_step = agent.timeline.iter().any(|item| {
                    matches!(item, AgentTimelineItem::Tool(tool) if tool.id == permission.tool_call_id)
                });
                if !about_a_step {
                    lines.push(answered_row(grid, permission));
                }
            }
            AgentTimelineItem::Permission(_) => {}
            AgentTimelineItem::Context { label, characters } => {
                contexts.push((label.clone(), *characters));
            }
            AgentTimelineItem::Error(message) => {
                flush(&mut blocks, &mut lines);
                blocks.push(failed_block(grid, message));
            }
            AgentTimelineItem::TurnEnded { at, stop } => {
                flush(&mut blocks, &mut lines);
                let changes = turn_changes(&agent.timeline[turn_start_index..index]);
                blocks.push(turn_end_line(grid, *at, turn_started, *stop, changes));
                turn_started = None;
            }
        }
    }
    flush_contexts(&mut blocks, &mut contexts);
    flush(&mut blocks, &mut lines);
    for text in &agent.queued {
        blocks.push(queued_block(grid, text));
    }
    blocks
}

/// What a session says before anything has happened in it.
fn session_intro(view: &PaneView<'_>, agent: &AgentSessionSnapshot) -> Vec<AnyElement> {
    let grid = view.grid;
    let title = agent_title(agent);
    let (line, tone) = match agent.status {
        AgentSessionStatus::Starting => (
            format!("opening {title}. a first start fetches its adapter, which takes a moment"),
            MUTED_TEXT,
        ),
        AgentSessionStatus::WaitingForAuthentication | AgentSessionStatus::Authenticating => {
            (format!("{title} needs you to sign in first"), SIGNAL)
        }
        AgentSessionStatus::Failed => (format!("{title} could not start"), SIGNAL),
        AgentSessionStatus::Closed => (format!("{title} has stopped"), FAINT_TEXT),
        AgentSessionStatus::Idle
        | AgentSessionStatus::Working
        | AgentSessionStatus::WaitingForPermission => ("ready when you are".to_owned(), MUTED_TEXT),
    };
    vec![
        div()
            .ml(px(grid.column))
            .mt(px(14.0))
            .text_color(color(tone))
            .child(line)
            .into_any_element(),
        h_flex()
            .ml(px(grid.column))
            .mt(px(8.0))
            .gap_x(px(16.0))
            .gap_y(px(2.0))
            .flex_wrap()
            .text_color(color(FAINT_TEXT))
            .child(key_hint("@", "adds a file"))
            .child(key_hint("/", "lists commands"))
            .child(key_hint("⇧tab", "changes mode"))
            .into_any_element(),
    ]
}

/// gofer's slab head, set at the top of the thread rather than over it: one
/// dot-matrix fact (where), then two quiet lines beside it. It scrolls away
/// with the thread, so it never costs the pane a row.
fn thread_head(grid: Grid, place: &str, title: AnyElement, meta: String) -> AnyElement {
    let pitch = if grid.compact { 3.5 } else { 5.0 };
    let place: String = place
        .chars()
        .take(if grid.compact { 8 } else { 12 })
        .collect();
    h_flex()
        .w_full()
        .min_w_0()
        .gap(px(16.0))
        .children(chrome::dot_matrix(&place, pitch, color(TEXT)))
        .child(
            v_flex().min_w_0().child(title).child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(color(FAINT_TEXT))
                    .child(meta),
            ),
        )
        .into_any_element()
}

/// The head of a session's thread: where it works, what it is, since when.
fn session_head(view: &PaneView<'_>, agent: &AgentSessionSnapshot) -> AnyElement {
    let title = h_flex()
        .min_w_0()
        .gap(px(8.0))
        .child(
            div()
                .flex_none()
                .font_weight(FontWeight::BOLD)
                .child(agent_title(agent)),
        )
        .when_some(agent.agent_version.clone(), |title, version| {
            title.child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(color(FAINT_TEXT))
                    .child(version),
            )
        })
        .into_any_element();
    let mut meta = vec![home_relative(&agent.cwd)];
    if agent.started_at > 0 {
        meta.push(format!("since {}", clock(agent.started_at)));
    }
    thread_head(
        view.grid,
        &place_name(&agent.cwd.display().to_string()),
        title,
        meta.join(" · "),
    )
}

/// A directory's last component, as the dot-matrix shows a place.
fn place_name(path: &str) -> String {
    let path = path.trim_end_matches('/');
    match path.rsplit('/').next() {
        Some("~") | None => "home".to_owned(),
        Some("") => "root".to_owned(),
        Some(name) => name.to_lowercase(),
    }
}

/// One agent the launcher can start.
fn launcher_row(
    view: &PaneView<'_>,
    profile: &AgentProfile,
    available: Option<bool>,
    selected: bool,
) -> AnyElement {
    let app = view.app.clone();
    let start = profile.clone();
    let state: AnyElement = match available {
        Some(true) => div()
            .text_color(color(SAGE))
            .child("ready")
            .into_any_element(),
        Some(false) => div()
            .min_w_0()
            .truncate()
            .text_color(color(FAINT_TEXT))
            .child(install_hint(&profile.id))
            .into_any_element(),
        None => div()
            .text_color(color(FAINT_TEXT))
            .child("looking")
            .into_any_element(),
    };
    h_flex()
        .id(SharedString::from(format!("agent-launch-{}", profile.id)))
        .relative()
        .min_w(px(288.0))
        .max_w_full()
        .h(px(24.0))
        .pl(px(24.0))
        .pr(px(12.0))
        .gap(px(16.0))
        .rounded(px(6.0))
        .cursor_pointer()
        .when(selected, |row| row.bg(wash(0.1)))
        .when(!selected, |row| row.hover(|row| row.bg(wash(0.05))))
        .child(
            div()
                .absolute()
                .left(px(8.0))
                .text_color(color(SIGNAL))
                .when(!selected, gpui::Styled::invisible)
                .child("›"),
        )
        .child(
            div()
                .w(px(120.0))
                .flex_none()
                .when(selected, |name| name.font_weight(FontWeight::BOLD))
                .text_color(color(if available == Some(false) {
                    MUTED_TEXT
                } else {
                    TEXT
                }))
                .child(profile_title(profile)),
        )
        .child(state)
        .on_click(move |_, window, cx| {
            let _ = app.update(cx, |this, cx| {
                this.launch_agent_profile(&start, window, cx);
            });
        })
        .into_any_element()
}

/// A key in brackets and what it does, as gofer's hints read.
fn key_hint(key: &'static str, label: &'static str) -> gpui::Div {
    h_flex()
        .flex_none()
        .gap(px(6.0))
        .child(kbd(key, color(MUTED_TEXT)))
        .child(label)
}

/// A shell command set in running text, on the same wash as inline code.
fn inline_command(command: &str) -> AnyElement {
    div()
        .flex_none()
        .px(px(4.0))
        .rounded(px(3.0))
        .bg(wash(0.07))
        .text_color(color(TEXT))
        .child(command.to_owned())
        .into_any_element()
}

/// What /help shows: the commands and keys the pane answers to.
fn help_block(grid: Grid, agent: Option<&AgentSessionSnapshot>) -> AnyElement {
    let mut rows: Vec<(String, String)> = vec![
        (
            "/new [agent]".into(),
            "start another session in this tab".into(),
        ),
        ("/use n".into(), "switch to session n".into()),
        (
            "/model, /mode, /effort".into(),
            "show or change them".into(),
        ),
        (
            "/context tab|none".into(),
            "send the other panes along, or not".into(),
        ),
        (
            "/expand all".into(),
            "open every step; /collapse closes them".into(),
        ),
        (
            "/cancel, /end".into(),
            "interrupt the turn, or end the session".into(),
        ),
    ];
    // The agent's own commands follow under its name, so it's clear which of
    // them mux answers and which go through to the agent.
    let own = rows.len();
    if let Some(agent) = agent {
        rows.extend(agent.available_commands.iter().take(8).map(|command| {
            (
                format!("/{}", command.name),
                sentence_lower(&first_line(&command.description)),
            )
        }));
    }
    let mut list = v_flex().ml(px(grid.column)).mt(px(2.0)).min_w_0();
    for (index, (command, what)) in rows.into_iter().enumerate() {
        if index == own
            && let Some(agent) = agent
        {
            list = list.child(
                div()
                    .mt(px(8.0))
                    .text_color(color(FAINT_TEXT))
                    .child(format!("from {}", agent_short_name(agent))),
            );
        }
        list = list.child(
            h_flex()
                .min_w_0()
                .gap(px(16.0))
                .child(
                    div()
                        .flex_none()
                        .w(px(200.0))
                        .text_color(color(TEXT))
                        .child(command),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(color(MUTED_TEXT))
                        .child(what),
                ),
        );
    }
    v_flex()
        .mt(px(16.0))
        .w_full()
        .min_w_0()
        .child(block_head(grid, None, "help", color(TEXT)))
        .child(list)
        .child(
            h_flex()
                .ml(px(grid.column))
                .mt(px(8.0))
                .gap_x(px(16.0))
                .gap_y(px(2.0))
                .flex_wrap()
                .text_color(color(FAINT_TEXT))
                .child(key_hint("⌃a", "back to the shell"))
                .child(key_hint("⌥←→", "other sessions"))
                .child(key_hint("esc", "interrupt")),
        )
        .into_any_element()
}

/// A block's head line: the time in the gutter, then a bold word.
fn block_head(grid: Grid, at: Option<u64>, title: &str, title_color: Hsla) -> gpui::Div {
    h_flex()
        .w_full()
        .min_w_0()
        .gap(px(grid.gap()))
        .child(
            div()
                .flex_none()
                .w(px(grid.gutter))
                .text_color(color(MUTED_TEXT))
                .child(at.map(clock).unwrap_or_default()),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .font_weight(FontWeight::BOLD)
                .text_color(title_color)
                .child(title.to_owned()),
        )
}

fn you_block(grid: Grid, at: Option<u64>, text: &str) -> AnyElement {
    v_flex()
        .mt(px(16.0))
        .w_full()
        .min_w_0()
        .child(block_head(grid, at, "you", color(TEXT)))
        .child(
            div()
                .relative()
                .ml(px(grid.column))
                .min_w_0()
                .text_size(px(PROSE_SIZE))
                .line_height(px(PROSE_LINE))
                .child(
                    div()
                        .absolute()
                        .left(px(-16.0))
                        .top_0()
                        .text_color(color(MUTED_TEXT))
                        .child("›"),
                )
                .child(text.trim().to_owned()),
        )
        .into_any_element()
}

fn queued_block(grid: Grid, text: &str) -> AnyElement {
    v_flex()
        .mt(px(16.0))
        .w_full()
        .min_w_0()
        .opacity(0.55)
        .child(
            block_head(grid, None, "you", color(TEXT)).child(
                div()
                    .flex_none()
                    .text_color(color(FAINT_TEXT))
                    .child("queued"),
            ),
        )
        .child(
            div()
                .relative()
                .ml(px(grid.column))
                .min_w_0()
                .text_size(px(PROSE_SIZE))
                .line_height(px(PROSE_LINE))
                .child(
                    div()
                        .absolute()
                        .left(px(-16.0))
                        .top_0()
                        .text_color(color(MUTED_TEXT))
                        .child("›"),
                )
                .child(text.trim().to_owned()),
        )
        .into_any_element()
}

fn context_line(grid: Grid, contexts: &[(String, usize)]) -> AnyElement {
    let panes = contexts
        .iter()
        .filter(|(label, _)| label.starts_with("terminal pane"))
        .count();
    let characters: usize = contexts.iter().map(|(_, characters)| characters).sum();
    let mut parts = Vec::new();
    if panes > 0 {
        parts.push(if panes == 1 {
            "1 terminal pane".to_owned()
        } else {
            format!("{panes} terminal panes")
        });
    }
    parts.extend(
        contexts
            .iter()
            .filter(|(label, _)| !label.starts_with("terminal pane"))
            .map(|(label, _)| {
                Path::new(label)
                    .file_name()
                    .map_or_else(|| label.clone(), |name| name.to_string_lossy().into_owned())
            }),
    );
    div()
        .ml(px(grid.column))
        .mt(px(2.0))
        .text_color(color(FAINT_TEXT))
        .child(format!(
            "+ {} · {}",
            parts.join(", "),
            size_label(characters)
        ))
        .into_any_element()
}

fn says_block(
    grid: Grid,
    at: Option<u64>,
    name: &str,
    text: &str,
    cursor: Option<bool>,
) -> AnyElement {
    v_flex()
        .mt(px(16.0))
        .w_full()
        .min_w_0()
        .child(block_head(grid, at, name, color(TEXT)))
        .child(
            div()
                .ml(px(grid.column))
                .min_w_0()
                .child(markdown(text, cursor)),
        )
        .into_any_element()
}

fn failed_block(grid: Grid, message: &str) -> AnyElement {
    v_flex()
        .mt(px(16.0))
        .w_full()
        .min_w_0()
        .child(block_head(grid, None, "failed", color(SIGNAL)))
        .child(
            div()
                .ml(px(grid.column))
                .min_w_0()
                .text_size(px(PROSE_SIZE))
                .line_height(px(PROSE_LINE))
                .text_color(color(SIGNAL))
                .child(message.trim().to_owned()),
        )
        .into_any_element()
}

/// A question the agent asked that no step in the thread stands for.
fn answered_row(grid: Grid, permission: &AgentPermission) -> AnyElement {
    ledger_row(grid, Some("asked"))
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .gap(px(10.0))
                .child(div().min_w_0().truncate().child(ledger_text(
                    grid,
                    "asked",
                    permission_subject(permission),
                )))
                .child(decision_badge(permission)),
        )
        .into_any_element()
}

/// What a turn changed on disk: how many files, lines added and removed.
fn turn_changes(items: &[AgentTimelineItem]) -> Option<(usize, usize, usize)> {
    let mut files = HashSet::new();
    let (mut added, mut removed) = (0, 0);
    for item in items {
        let AgentTimelineItem::Tool(tool) = item else {
            continue;
        };
        if tool.status == ToolStatus::Failed {
            continue;
        }
        for diff in &tool.diffs {
            files.insert(diff.path.as_path());
            let (more, fewer) = diff_stats(diff);
            added += more;
            removed += fewer;
        }
    }
    (!files.is_empty()).then_some((files.len(), added, removed))
}

/// The line that closes a turn: when, how long, why it stopped, and what it
/// changed, so a glance down the thread reads as a log of work done.
fn turn_end_line(
    grid: Grid,
    at: u64,
    started: Option<u64>,
    stop: AgentStopReason,
    changes: Option<(usize, usize, usize)>,
) -> AnyElement {
    let took = started.map(|started| duration_label(at.saturating_sub(started)));
    let (text, tone) = match stop {
        AgentStopReason::EndTurn => (
            took.map_or_else(|| "done".to_owned(), |took| format!("worked {took}")),
            FAINT_TEXT,
        ),
        AgentStopReason::Cancelled => (
            took.map_or_else(
                || "interrupted".to_owned(),
                |took| format!("interrupted after {took}"),
            ),
            FAINT_TEXT,
        ),
        AgentStopReason::MaxTokens => ("stopped at its token limit".to_owned(), SIGNAL),
        AgentStopReason::MaxTurnRequests => ("stopped after too many steps".to_owned(), SIGNAL),
        AgentStopReason::Refusal => ("declined to go on".to_owned(), SIGNAL),
        AgentStopReason::Other => ("stopped".to_owned(), FAINT_TEXT),
    };
    h_flex()
        .mt(px(12.0))
        .w_full()
        .gap(px(grid.gap()))
        .text_color(color(FAINT_TEXT))
        .child(div().flex_none().w(px(grid.gutter)).child(clock(at)))
        .child(
            h_flex()
                .min_w_0()
                .gap(px(6.0))
                .child(div().flex_none().text_color(color(tone)).child(text))
                .when_some(changes, |line, (files, added, removed)| {
                    let files = format!("· {files} {}", if files == 1 { "file" } else { "files" });
                    line.child(div().min_w_0().truncate().child(if grid.compact {
                        files
                    } else {
                        format!("{files} changed")
                    }))
                    .child(
                        div()
                            .flex_none()
                            .text_color(color(SAGE))
                            .child(format!("+{added}")),
                    )
                    .child(div().flex_none().child(format!("−{removed}")))
                }),
        )
        .into_any_element()
}

/// A turn's plan. While the turn runs it lists every step and where each one
/// stands; once the turn is over it folds to a line that opens on a click.
fn plan_block(
    view: &PaneView<'_>,
    agent: &AgentSessionSnapshot,
    index: usize,
    entries: &[AgentPlanEntry],
    ended: bool,
) -> AnyElement {
    let grid = view.grid;
    let done = entries
        .iter()
        .filter(|entry| entry.status == PlanStatus::Completed)
        .count();
    let key = format!("{}:plan:{index}", agent.id);
    let open = !ended || view.expanded.contains(&key);
    let count = format!("{done} of {} done", entries.len());
    let app = view.app.clone();
    let head = ledger_row(grid, Some("plan"))
        .id(SharedString::from(format!("agent-plan-{key}")))
        .when(ended, |row| {
            row.cursor_pointer()
                .when(open, |row| row.bg(wash(0.1)))
                .when(!open, |row| row.hover(|row| row.bg(wash(0.05))))
                .on_click(move |_, _, cx| {
                    let _ = app.update(cx, |this, cx| {
                        if !this.expanded_agent_items.remove(&key) {
                            this.expanded_agent_items.insert(key.clone());
                        }
                        cx.notify();
                    });
                })
        })
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_color(color(MUTED_TEXT))
                .child(ledger_text(grid, "plan", count)),
        );
    if !open {
        return head.into_any_element();
    }
    let mut rows = v_flex().ml(px(grid.column)).mt(px(2.0)).min_w_0();
    for entry in entries {
        let (mark, text_color): (AnyElement, Hsla) = match entry.status {
            PlanStatus::Completed => (
                div()
                    .size(px(7.0))
                    .rounded_full()
                    .bg(color(FAINT_TEXT))
                    .into_any_element(),
                color(FAINT_TEXT),
            ),
            PlanStatus::Running => (
                div()
                    .size(px(9.0))
                    .rounded_full()
                    .border_2()
                    .border_color(color(SIGNAL))
                    .into_any_element(),
                color(TEXT),
            ),
            PlanStatus::Pending => (
                div()
                    .size(px(7.0))
                    .rounded_full()
                    .border_1()
                    .border_color(color(FAINT_TEXT))
                    .into_any_element(),
                color(MUTED_TEXT),
            ),
        };
        rows = rows.child(
            h_flex()
                .min_w_0()
                .items_start()
                .child(
                    div()
                        .relative()
                        .flex_none()
                        .w(px(16.0))
                        .ml(px(-16.0))
                        .h(px(LINE))
                        .flex()
                        .items_center()
                        .child(mark),
                )
                .child(
                    div()
                        .min_w_0()
                        .text_color(text_color)
                        .child(entry.text.trim().to_owned()),
                ),
        );
    }
    v_flex()
        .w_full()
        .min_w_0()
        .child(head)
        .child(rows.mb(px(4.0)))
        .into_any_element()
}

/// How a step row ends on the right.
#[derive(Clone, Copy)]
enum StepEnd {
    Blank,
    Running,
    NeedsYou,
    Took(u64),
    /// Cut short when you interrupted the turn.
    Stopped(Option<u64>),
    Failed(Option<u64>),
}

/// One step of a turn, as a ledger row: what kind of step it was in the
/// gutter, set flush against the text column, then what it touched, then how
/// long it took. The row opens into a well. Steps carry no clock of their own;
/// the turn's messages already say when.
fn step_row(
    view: &PaneView<'_>,
    key: &str,
    verb: &str,
    text: SharedString,
    badges: Vec<AnyElement>,
    end: StepEnd,
    open: bool,
) -> AnyElement {
    let grid = view.grid;
    let app = view.app.clone();
    let toggle = key.to_owned();
    let failed = matches!(end, StepEnd::Failed(_));
    let running = matches!(end, StepEnd::Running);
    let right: Option<AnyElement> = match end {
        StepEnd::Blank | StepEnd::Running => None,
        StepEnd::NeedsYou => Some(
            div()
                .text_color(color(SIGNAL))
                .child("needs you")
                .into_any_element(),
        ),
        StepEnd::Took(ms) => Some(div().child(duration_label(ms)).into_any_element()),
        StepEnd::Stopped(ms) => Some(
            div()
                .child(ms.map_or_else(
                    || "stopped".to_owned(),
                    |ms| format!("stopped {}", duration_label(ms)),
                ))
                .into_any_element(),
        ),
        StepEnd::Failed(ms) => Some(
            div()
                .text_color(color(SIGNAL))
                .child(ms.map_or_else(
                    || "failed".to_owned(),
                    |ms| format!("failed {}", duration_label(ms)),
                ))
                .into_any_element(),
        ),
    };
    ledger_row(grid, Some(verb))
        .id(SharedString::from(format!("agent-step-{key}")))
        .cursor_pointer()
        .when(open, |row| row.bg(wash(0.1)))
        .when(!open, |row| row.hover(|row| row.bg(wash(0.05))))
        .active(|row| row.bg(wash(0.1)))
        .on_click(move |_, _, cx| {
            let _ = app.update(cx, |this, cx| {
                if !this.expanded_agent_items.remove(&toggle) {
                    this.expanded_agent_items.insert(toggle.clone());
                }
                cx.notify();
            });
        })
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .gap(px(10.0))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(color(if failed || verb == "think" {
                            MUTED_TEXT
                        } else {
                            TEXT
                        }))
                        .child(ledger_text(grid, verb, text)),
                )
                .children(badges)
                .when(running, |row| row.child(pulse_dot(view.pulse, 6.0))),
        )
        .when_some(right, |row, right| {
            row.child(
                h_flex()
                    .flex_none()
                    .min_w(px(DURATION_WIDTH))
                    .justify_end()
                    .text_color(color(FAINT_TEXT))
                    .child(right),
            )
        })
        .into_any_element()
}

/// A row on the thread's ledger: a quiet word in the gutter, flush against
/// the text column, and whatever the caller adds after it. Rows reach 8px
/// past the column on either side so a hover wash has room to breathe.
fn ledger_row(grid: Grid, verb: Option<&str>) -> gpui::Div {
    // A narrow pane has no room for the word; ledger_text leads with it.
    let verb = verb.filter(|_| !grid.compact);
    h_flex()
        .w_full()
        .min_w_0()
        .mx(px(-8.0))
        .px(px(8.0))
        .py(px(2.0))
        .rounded(px(6.0))
        .gap(px(grid.gap()))
        .child(
            h_flex()
                .flex_none()
                .w(px(grid.gutter))
                .justify_end()
                .overflow_hidden()
                .text_color(color(FAINT_TEXT))
                .children(verb.map(str::to_owned)),
        )
}

/// A ledger row's text, led by its verb when the pane is too narrow for the
/// gutter to carry it.
fn ledger_text(grid: Grid, verb: &str, text: impl Into<SharedString>) -> SharedString {
    let text = text.into();
    if grid.compact {
        format!("{verb} {text}").into()
    } else {
        text
    }
}

/// Which kind of answer a permission got, if it got one.
fn selected_kind(permission: &AgentPermission) -> Option<PermissionKind> {
    permission.selected_option.as_ref().and_then(|selected| {
        permission
            .options
            .iter()
            .find(|option| option.id == *selected)
            .map(|option| option.kind)
    })
}

/// Whether you turned the step down, or let the question lapse.
fn was_declined(permission: &AgentPermission) -> bool {
    !matches!(
        selected_kind(permission),
        Some(PermissionKind::AllowOnce | PermissionKind::AllowAlways)
    )
}

/// How the turn holding the item at `index` ended, once it has.
fn turn_end(agent: &AgentSessionSnapshot, index: usize) -> Option<AgentStopReason> {
    agent.timeline[index + 1..]
        .iter()
        .map_while(|item| match item {
            AgentTimelineItem::Message {
                role: AgentMessageRole::User,
                ..
            } => None,
            AgentTimelineItem::TurnEnded { stop, .. } => Some(Some(*stop)),
            _ => Some(None),
        })
        .find_map(|stop| stop)
}

/// Whether a failed step was still running when you stopped the turn, rather
/// than failing on its own and the agent carrying on past it.
fn cut_short(agent: &AgentSessionSnapshot, index: usize, tool: &AgentTool) -> bool {
    for item in &agent.timeline[index + 1..] {
        match item {
            AgentTimelineItem::TurnEnded { stop, .. } => {
                return *stop == AgentStopReason::Cancelled;
            }
            AgentTimelineItem::Message { .. } => return false,
            AgentTimelineItem::Tool(later)
                if later
                    .started_at
                    .zip(tool.finished_at)
                    .is_some_and(|(start, end)| start > end) =>
            {
                return false;
            }
            _ => {}
        }
    }
    false
}

/// How a permission was answered, as a word after the step it was about.
fn decision_badge(permission: &AgentPermission) -> AnyElement {
    let (word, tone) = match selected_kind(permission) {
        Some(PermissionKind::AllowOnce) => ("allowed", FAINT_TEXT),
        Some(PermissionKind::AllowAlways) => ("always allowed", FAINT_TEXT),
        Some(PermissionKind::RejectOnce) => ("rejected", SIGNAL),
        Some(PermissionKind::RejectAlways) => ("always rejected", SIGNAL),
        None => ("not answered", FAINT_TEXT),
    };
    div()
        .flex_none()
        .text_color(color(tone))
        .child(word)
        .into_any_element()
}

/// A peach dot that breathes with the pane's pulse.
fn pulse_dot(level: f32, size: f32) -> AnyElement {
    div()
        .flex_none()
        .size(px(size))
        .rounded_full()
        .bg(color(SIGNAL).opacity(level))
        .into_any_element()
}

/// The pulse as an opacity: a slow breath over eight ticks.
pub(super) fn pulse_level(tick: u8) -> f32 {
    let phase = f32::from(tick % 8) / 8.0 * std::f32::consts::TAU;
    0.45 + 0.55 * (0.5 + 0.5 * phase.cos())
}

/// How a line in a well is coloured: commands and additions stand forward,
/// removals and paths step back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tint {
    Plain,
    /// What a step printed.
    Output,
    Command,
    Added,
    Removed,
    Note,
    Error,
}

/// A step opened up: its command, diff and output in a dark well. Each run of
/// added or removed lines sits on its own faint wash, edge to edge, so a diff
/// reads at a glance; output that follows a diff steps down from it.
fn detail_well(grid: Grid, lines: &[(Tint, String)]) -> AnyElement {
    let mut well = v_flex()
        .mt(px(4.0))
        .mb(px(8.0))
        .ml(px((grid.column - 12.0).max(0.0)))
        .px(px(12.0))
        .py(px(8.0))
        .rounded(px(6.0))
        .bg(gpui::black().opacity(0.26))
        .min_w_0()
        .text_size(px(12.5))
        .line_height(px(19.0));
    if lines.is_empty() {
        return well
            .text_color(color(FAINT_TEXT))
            .child("nothing to show")
            .into_any_element();
    }
    let mut previous = None;
    for run in lines.chunk_by(|a, b| a.0 == b.0) {
        let tint = run[0].0;
        let text = run
            .iter()
            .map(|(_, line)| line.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let tone = match tint {
            Tint::Plain | Tint::Output => color(MUTED_TEXT),
            Tint::Command => color(TEXT),
            Tint::Added => color(SAGE),
            Tint::Removed | Tint::Note => color(FAINT_TEXT),
            Tint::Error => color(SIGNAL),
        };
        let wash = match tint {
            Tint::Added => Some(color(SAGE).opacity(0.08)),
            Tint::Removed => Some(color(SIGNAL).opacity(0.06)),
            _ => None,
        };
        let steps_down =
            tint == Tint::Output && previous.is_some_and(|before| before != Tint::Output);
        well = well.child(
            div()
                .mx(px(-12.0))
                .px(px(12.0))
                .min_w_0()
                .when(steps_down, |run| run.mt(px(8.0)))
                .when_some(wash, gpui::Styled::bg)
                .text_color(tone)
                .child(text),
        );
        previous = Some(tint);
    }
    well.into_any_element()
}

/// The pinned card for a permission the agent is waiting on. The slab opens
/// onto the ground there, and the card itself is warm.
#[allow(clippy::too_many_lines)]
fn needs_you_card(
    view: &PaneView<'_>,
    agent: &AgentSessionSnapshot,
    permission: &AgentPermission,
) -> AnyElement {
    let grid = view.grid;
    let tool = agent.timeline.iter().find_map(|item| match item {
        AgentTimelineItem::Tool(tool) if tool.id == permission.tool_call_id => Some(tool),
        _ => None,
    });
    let title = agent_short_name(agent);
    let question = permission_question(&title, permission, tool, &agent.cwd);
    let preview = tool
        .map(|tool| permission_preview(tool, &agent.cwd))
        .unwrap_or_default();
    let highlighted = view.choice.unwrap_or_else(|| default_choice(permission));
    let mut options = v_flex()
        .mt(px(10.0))
        .ml(px(grid.column - 24.0))
        .items_start()
        .gap(px(2.0));
    for (index, option) in permission
        .options
        .iter()
        .take(OPTION_KEYS.len())
        .enumerate()
    {
        let app = view.app.clone();
        let session_id = agent.id;
        let request_id = permission.request_id.clone();
        let option_id = option.id.clone();
        options = options.child(
            choice_row(
                SharedString::from(format!("agent-permission-{}", option.id)),
                OPTION_KEYS[index],
                index == highlighted,
                color(SIGNAL),
            )
            .child(div().min_w_0().truncate().child(label_lower(&option.label)))
            .on_click(move |_, _, cx| {
                let _ = app.update(cx, |this, cx| {
                    this.backend.send(CommandMessage::ResolveAgentPermission {
                        session_id,
                        request_id: request_id.clone(),
                        option_id: Some(option_id.clone()),
                    });
                    cx.notify();
                });
            }),
        );
    }
    let enter_label = permission
        .options
        .get(highlighted)
        .map_or_else(|| "answer".to_owned(), |option| label_lower(&option.label));
    let mut inner = v_flex()
        .w_full()
        .min_w_0()
        .px(px(SIDE))
        .pt(px(12.0))
        .pb(px(14.0))
        .border_t_1()
        .border_b_1()
        .border_color(hairline(0.07))
        .child(
            h_flex()
                .gap(px(grid.gap()))
                .text_color(color(SIGNAL))
                .child(
                    h_flex()
                        .flex_none()
                        .w(px(grid.gutter))
                        .h(px(LINE))
                        .child(pulse_dot(view.pulse, 8.0)),
                )
                .child(div().font_weight(FontWeight::BOLD).child("needs you"))
                .child(div().flex_1())
                .child(
                    div()
                        .flex_none()
                        .text_color(color(FAINT_TEXT))
                        .child(permission.asked_at.map(clock).unwrap_or_default()),
                ),
        )
        .child(
            div()
                .ml(px(grid.column))
                .mt(px(4.0))
                .min_w_0()
                .text_size(px(18.0))
                .line_height(px(28.0))
                .font_weight(FontWeight::MEDIUM)
                .child(question),
        );
    if !preview.is_empty() {
        inner = inner.child(detail_well(grid, &preview));
    }
    inner = inner.child(options).child(
        h_flex()
            .ml(px(grid.column))
            .mt(px(12.0))
            .gap_x(px(16.0))
            .gap_y(px(2.0))
            .flex_wrap()
            .text_color(color(FAINT_TEXT))
            .child(key_hint("↑↓", "choose"))
            .child(
                h_flex()
                    .flex_none()
                    .gap(px(6.0))
                    .child(kbd("enter", color(MUTED_TEXT)))
                    .child(enter_label),
            )
            .child(key_hint("esc", "interrupt")),
    );
    div()
        .w_full()
        .flex_none()
        .py(px(10.0))
        .child(
            div()
                .relative()
                .w_full()
                .bg(color(SURFACE))
                .child(
                    img(chrome::GLOW)
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .object_fit(ObjectFit::Fill),
                )
                .child(inner),
        )
        .into_any_element()
}

/// The option a permission card highlights first: allowing once, never a
/// standing rule, so a reflexive enter is the cautious choice.
pub(super) fn default_choice(permission: &AgentPermission) -> usize {
    permission
        .options
        .iter()
        .take(OPTION_KEYS.len())
        .position(|option| option.kind == PermissionKind::AllowOnce)
        .unwrap_or_default()
}

/// The pinned card for an agent that needs a sign-in before it can start.
fn sign_in_card(view: &PaneView<'_>, agent: &AgentSessionSnapshot) -> AnyElement {
    let grid = view.grid;
    let title = agent_title(agent);
    let mut options = v_flex()
        .mt(px(10.0))
        .ml(px(grid.column - 24.0))
        .items_start()
        .gap(px(2.0));
    for (index, method) in agent
        .auth_methods
        .iter()
        .take(OPTION_KEYS.len())
        .enumerate()
    {
        let app = view.app.clone();
        let session_id = agent.id;
        let method_id = method.id.clone();
        options = options.child(
            choice_row(
                SharedString::from(format!("agent-auth-{}", method.id)),
                OPTION_KEYS[index],
                index == 0,
                color(SIGNAL),
            )
            .child(div().flex_none().child(label_lower(&method.name)))
            .when_some(method.description.clone(), |row, description| {
                row.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(color(FAINT_TEXT))
                        .child(sentence_lower(&description)),
                )
            })
            .on_click(move |_, _, cx| {
                let _ = app.update(cx, |this, cx| {
                    this.backend.send(CommandMessage::AuthenticateAgent {
                        session_id,
                        method_id: method_id.clone(),
                    });
                    cx.notify();
                });
            }),
        );
    }
    let inner = v_flex()
        .w_full()
        .px(px(SIDE))
        .pt(px(12.0))
        .pb(px(14.0))
        .border_t_1()
        .border_b_1()
        .border_color(hairline(0.07))
        .bg(color(SURFACE))
        .child(
            h_flex()
                .gap(px(grid.gap()))
                .text_color(color(SIGNAL))
                .child(
                    h_flex()
                        .flex_none()
                        .w(px(grid.gutter))
                        .h(px(LINE))
                        .child(pulse_dot(view.pulse, 8.0)),
                )
                .child(div().font_weight(FontWeight::BOLD).child("needs you")),
        )
        .child(
            div()
                .ml(px(grid.column))
                .mt(px(4.0))
                .text_size(px(17.0))
                .line_height(px(26.0))
                .font_weight(FontWeight::MEDIUM)
                .child(format!("sign in to {title}")),
        )
        .child(options);
    div()
        .w_full()
        .flex_none()
        .py(px(8.0))
        .child(inner)
        .into_any_element()
}

/// One of gofer's keyed choices: a peach `›` on the default or hovered row, a
/// bracketed key, then whatever the caller adds.
fn choice_row(
    id: SharedString,
    key: &'static str,
    default: bool,
    key_color: Hsla,
) -> gpui::Stateful<gpui::Div> {
    let group = SharedString::from(format!("{id}-group"));
    h_flex()
        .id(id)
        .group(group.clone())
        .relative()
        .min_w(px(288.0))
        .max_w_full()
        .h(px(24.0))
        .pl(px(24.0))
        .pr(px(12.0))
        .gap(px(8.0))
        .rounded(px(6.0))
        .cursor_pointer()
        .hover(|row| row.bg(color(SIGNAL).opacity(0.12)))
        .active(|row| row.bg(color(SIGNAL)).text_color(color(GROUND)))
        .child(
            div()
                .absolute()
                .left(px(8.0))
                .text_color(color(SIGNAL))
                .when(!default, |mark| {
                    mark.invisible()
                        .group_hover(group.clone(), gpui::Styled::visible)
                })
                .child("›"),
        )
        .child(kbd(key, key_color))
}

/// The line over the composer while the agent works: how long it has been
/// at it, in dot-matrix, and what it is doing.
#[allow(clippy::too_many_lines)]
fn working_line(view: &PaneView<'_>, agent: &AgentSessionSnapshot) -> Option<AnyElement> {
    let grid = view.grid;
    let busy = matches!(
        agent.status,
        AgentSessionStatus::Working | AgentSessionStatus::WaitingForPermission
    );
    if !busy
        && agent.status != AgentSessionStatus::Starting
        && agent.status != AgentSessionStatus::Authenticating
    {
        return None;
    }
    let started = agent
        .timeline
        .iter()
        .rev()
        .find_map(|item| match item {
            AgentTimelineItem::Message {
                role: AgentMessageRole::User,
                at,
                ..
            } => *at,
            _ => None,
        })
        .unwrap_or(agent.started_at);
    let elapsed = elapsed_label(view.now.saturating_sub(started));
    let activity = match agent.status {
        AgentSessionStatus::Starting => format!("starting {}", agent_title(agent)),
        AgentSessionStatus::Authenticating => "signing in · finish in your browser".to_owned(),
        AgentSessionStatus::WaitingForPermission => "waiting for you".to_owned(),
        _ => current_activity(agent),
    };
    // Only this turn's plan counts; an earlier turn's is finished business.
    let plan = agent
        .timeline
        .iter()
        .rev()
        .take_while(|item| {
            !matches!(
                item,
                AgentTimelineItem::Message {
                    role: AgentMessageRole::User,
                    ..
                }
            )
        })
        .find_map(|item| match item {
            AgentTimelineItem::Plan(entries) => Some(entries),
            _ => None,
        });
    let progress = plan.filter(|entries| !entries.is_empty()).map(|entries| {
        let done = entries
            .iter()
            .filter(|entry| entry.status == PlanStatus::Completed)
            .count();
        format!(
            "step {} of {}",
            (done + 1).min(entries.len()),
            entries.len()
        )
    });
    let stopwatch = chrome::dot_matrix(&elapsed, 2.0, color(TEXT))
        .unwrap_or_else(|| div().child(elapsed.clone()).into_any_element());
    Some(
        h_flex()
            .w_full()
            .flex_none()
            .h(px(30.0))
            .px(px(SIDE))
            .gap(px(grid.gap()))
            .border_t_1()
            .border_color(hairline(0.07))
            .bg(color(SURFACE))
            .child(h_flex().flex_none().w(px(grid.gutter)).child(stopwatch))
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(8.0))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(color(MUTED_TEXT))
                            .child(activity),
                    )
                    .child(pulse_dot(view.pulse, 6.0))
                    .when_some(progress, |line, progress| {
                        line.child(
                            div()
                                .flex_none()
                                .text_color(color(FAINT_TEXT))
                                .child(progress),
                        )
                    }),
            )
            .when(busy, |line| {
                line.child(
                    h_flex()
                        .flex_none()
                        .gap(px(8.0))
                        .text_color(color(FAINT_TEXT))
                        .child(kbd("esc", color(TEXT)))
                        .child("interrupt"),
                )
            })
            .into_any_element(),
    )
}

/// The composer's placeholder, in gofer's words: who a message goes to.
pub(super) fn composer_placeholder(
    agent: Option<&AgentSessionSnapshot>,
    launcher: Option<&AgentProfile>,
) -> String {
    match agent {
        Some(agent)
            if matches!(
                agent.status,
                AgentSessionStatus::Working | AgentSessionStatus::WaitingForPermission
            ) =>
        {
            format!("queue a message for {}", agent_short_name(agent))
        }
        Some(agent) => format!("message {}", agent_short_name(agent)),
        None => launcher.map_or_else(
            || "message an agent".to_owned(),
            |profile| format!("message {}", profile_title(profile)),
        ),
    }
}

/// What the agent is doing right now, in a few words.
fn current_activity(agent: &AgentSessionSnapshot) -> String {
    for item in agent.timeline.iter().rev() {
        match item {
            AgentTimelineItem::Tool(tool)
                if matches!(tool.status, ToolStatus::Running | ToolStatus::Pending) =>
            {
                return format!("{} {}", tool_verb(tool), tool_preview(tool, &agent.cwd));
            }
            AgentTimelineItem::Message {
                role: AgentMessageRole::Thought,
                ..
            } => return "thinking".to_owned(),
            AgentTimelineItem::Message {
                role: AgentMessageRole::Agent,
                ..
            } => return "writing".to_owned(),
            AgentTimelineItem::Message {
                role: AgentMessageRole::User,
                ..
            } => break,
            _ => {}
        }
    }
    format!("waiting for {}", agent_short_name(agent))
}

/// One piece of a hint line: a key in brackets and what it does, or words.
enum Hint {
    Key(&'static str, String),
    Words(String),
    /// The composer answering a command: what happened, in ink when it went
    /// wrong, then what to try, quieter.
    Note {
        lead: String,
        rest: Option<String>,
        problem: bool,
    },
}

/// What the composer says back to one of mux's slash commands, on the line
/// its key hints usually take, until the next keystroke. It stands in for a
/// toast: the answer appears where the command was typed.
pub(super) struct ComposerNote {
    pub tab_id: TabId,
    pub lead: String,
    pub rest: Option<String>,
    /// A command that went wrong stays in the composer to be fixed.
    pub problem: bool,
}

fn hint_key(key: &'static str, label: impl Into<String>) -> Hint {
    Hint::Key(key, label.into())
}

/// A line of hints, keys in brackets, as gofer writes them.
fn hint_line(hints: Vec<Hint>) -> gpui::Div {
    let mut line = h_flex()
        .min_w_0()
        .gap(px(16.0))
        .overflow_hidden()
        .text_color(color(FAINT_TEXT));
    for hint in hints {
        line = line.child(match hint {
            Hint::Key(key, label) => h_flex()
                .flex_none()
                .gap(px(6.0))
                .child(kbd(key, color(MUTED_TEXT)))
                .child(label),
            Hint::Words(words) => div().min_w_0().truncate().child(words),
            Hint::Note {
                lead,
                rest,
                problem,
            } => h_flex()
                .min_w_0()
                .gap(px(12.0))
                .child(
                    div()
                        .flex_none()
                        .text_color(color(if problem { SIGNAL } else { MUTED_TEXT }))
                        .child(lead),
                )
                .children(rest.map(|rest| div().min_w_0().truncate().child(rest))),
        });
    }
    line
}

/// What the line under the composer says: the keys that matter right now.
fn composer_hint(
    agent: Option<&AgentSessionSnapshot>,
    busy: bool,
    draft: &str,
    menu_open: bool,
    note: Option<&ComposerNote>,
) -> Vec<Hint> {
    if menu_open {
        return vec![
            hint_key("↑↓", "choose"),
            hint_key("tab", "complete"),
            hint_key("esc", "close"),
        ];
    }
    if let Some(note) = note {
        return vec![Hint::Note {
            lead: note.lead.clone(),
            rest: note.rest.clone(),
            problem: note.problem,
        }];
    }
    if let Some(agent) = agent {
        if !agent.queued.is_empty() {
            let count = agent.queued.len();
            return vec![
                Hint::Words(format!("{count} queued, runs when this turn ends")),
                hint_key("esc", if count == 1 { "drop it" } else { "drop them" }),
            ];
        }
        if busy && draft.trim().is_empty() {
            return vec![Hint::Words(
                "type ahead: it runs when this turn ends".to_owned(),
            )];
        }
        if agent.status == AgentSessionStatus::Failed {
            return vec![
                hint_key("enter", "try again"),
                Hint::Words("/new starts a fresh session".to_owned()),
            ];
        }
        if agent.status == AgentSessionStatus::Closed {
            return vec![
                Hint::Words("this session ended".to_owned()),
                hint_key("enter", "start a new one"),
            ];
        }
    }
    if draft.starts_with('/') {
        return vec![hint_key("tab", "complete"), hint_key("enter", "run it")];
    }
    let mut hints = vec![hint_key("@", "file"), hint_key("/", "command")];
    if agent.is_some() {
        hints.push(hint_key("⇧tab", "mode"));
    }
    hints.push(hint_key("⇧enter", "new line"));
    hints
}

/// Completions open as a sheet over the composer, on the thread's own grid:
/// the chosen row marked in the gutter's edge, the name on the text column,
/// what it does after it. Files lead with their name, then where they live.
fn completion_sheet(view: &PaneView<'_>, menu: &AgentCompletionMenu) -> AnyElement {
    let grid = view.grid;
    let agent_name = view.agent.map(agent_short_name);
    let mut rows = v_flex()
        .w_full()
        .min_w_0()
        .pl(px(SIDE + grid.column - 24.0))
        .pr(px(SIDE - 8.0))
        .py(px(6.0))
        .gap(px(2.0));
    for (index, completion) in menu.items.iter().enumerate().take(8) {
        let selected = index == menu.selected;
        let app = view.app.clone();
        let (name, rest) = match completion.kind {
            AgentCompletionKind::File => {
                let path = Path::new(&completion.label);
                let name = path.file_name().map_or_else(
                    || completion.label.clone(),
                    |name| name.to_string_lossy().into_owned(),
                );
                let parent = path
                    .parent()
                    .map(|parent| parent.display().to_string())
                    .filter(|parent| !parent.is_empty())
                    .unwrap_or_default();
                (name, parent)
            }
            AgentCompletionKind::Command | AgentCompletionKind::Value => (
                completion.label.clone(),
                sentence_lower(&first_line(&completion.description)),
            ),
        };
        // Only the agent's own commands say whose they are.
        let source = (completion.kind == AgentCompletionKind::Command
            && completion.detail == "ACP")
            .then(|| agent_name.clone())
            .flatten();
        rows = rows.child(
            h_flex()
                .id(SharedString::from(format!("agent-completion-{index}")))
                .relative()
                .w_full()
                .min_w_0()
                .h(px(24.0))
                .pl(px(24.0))
                .pr(px(8.0))
                .gap(px(12.0))
                .rounded(px(6.0))
                .cursor_pointer()
                .when(selected, |row| row.bg(wash(0.1)))
                .when(!selected, |row| row.hover(|row| row.bg(wash(0.05))))
                .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                    cx.stop_propagation();
                    let _ = app.update(cx, |this, cx| {
                        this.accept_agent_completion(Some(index), window, cx);
                    });
                })
                .child(
                    div()
                        .absolute()
                        .left(px(8.0))
                        .text_color(color(SIGNAL))
                        .when(!selected, gpui::Styled::invisible)
                        .child("›"),
                )
                .child(
                    div()
                        .flex_none()
                        .when(selected, |name| name.font_weight(FontWeight::BOLD))
                        .text_color(color(TEXT))
                        .child(name),
                )
                .when(completion.current, |row| {
                    row.child(div().flex_none().text_color(color(SAGE)).child("current"))
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(color(FAINT_TEXT))
                        .child(rest),
                )
                .children(source.map(|source| {
                    div()
                        .flex_none()
                        .text_color(color(FAINT_TEXT))
                        .child(source)
                })),
        );
    }
    div()
        .id("agent-completion-menu")
        .w_full()
        .flex_none()
        .border_t_1()
        .border_color(hairline(0.12))
        .bg(color(SURFACE))
        .child(rows)
        .into_any_element()
}

// ---------------------------------------------------------------- markdown

/// An agent's reply as gofer sets it: mono, 14px on a 22px line, code on a
/// wash, blocks in wells, links underlined in a hairline. Paragraphs part by
/// 10px, list items by 2px, and headings sit closer to what they head.
fn markdown(source: &str, cursor: Option<bool>) -> AnyElement {
    let doc = bezel_markdown::parse(source);
    let mut column = v_flex().w_full().min_w_0();
    let count = doc.blocks.len();
    if count == 0 {
        return div()
            .min_w_0()
            .text_size(px(PROSE_SIZE))
            .line_height(px(PROSE_LINE))
            .child(StyledText::new(CURSOR).with_runs(vec![cursor_run(
                cursor.unwrap_or(false),
                FontWeight::NORMAL,
            )]))
            .into_any_element();
    }
    let mut previous: Option<&BlockKind> = None;
    for (index, block) in doc.blocks.iter().enumerate() {
        let cursor = cursor.filter(|_| index + 1 == count);
        let indent = f32::from(block.indent) * 20.0;
        let element: AnyElement = match &block.kind {
            BlockKind::Paragraph(text) => {
                prose(text, FontWeight::NORMAL, color(TEXT), cursor, index)
            }
            BlockKind::Heading { text, .. } => {
                prose(text, FontWeight::BOLD, color(TEXT), cursor, index)
            }
            BlockKind::Bullet(text) => hanging(
                indent,
                "•",
                prose(text, FontWeight::NORMAL, color(TEXT), cursor, index),
            ),
            BlockKind::Ordered { number, text } => hanging(
                indent,
                &format!("{number}."),
                prose(text, FontWeight::NORMAL, color(TEXT), cursor, index),
            ),
            BlockKind::Task { checked, text } => hanging(
                indent,
                if *checked { "[x]" } else { "[ ]" },
                prose(
                    text,
                    FontWeight::NORMAL,
                    color(if *checked { FAINT_TEXT } else { TEXT }),
                    cursor,
                    index,
                ),
            ),
            BlockKind::Quote(text) => div()
                .pl(px(12.0))
                .border_l_1()
                .border_color(hairline(0.2))
                .child(prose(
                    text,
                    FontWeight::NORMAL,
                    color(MUTED_TEXT),
                    cursor,
                    index,
                ))
                .into_any_element(),
            BlockKind::Code { code, .. } => div()
                .px(px(12.0))
                .py(px(8.0))
                .rounded(px(6.0))
                .bg(gpui::black().opacity(0.26))
                .text_size(px(12.5))
                .line_height(px(19.0))
                .text_color(color(MUTED_TEXT))
                .child(code.trim_end_matches('\n').to_owned())
                .into_any_element(),
            BlockKind::Table { header, rows, .. } => table(header, rows),
            BlockKind::Rule => div()
                .h(px(1.0))
                .my(px(4.0))
                .bg(hairline(0.12))
                .into_any_element(),
            BlockKind::Image { alt, .. } => div()
                .text_color(color(FAINT_TEXT))
                .child(format!(
                    "[image{}]",
                    if alt.is_empty() {
                        String::new()
                    } else {
                        format!(": {alt}")
                    }
                ))
                .into_any_element(),
        };
        let space = match (previous, &block.kind) {
            (None, _) => 0.0,
            (Some(before), kind) if is_list_item(before) && is_list_item(kind) => 2.0,
            (Some(BlockKind::Heading { .. }), _) => 4.0,
            (Some(_), BlockKind::Heading { .. }) => 14.0,
            _ => 10.0,
        };
        column = column.child(div().mt(px(space)).w_full().min_w_0().child(element));
        previous = Some(&block.kind);
    }
    column.into_any_element()
}

fn is_list_item(kind: &BlockKind) -> bool {
    matches!(
        kind,
        BlockKind::Bullet(_) | BlockKind::Ordered { .. } | BlockKind::Task { .. }
    )
}

fn hanging(indent: f32, mark: &str, body: AnyElement) -> AnyElement {
    h_flex()
        .items_start()
        .min_w_0()
        .pl(px(indent))
        .child(
            div()
                .flex_none()
                .w(px(if mark.len() > 2 { 32.0 } else { 20.0 }))
                .text_size(px(PROSE_SIZE))
                .line_height(px(PROSE_LINE))
                .text_color(color(FAINT_TEXT))
                .child(mark.to_owned()),
        )
        .child(div().flex_1().min_w_0().child(body))
        .into_any_element()
}

/// gofer's streaming cursor: a cell of solid text colour after the last
/// word, blinking while the reply is still arriving.
const CURSOR: &str = "█";

fn cursor_run(lit: bool, weight: FontWeight) -> TextRun {
    let mut face = font(EMBEDDED_TERMINAL_FONT);
    face.weight = weight;
    TextRun {
        len: CURSOR.len(),
        font: face,
        color: if lit {
            color(TEXT)
        } else {
            gpui::transparent_black()
        },
        background_color: None,
        underline: None,
        strikethrough: None,
    }
}

/// Inline text with marks, as one run list in the mono face.
fn prose(
    text: &MarkdownText,
    weight: FontWeight,
    base: Hsla,
    cursor: Option<bool>,
    index: usize,
) -> AnyElement {
    let mut cuts: Vec<usize> = text
        .marks
        .iter()
        .flat_map(|span| [span.range.start, span.range.end])
        .chain([0, text.text.len()])
        .filter(|cut| *cut <= text.text.len())
        .collect();
    cuts.sort_unstable();
    cuts.dedup();
    let mut runs = Vec::new();
    let mut links: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    for pair in cuts.windows(2) {
        let (start, end) = (pair[0], pair[1]);
        if start == end {
            continue;
        }
        let (mut bold, mut italic, mut code, mut strike) = (false, false, false, false);
        let mut link = None;
        for span in text
            .marks
            .iter()
            .filter(|span| span.range.start <= start && span.range.end >= end)
        {
            match &span.mark {
                Mark::Bold => bold = true,
                Mark::Italic => italic = true,
                Mark::Strike => strike = true,
                Mark::Code => code = true,
                Mark::Link(url) | Mark::Image(url) => link = Some(url.clone()),
            }
        }
        if let Some(url) = &link {
            match links.last_mut() {
                Some((range, last)) if range.end == start && last == url => range.end = end,
                _ => links.push((start..end, url.clone())),
            }
        }
        let mut face = font(EMBEDDED_TERMINAL_FONT);
        face.weight = if bold { FontWeight::BOLD } else { weight };
        face.style = if italic {
            FontStyle::Italic
        } else {
            FontStyle::Normal
        };
        runs.push(TextRun {
            len: end - start,
            font: face,
            color: if strike { color(FAINT_TEXT) } else { base },
            background_color: code.then(|| wash(0.06)),
            underline: link.is_some().then_some(UnderlineStyle {
                color: Some(hairline(0.3)),
                thickness: px(1.0),
                wavy: false,
            }),
            strikethrough: strike.then_some(StrikethroughStyle {
                thickness: px(1.0),
                color: Some(color(FAINT_TEXT)),
            }),
        });
    }
    let mut content = text.text.clone();
    if let Some(lit) = cursor {
        content.push_str(CURSOR);
        runs.push(cursor_run(lit, weight));
    }
    let styled = StyledText::new(content).with_runs(runs);
    let painted: AnyElement = if links.is_empty() {
        styled.into_any_element()
    } else {
        let (ranges, urls): (Vec<_>, Vec<_>) = links.into_iter().unzip();
        InteractiveText::new(gpui::ElementId::named_usize("agent-md", index), styled)
            .on_click(ranges, move |clicked, _window, cx| {
                if let Some(url) = urls.get(clicked) {
                    cx.open_url(url);
                }
            })
            .into_any_element()
    };
    div()
        .min_w_0()
        .text_size(px(PROSE_SIZE))
        .line_height(px(PROSE_LINE))
        .child(painted)
        .into_any_element()
}

/// A table set in the mono face: columns padded to their widest cell.
fn table(header: &[MarkdownText], rows: &[Vec<MarkdownText>]) -> AnyElement {
    let columns = header
        .len()
        .max(rows.iter().map(Vec::len).max().unwrap_or(0));
    let mut widths = vec![0usize; columns];
    for row in std::iter::once(header).chain(rows.iter().map(Vec::as_slice)) {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(cell.text.chars().count().min(40));
        }
    }
    let line = |row: &[MarkdownText]| {
        (0..columns)
            .map(|index| {
                let cell = row.get(index).map_or("", |cell| cell.text.as_str());
                let cell: String = cell.chars().take(40).collect();
                format!("{cell:<width$}", width = widths[index])
            })
            .collect::<Vec<_>>()
            .join("  ")
            .trim_end()
            .to_owned()
    };
    let mut table = v_flex()
        .min_w_0()
        .text_size(px(13.0))
        .line_height(px(LINE))
        .child(
            div()
                .font_weight(FontWeight::BOLD)
                .pb(px(2.0))
                .border_b_1()
                .border_color(hairline(0.07))
                .whitespace_nowrap()
                .overflow_hidden()
                .child(line(header)),
        );
    for row in rows {
        table = table.child(
            div()
                .py(px(1.0))
                .border_b_1()
                .border_color(hairline(0.07))
                .whitespace_nowrap()
                .overflow_hidden()
                .text_color(color(MUTED_TEXT))
                .child(line(row)),
        );
    }
    table.into_any_element()
}

// ------------------------------------------------------------ tools & words

/// The short name of a tool's kind, as a step row's second column.
fn tool_label(tool: &AgentTool) -> &'static str {
    match tool.kind {
        AgentToolKind::Read => "read",
        AgentToolKind::Edit => {
            if tool.diffs.iter().any(|diff| diff.old_text.is_none()) {
                "write"
            } else {
                "edit"
            }
        }
        AgentToolKind::Delete => "delete",
        AgentToolKind::Move => "move",
        AgentToolKind::Search => "search",
        AgentToolKind::Execute => "shell",
        AgentToolKind::Think => "think",
        AgentToolKind::Fetch => "web",
        AgentToolKind::SwitchMode => "mode",
        AgentToolKind::Other => "tool",
    }
}

fn tool_verb(tool: &AgentTool) -> &'static str {
    match tool.kind {
        AgentToolKind::Read => "reading",
        AgentToolKind::Edit => "editing",
        AgentToolKind::Delete => "deleting",
        AgentToolKind::Move => "moving",
        AgentToolKind::Search => "searching",
        AgentToolKind::Execute => "running",
        AgentToolKind::Think => "thinking about",
        AgentToolKind::Fetch => "fetching",
        AgentToolKind::SwitchMode => "switching to",
        AgentToolKind::Other => "using",
    }
}

fn raw_string<'a>(tool: &'a AgentTool, keys: &[&str]) -> Option<&'a str> {
    let input = tool.raw_input.as_ref()?;
    keys.iter()
        .find_map(|key| input.get(key).and_then(serde_json::Value::as_str))
        .filter(|value| !value.trim().is_empty())
}

/// The command a shell step ran, from its input when the agent sent one.
fn tool_command(tool: &AgentTool) -> Option<String> {
    let input = tool.raw_input.as_ref()?;
    let command = input.get("command").or_else(|| input.get("cmd"))?;
    match command {
        serde_json::Value::String(command) => Some(command.trim().to_owned()),
        serde_json::Value::Array(parts) => {
            let words = parts
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect::<Vec<_>>();
            // ["bash", "-lc", "cargo test"] says only "cargo test".
            match words.as_slice() {
                [shell, flag, script] if shell.ends_with("sh") && flag.starts_with('-') => {
                    Some((*script).to_owned())
                }
                _ => Some(words.join(" ")),
            }
        }
        _ => None,
    }
}

fn tool_path(tool: &AgentTool) -> Option<&Path> {
    tool.locations
        .first()
        .map(|location| location.path.as_path())
        .or_else(|| tool.diffs.first().map(|diff| diff.path.as_path()))
        .or_else(|| {
            raw_string(tool, &["file_path", "path", "abs_path", "notebook_path"]).map(Path::new)
        })
}

/// What a step was about, in one line: the command, the file, the query.
fn tool_preview(tool: &AgentTool, cwd: &Path) -> String {
    let title = plain_line(&tool.title);
    let preview = match tool.kind {
        AgentToolKind::Execute => tool_command(tool).map(|command| plain_line(&command)),
        AgentToolKind::Read | AgentToolKind::Edit | AgentToolKind::Delete | AgentToolKind::Move => {
            tool_path(tool).map(|path| {
                let mut shown = relative_path(path, cwd);
                if tool.kind == AgentToolKind::Read
                    && let Some(line) = tool.locations.first().and_then(|location| location.line)
                {
                    shown = format!("{shown}:{line}");
                }
                shown
            })
        }
        AgentToolKind::Search => raw_string(tool, &["pattern", "query", "regex"])
            .map(|pattern| format!("\"{}\"", plain_line(pattern))),
        AgentToolKind::Fetch => raw_string(tool, &["url", "query"]).map(plain_line),
        AgentToolKind::Think | AgentToolKind::SwitchMode | AgentToolKind::Other => None,
    };
    let preview = preview
        .filter(|preview| !preview.is_empty())
        .unwrap_or(title);
    if preview.is_empty() {
        "…".to_owned()
    } else {
        preview
    }
}

fn tool_duration(tool: &AgentTool) -> Option<u64> {
    tool.started_at
        .zip(tool.finished_at)
        .map(|(start, end)| end.saturating_sub(start))
}

/// `+3 −1` beside an edit: what it adds forward, what it takes away back.
fn diff_badge(tool: &AgentTool) -> Option<AnyElement> {
    if tool.diffs.is_empty() {
        return None;
    }
    let (added, removed) = tool
        .diffs
        .iter()
        .map(diff_stats)
        .fold((0, 0), |(added, removed), (a, r)| (added + a, removed + r));
    Some(
        h_flex()
            .flex_none()
            .gap(px(6.0))
            .child(div().text_color(color(SAGE)).child(format!("+{added}")))
            .child(
                div()
                    .text_color(color(FAINT_TEXT))
                    .child(format!("−{removed}")),
            )
            .into_any_element(),
    )
}

/// What a step's well shows: the command, the change, the output.
fn tool_detail(tool: &AgentTool, cwd: &Path) -> Vec<(Tint, String)> {
    let mut lines = Vec::new();
    if let Some(command) = tool_command(tool) {
        for (index, line) in plain(&command).lines().enumerate() {
            lines.push((
                Tint::Command,
                if index == 0 {
                    format!("$ {line}")
                } else {
                    format!("  {line}")
                },
            ));
        }
    } else if let Some(query) = raw_string(tool, &["pattern", "query", "url"]) {
        lines.push((Tint::Command, format!("> {}", plain_line(query))));
    }
    for diff in &tool.diffs {
        lines.push((
            Tint::Command,
            format!("@ {}", relative_path(&diff.path, cwd)),
        ));
        lines.extend(diff_lines(diff, 40));
    }
    let output = tool
        .detail
        .clone()
        .filter(|detail| !detail.trim().is_empty())
        .or_else(|| raw_output_text(tool));
    if let Some(output) = output {
        let output = plain(&output);
        let all: Vec<&str> = output.trim_end().lines().collect();
        let skipped = all.len().saturating_sub(DETAIL_LINES);
        if skipped > 0 {
            lines.push((Tint::Note, format!("# {skipped} earlier lines")));
        }
        for line in &all[skipped..] {
            lines.push((
                if tool.status == ToolStatus::Failed {
                    Tint::Error
                } else {
                    Tint::Output
                },
                (*line).to_owned(),
            ));
        }
    }
    if lines.is_empty()
        && let Some(input) = tool.raw_input.as_ref()
    {
        let pretty = serde_json::to_string_pretty(input).unwrap_or_default();
        lines.extend(
            pretty
                .lines()
                .take(DETAIL_LINES)
                .map(|line| (Tint::Plain, line.to_owned())),
        );
    }
    lines
}

fn raw_output_text(tool: &AgentTool) -> Option<String> {
    let output = tool.raw_output.as_ref()?;
    for key in [
        "formatted_output",
        "aggregated_output",
        "output",
        "stdout",
        "content",
    ] {
        if let Some(text) = output.get(key).and_then(serde_json::Value::as_str)
            && !text.trim().is_empty()
        {
            return Some(text.to_owned());
        }
    }
    output.as_str().map(str::to_owned)
}

/// The preview a permission card shows under its question.
fn permission_preview(tool: &AgentTool, cwd: &Path) -> Vec<(Tint, String)> {
    let mut lines = Vec::new();
    if let Some(command) = tool_command(tool) {
        for (index, line) in plain(&command).lines().take(8).enumerate() {
            lines.push((
                Tint::Command,
                if index == 0 {
                    format!("$ {line}")
                } else {
                    format!("  {line}")
                },
            ));
        }
    }
    for diff in tool.diffs.iter().take(2) {
        lines.push((Tint::Note, format!("# {}", relative_path(&diff.path, cwd))));
        lines.extend(diff_lines(diff, 12));
    }
    if lines.is_empty()
        && let Some(url) = raw_string(tool, &["url"])
    {
        lines.push((Tint::Command, format!("> {url}")));
    }
    lines
}

fn permission_question(
    agent: &str,
    permission: &AgentPermission,
    tool: Option<&AgentTool>,
    cwd: &Path,
) -> String {
    let Some(tool) = tool else {
        return format!("allow {agent} to {}?", sentence_lower(&permission.title));
    };
    let path = tool_path(tool).map(|path| relative_path(path, cwd));
    match (tool.kind, path) {
        (AgentToolKind::Execute, _) => format!("allow {agent} to run this?"),
        (AgentToolKind::Edit, Some(path)) => format!("allow {agent} to edit {path}?"),
        (AgentToolKind::Delete, Some(path)) => format!("allow {agent} to delete {path}?"),
        (AgentToolKind::Move, Some(path)) => format!("allow {agent} to move {path}?"),
        (AgentToolKind::Read, Some(path)) => format!("allow {agent} to read {path}?"),
        (AgentToolKind::Fetch, _) => format!("allow {agent} to fetch this?"),
        _ => format!(
            "allow {agent} to use {}?",
            sentence_lower(&plain_line(&tool.title))
        ),
    }
}

fn permission_subject(permission: &AgentPermission) -> String {
    sentence_lower(&plain_line(&permission.title))
}

/// Lines added and removed by a diff.
fn diff_stats(diff: &AgentDiff) -> (usize, usize) {
    let lines = diff_lines(diff, usize::MAX);
    let added = lines
        .iter()
        .filter(|(tint, _)| *tint == Tint::Added)
        .count();
    let removed = lines
        .iter()
        .filter(|(tint, _)| *tint == Tint::Removed)
        .count();
    (added, removed)
}

/// A diff as tinted lines: `+ ` additions, `- ` removals, and a little
/// context, trimmed to `limit` lines.
fn diff_lines(diff: &AgentDiff, limit: usize) -> Vec<(Tint, String)> {
    let old: Vec<&str> = diff
        .old_text
        .as_deref()
        .map_or_else(Vec::new, |text| text.lines().collect());
    let new: Vec<&str> = diff.new_text.lines().collect();
    let prefix = old
        .iter()
        .zip(&new)
        .take_while(|(old, new)| old == new)
        .count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(old, new)| old == new)
        .count();
    let old_middle = &old[prefix..old.len() - suffix];
    let new_middle = &new[prefix..new.len() - suffix];
    let mut lines = Vec::new();
    for line in &old[prefix.saturating_sub(2)..prefix] {
        lines.push((Tint::Plain, format!("  {line}")));
    }
    lines.extend(middle_diff(old_middle, new_middle));
    for line in old[old.len() - suffix..].iter().take(2) {
        lines.push((Tint::Plain, format!("  {line}")));
    }
    if lines.len() > limit {
        let hidden = lines.len() - limit;
        lines.truncate(limit);
        lines.push((Tint::Note, format!("# {hidden} more lines")));
    }
    lines
}

/// The changed middle of a diff, by longest common subsequence when it is
/// small enough to afford, else as a plain replacement.
fn middle_diff(old: &[&str], new: &[&str]) -> Vec<(Tint, String)> {
    let removed = |line: &&str| (Tint::Removed, format!("- {line}"));
    let added = |line: &&str| (Tint::Added, format!("+ {line}"));
    if old.is_empty() || new.is_empty() || old.len() * new.len() > 250_000 {
        return old
            .iter()
            .map(removed)
            .chain(new.iter().map(added))
            .collect();
    }
    let (rows, columns) = (old.len(), new.len());
    let mut table = vec![0u32; (rows + 1) * (columns + 1)];
    for i in (0..rows).rev() {
        for j in (0..columns).rev() {
            table[i * (columns + 1) + j] = if old[i] == new[j] {
                table[(i + 1) * (columns + 1) + j + 1] + 1
            } else {
                table[(i + 1) * (columns + 1) + j].max(table[i * (columns + 1) + j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut lines = Vec::new();
    while i < rows && j < columns {
        if old[i] == new[j] {
            lines.push((Tint::Plain, format!("  {}", old[i])));
            i += 1;
            j += 1;
        } else if table[(i + 1) * (columns + 1) + j] >= table[i * (columns + 1) + j + 1] {
            lines.push(removed(&old[i]));
            i += 1;
        } else {
            lines.push(added(&new[j]));
            j += 1;
        }
    }
    lines.extend(old[i..].iter().map(removed));
    lines.extend(new[j..].iter().map(added));
    lines
}

/// The keys of a session's steps that open into a well, oldest first. The
/// rows above build theirs the same way.
pub(super) fn expandable_keys(agent: &AgentSessionSnapshot) -> Vec<String> {
    agent
        .timeline
        .iter()
        .enumerate()
        .filter_map(|(index, item)| match item {
            AgentTimelineItem::Tool(tool) => Some(format!("{}:{}", agent.id, tool.id)),
            AgentTimelineItem::Message {
                role: AgentMessageRole::Thought,
                ..
            } => Some(format!("{}:thought:{index}", agent.id)),
            _ => None,
        })
        .collect()
}

fn item_time(item: &AgentTimelineItem) -> Option<u64> {
    match item {
        AgentTimelineItem::Message { at, .. } => *at,
        AgentTimelineItem::Tool(tool) => tool.started_at,
        AgentTimelineItem::Permission(permission) => permission.asked_at,
        AgentTimelineItem::TurnEnded { at, .. } => Some(*at),
        AgentTimelineItem::Plan(_)
        | AgentTimelineItem::Context { .. }
        | AgentTimelineItem::Error(_) => None,
    }
}

/// A session's state as the slab head says it, when there is one worth saying.
pub(super) fn agent_state(status: AgentSessionStatus) -> Option<(&'static str, u32)> {
    match status {
        AgentSessionStatus::Starting => Some(("starting", FAINT_TEXT)),
        AgentSessionStatus::Working => Some(("working", MUTED_TEXT)),
        AgentSessionStatus::WaitingForPermission => Some(("needs you", SIGNAL)),
        AgentSessionStatus::WaitingForAuthentication => Some(("sign in", SIGNAL)),
        AgentSessionStatus::Authenticating => Some(("signing in", FAINT_TEXT)),
        AgentSessionStatus::Failed => Some(("failed", SIGNAL)),
        AgentSessionStatus::Idle | AgentSessionStatus::Closed => None,
    }
}

/// The agent's name as the panes say it: lowercase, and short.
pub(super) fn agent_title(agent: &AgentSessionSnapshot) -> String {
    match agent_family(agent) {
        Some("claude") => "claude code".to_owned(),
        Some(family) => family.to_owned(),
        None => agent
            .agent_name
            .as_deref()
            .unwrap_or(&agent.name)
            .to_lowercase(),
    }
}

/// The name a message is signed with.
pub(super) fn agent_short_name(agent: &AgentSessionSnapshot) -> String {
    agent_family(agent).map_or_else(|| agent_title(agent), str::to_owned)
}

fn agent_family(agent: &AgentSessionSnapshot) -> Option<&'static str> {
    let name = format!(
        "{} {}",
        agent.name,
        agent.agent_name.as_deref().unwrap_or_default()
    )
    .to_lowercase();
    ["claude", "codex", "copilot", "gemini"]
        .into_iter()
        .find(|family| name.contains(family))
}

/// The short name an agent goes by in commands: `/new codex`.
pub(super) fn agent_handle(profile: &AgentProfile) -> String {
    match profile.id.as_str() {
        "claude-acp" => "claude".to_owned(),
        "codex-acp" => "codex".to_owned(),
        "github-copilot" => "copilot".to_owned(),
        id => id.to_owned(),
    }
}

/// Whether `/new <query>` means this agent: its id, its short name or its
/// title, in any case.
pub(super) fn profile_answers_to(profile: &AgentProfile, query: &str) -> bool {
    let query = query.trim();
    profile.id.eq_ignore_ascii_case(query)
        || agent_handle(profile).eq_ignore_ascii_case(query)
        || profile_title(profile).eq_ignore_ascii_case(query)
}

pub(super) fn profile_title(profile: &AgentProfile) -> String {
    match profile.id.as_str() {
        "claude-acp" => "claude code".to_owned(),
        "codex-acp" => "codex".to_owned(),
        "github-copilot" => "copilot".to_owned(),
        "gemini" => "gemini".to_owned(),
        _ => profile.name.to_lowercase(),
    }
}

fn install_hint(profile_id: &str) -> &'static str {
    match profile_id {
        "github-copilot" => "not installed",
        "claude-acp" | "codex-acp" | "gemini" => "needs node",
        _ => "not found on PATH",
    }
}

/// The shell command that makes an agent runnable here, when there is one.
pub(super) fn install_command(profile_id: &str) -> Option<&'static str> {
    match profile_id {
        "github-copilot" => Some("npm i -g @github/copilot"),
        "claude-acp" | "codex-acp" | "gemini" => Some("brew install node"),
        _ => None,
    }
}

fn agent_option_label(
    agent: &AgentSessionSnapshot,
    category: AgentConfigCategory,
) -> Option<String> {
    let option = agent
        .config_options
        .iter()
        .find(|option| option.category == category)?;
    match &option.value {
        AgentConfigValue::Select { current, choices } => Some(
            choices
                .iter()
                .find(|choice| choice.id == *current)
                .map_or_else(|| current.clone(), |choice| choice.name.clone())
                .to_lowercase(),
        ),
        AgentConfigValue::Boolean(_) => None,
    }
}

fn agent_mode_label(agent: &AgentSessionSnapshot) -> Option<String> {
    let current = agent.current_mode.as_deref()?;
    let name = agent
        .modes
        .iter()
        .find(|mode| mode.id == current)
        .map_or(current, |mode| mode.name.as_str());
    Some(name.to_lowercase())
}

fn relative_path(path: &Path, cwd: &Path) -> String {
    path.strip_prefix(cwd).map_or_else(
        |_| home_relative(path),
        |relative| {
            let shown = relative.display().to_string();
            if shown.is_empty() {
                ".".to_owned()
            } else {
                shown
            }
        },
    )
}

pub(super) fn home_relative(path: &Path) -> String {
    if let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from)
        && let Ok(relative) = path.strip_prefix(&home)
    {
        let shown = relative.display().to_string();
        return if shown.is_empty() {
            "~".to_owned()
        } else {
            format!("~/{shown}")
        };
    }
    path.display().to_string()
}

/// Text without terminal escapes or carriage returns.
fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '\u{1b}' => match chars.peek() {
                Some('[') => {
                    chars.next();
                    for next in chars.by_ref() {
                        if ('@'..='~').contains(&next) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    while let Some(next) = chars.next() {
                        if next == '\u{7}' {
                            break;
                        }
                        if next == '\u{1b}' {
                            chars.next();
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\r' => {}
            _ => out.push(character),
        }
    }
    out
}

/// The first non-empty line, without escapes, trimmed and kept short.
fn plain_line(text: &str) -> String {
    let text = plain(text);
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    chrome::truncate_chars(line.trim_matches('`'), 240)
}

/// A line of prose for a one-line row, without markdown's code ticks and
/// emphasis marks.
fn first_line(text: &str) -> String {
    let line = plain_line(text).replace('`', "").replace("**", "");
    line.trim_start_matches(['*', '#', ' '])
        .trim_end_matches('*')
        .to_owned()
}

/// Lowercase a sentence's first letter unless it starts an acronym or a name.
fn sentence_lower(text: &str) -> String {
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(first), Some(second)) if first.is_uppercase() && second.is_lowercase() => {
            let mut lowered: String = first.to_lowercase().collect();
            lowered.push(second);
            lowered.extend(chars);
            lowered
        }
        _ => text.to_owned(),
    }
}

/// Lowercase a short label word by word: `Always Allow` reads `always allow`,
/// while acronyms and names with inner capitals (`API`, `GitHub`) stay.
fn label_lower(text: &str) -> String {
    text.split(' ')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first)
                    if first.is_uppercase()
                        && word != "I"
                        && chars.clone().all(|rest| !rest.is_uppercase()) =>
                {
                    first.to_lowercase().chain(chars).collect()
                }
                _ => word.to_owned(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn size_label(characters: usize) -> String {
    if characters < 1000 {
        format!("{characters} chars")
    } else {
        let tenths = (characters + 50) / 100;
        format!("{}.{}k chars", tenths / 10, tenths % 10)
    }
}

fn local_time(ms: u64) -> Option<chrono::DateTime<chrono::Local>> {
    use chrono::TimeZone as _;
    chrono::Local
        .timestamp_millis_opt(i64::try_from(ms).ok()?)
        .single()
}

fn clock(ms: u64) -> String {
    local_time(ms).map_or_else(String::new, |time| time.format("%H:%M").to_string())
}

/// gofer's durations: `120 ms`, `3.4 s`, `2 min`, `1 h 5 min`.
fn duration_label(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms} ms")
    } else if ms < 60_000 {
        let tenths = (ms + 50) / 100;
        format!("{}.{} s", tenths / 10, tenths % 10)
    } else {
        let minutes = (ms + 30_000) / 60_000;
        if minutes < 60 {
            format!("{minutes} min")
        } else {
            format!("{} h {} min", minutes / 60, minutes % 60)
        }
    }
}

/// A stopwatch: `0:07`, `1:42`, `1:02:09`.
fn elapsed_label(ms: u64) -> String {
    let seconds = ms / 1000;
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tool(kind: AgentToolKind, raw_input: serde_json::Value) -> AgentTool {
        AgentTool {
            id: "tool".to_owned(),
            title: "Tool".to_owned(),
            kind,
            status: ToolStatus::Completed,
            detail: None,
            raw_input: Some(raw_input),
            raw_output: None,
            locations: Vec::new(),
            diffs: Vec::new(),
            started_at: Some(1_000),
            finished_at: Some(4_400),
        }
    }

    #[test]
    fn durations_read_like_gofer() {
        assert_eq!(duration_label(120), "120 ms");
        assert_eq!(duration_label(3_400), "3.4 s");
        assert_eq!(duration_label(59_960), "60.0 s");
        assert_eq!(duration_label(125_000), "2 min");
        assert_eq!(duration_label(3_900_000), "1 h 5 min");
        assert_eq!(elapsed_label(7_000), "0:07");
        assert_eq!(elapsed_label(3_729_000), "1:02:09");
    }

    #[test]
    fn shell_steps_show_the_script_not_the_shell() {
        let wrapped = tool(
            AgentToolKind::Execute,
            serde_json::json!({"command": ["bash", "-lc", "cargo test -p mux"]}),
        );
        assert_eq!(
            tool_preview(&wrapped, Path::new("/repo")),
            "cargo test -p mux"
        );
        let plain = tool(
            AgentToolKind::Execute,
            serde_json::json!({"command": "ls -la"}),
        );
        assert_eq!(tool_label(&plain), "shell");
        assert_eq!(tool_duration(&plain), Some(3_400));
    }

    #[test]
    fn file_steps_name_the_file_from_the_working_directory() {
        let mut read = tool(AgentToolKind::Read, serde_json::json!({}));
        read.locations = vec![mux_acp::AgentToolLocation {
            path: PathBuf::from("/repo/src/layout.rs"),
            line: Some(12),
        }];
        assert_eq!(tool_preview(&read, Path::new("/repo")), "src/layout.rs:12");
    }

    #[test]
    fn diffs_count_lines_and_keep_a_little_context() {
        let diff = AgentDiff {
            path: PathBuf::from("/repo/src/layout.rs"),
            old_text: Some("a\nb\nc\nd\n".to_owned()),
            new_text: "a\nb\nC\nd\ne\n".to_owned(),
        };
        assert_eq!(diff_stats(&diff), (2, 1));
        let lines = diff_lines(&diff, 40);
        assert_eq!(lines[0], (Tint::Plain, "  a".to_owned()));
        assert!(lines.contains(&(Tint::Removed, "- c".to_owned())));
        assert!(lines.contains(&(Tint::Added, "+ C".to_owned())));
        assert!(lines.contains(&(Tint::Added, "+ e".to_owned())));
        let new_file = AgentDiff {
            path: PathBuf::from("/repo/NEW.md"),
            old_text: None,
            new_text: "one\ntwo\n".to_owned(),
        };
        assert_eq!(diff_stats(&new_file), (2, 0));
    }

    #[test]
    fn terminal_escapes_never_reach_a_well() {
        assert_eq!(plain("\u{1b}[1;32mok\u{1b}[0m\r\n"), "ok\n");
        assert_eq!(plain("\u{1b}]0;title\u{7}done"), "done");
    }

    #[test]
    fn labels_lose_their_capital_but_not_their_acronyms() {
        assert_eq!(sentence_lower("Allow once"), "allow once");
        assert_eq!(sentence_lower("API key"), "API key");
        assert_eq!(size_label(1_234), "1.2k chars");
    }

    #[test]
    fn one_line_rows_drop_markdown_marks() {
        assert_eq!(
            first_line("`expand_home` takes a **&Path**\nand more"),
            "expand_home takes a &Path"
        );
        assert_eq!(first_line("## Planning the change"), "Planning the change");
    }

    #[test]
    fn every_built_in_agent_says_how_to_install_it() {
        for profile in mux_acp::built_in_agent_profiles() {
            assert!(
                install_command(&profile.id).is_some(),
                "the launcher has no install command for {}",
                profile.id
            );
        }
    }

    fn step(
        id: &str,
        status: ToolStatus,
        started_at: u64,
        finished_at: Option<u64>,
    ) -> AgentTimelineItem {
        AgentTimelineItem::Tool(AgentTool {
            id: id.to_owned(),
            status,
            started_at: Some(started_at),
            finished_at,
            ..tool(AgentToolKind::Execute, serde_json::json!({}))
        })
    }

    fn session(timeline: Vec<AgentTimelineItem>) -> AgentSessionSnapshot {
        let mut agent = AgentSessionSnapshot::new(
            AgentSessionId::new(),
            None,
            "codex".to_owned(),
            PathBuf::from("/src/mux"),
        );
        agent.timeline = timeline;
        agent
    }

    #[test]
    fn steps_only_read_as_stopped_when_your_interrupt_cut_them_off() {
        let said = |text: &str| AgentTimelineItem::Message {
            role: AgentMessageRole::Agent,
            message_id: None,
            text: text.to_owned(),
            at: Some(5_000),
        };
        let ended = |stop| AgentTimelineItem::TurnEnded { at: 9_000, stop };
        // Failed under the interrupt: stopped.
        let agent = session(vec![
            step("a", ToolStatus::Failed, 1_000, Some(8_900)),
            ended(AgentStopReason::Cancelled),
        ]);
        let AgentTimelineItem::Tool(failed) = &agent.timeline[0] else {
            unreachable!()
        };
        assert!(cut_short(&agent, 0, failed));
        // Failed, then the agent carried on before you stopped it: a failure.
        let agent = session(vec![
            step("a", ToolStatus::Failed, 1_000, Some(2_000)),
            said("that didn't work, trying another way"),
            step("b", ToolStatus::Failed, 6_000, Some(8_900)),
            ended(AgentStopReason::Cancelled),
        ]);
        let AgentTimelineItem::Tool(failed) = &agent.timeline[0] else {
            unreachable!()
        };
        assert!(!cut_short(&agent, 0, failed));
        // A step left running when a turn ends stops with it.
        let agent = session(vec![
            step("a", ToolStatus::Running, 1_000, None),
            ended(AgentStopReason::Cancelled),
        ]);
        assert_eq!(turn_end(&agent, 0), Some(AgentStopReason::Cancelled));
        // A turn still going has no end yet, and the next turn's end isn't it.
        let agent = session(vec![
            step("a", ToolStatus::Running, 1_000, None),
            AgentTimelineItem::Message {
                role: AgentMessageRole::User,
                message_id: None,
                text: "next".to_owned(),
                at: Some(9_500),
            },
            ended(AgentStopReason::EndTurn),
        ]);
        assert_eq!(turn_end(&agent, 0), None);
    }

    #[test]
    fn only_an_allow_counts_as_letting_a_step_run() {
        let option = |id: &str, kind| mux_acp::PermissionOption {
            id: id.to_owned(),
            label: id.to_owned(),
            kind,
        };
        let mut permission = AgentPermission {
            request_id: "request".to_owned(),
            tool_call_id: "tool".to_owned(),
            title: "Run tests".to_owned(),
            options: vec![
                option("allow", PermissionKind::AllowOnce),
                option("always", PermissionKind::AllowAlways),
                option("reject", PermissionKind::RejectOnce),
            ],
            selected_option: None,
            asked_at: Some(1_000),
            answered_at: Some(2_000),
        };
        assert!(was_declined(&permission), "a lapsed question ran nothing");
        for (selected, declined) in [("allow", false), ("always", false), ("reject", true)] {
            permission.selected_option = Some(selected.to_owned());
            assert_eq!(was_declined(&permission), declined, "{selected}");
        }
    }
}
