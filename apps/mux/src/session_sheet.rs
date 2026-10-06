//! The sessions sheet.
//!
//! mux's durable sessions in a slab that drops from the session's dot-matrix
//! name in the strip, listed the way gofer lists things: a peach mark on the
//! chosen row, the name, what it holds, keys in brackets. Renaming happens
//! in the row and ending a session asks in the row, so there is no dialog.
//! The foot of the sheet always shows the keys that work right now.

use super::*;

const SHEET_WIDTH: f32 = 420.0;
const ROW_HEIGHT: f32 = 30.0;

pub(super) struct SessionSheet {
    focus: FocusHandle,
    /// The session the keys act on, followed by id because the list is kept
    /// in name order and a rename can move it. `None` means the current one.
    selected: Option<SessionId>,
    rename: Option<SessionRename>,
    /// The session the sheet is asking whether to end.
    ending: Option<SessionId>,
}

struct SessionRename {
    session_id: SessionId,
    input: Entity<InputState>,
    _subscription: gpui::Subscription,
}

impl MuxApp {
    /// Open the sheet, or close it if it is open: the same key does both.
    pub(super) fn toggle_session_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.session_sheet.is_some() {
            self.close_session_sheet(window, cx);
            return;
        }
        self.backend.send(CommandMessage::ListSessions);
        let focus = cx.focus_handle();
        let deferred = focus.clone();
        window.on_next_frame(move |window, cx| deferred.focus(window, cx));
        self.session_sheet = Some(SessionSheet {
            focus,
            selected: None,
            rename: None,
            ending: None,
        });
        self.mode = InputMode::Normal;
        cx.notify();
    }

    pub(super) fn close_session_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.session_sheet.take().is_some() {
            self.restore_keyboard(window, cx);
            cx.notify();
        }
    }

    /// Give the keyboard back to the focused pane: its composer when it is
    /// showing an agent, else its shell.
    pub(super) fn restore_keyboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .active_agent_pane()
            .is_some_and(|pane_id| Some(pane_id) == self.focused_pane_id())
        {
            self.focus_agent_composer(window);
        } else {
            self.focus_handle.focus(window, cx);
        }
    }

    /// The row the keys act on: the chosen session while it is listed, else
    /// the current one, else the first.
    fn session_sheet_row(&self) -> Option<usize> {
        let sheet = self.session_sheet.as_ref()?;
        let row_of = |session_id: Option<SessionId>| {
            let session_id = session_id?;
            self.sessions
                .iter()
                .position(|session| session.id == session_id)
        };
        row_of(sheet.selected)
            .or_else(|| row_of(self.session.as_ref().map(|session| session.id)))
            .or_else(|| (!self.sessions.is_empty()).then_some(0))
    }

    fn session_sheet_choice(&self) -> Option<&SessionSummary> {
        self.session_sheet_row()
            .and_then(|row| self.sessions.get(row))
    }

    pub(super) fn session_sheet_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        let Some(sheet) = self.session_sheet.as_mut() else {
            return;
        };
        if sheet.rename.is_some() {
            // The field has the keyboard; esc is the sheet's to take back.
            if key == "escape" {
                sheet.rename = None;
                sheet.focus.focus(window, cx);
                cx.stop_propagation();
                cx.notify();
            }
            return;
        }
        if let Some(session_id) = sheet.ending {
            match key {
                "y" | "enter" => {
                    sheet.ending = None;
                    self.backend.send(CommandMessage::KillSession(session_id));
                }
                "n" | "escape" => sheet.ending = None,
                _ => {}
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        let toggles = key_chord(&event.keystroke).is_some_and(|chord| {
            self.keymap.resolve(InputMode::Normal, chord) == Some(&Action::OpenSessionSwitcher)
        });
        if toggles || key == "escape" {
            self.close_session_sheet(window, cx);
            cx.stop_propagation();
            return;
        }
        if event.keystroke.modifiers.modified() {
            return;
        }
        match key {
            "up" | "k" => self.move_session_sheet(-1),
            "down" | "j" => self.move_session_sheet(1),
            "enter" => self.open_session_sheet_choice(window, cx),
            "n" => self.new_session_from_sheet(window, cx),
            "r" => self.rename_session_in_sheet(window, cx),
            "x" => {
                let session_id = self.session_sheet_choice().map(|session| session.id);
                if let Some(sheet) = self.session_sheet.as_mut() {
                    sheet.ending = session_id;
                }
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn move_session_sheet(&mut self, delta: isize) {
        let Some(row) = self.session_sheet_row() else {
            return;
        };
        let row = wrapping_step(row, delta, self.sessions.len());
        let session_id = self.sessions.get(row).map(|session| session.id);
        if let Some(sheet) = self.session_sheet.as_mut() {
            sheet.selected = session_id;
        }
    }

    fn open_session_sheet_choice(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session_id) = self.session_sheet_choice().map(|session| session.id) else {
            return;
        };
        if self.session.as_ref().map(|session| session.id) != Some(session_id) {
            self.backend.send(CommandMessage::AttachSession(session_id));
        }
        self.close_session_sheet(window, cx);
    }

    /// A fresh session with one shell, started where the focused pane is.
    fn new_session_from_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pane_id) = self.focused_pane_id() else {
            return;
        };
        let mut number = self.sessions.len() + 1;
        let name = loop {
            let candidate = format!("session-{number}");
            if !self
                .sessions
                .iter()
                .any(|session| session.name == candidate)
            {
                break candidate;
            }
            number += 1;
        };
        self.backend
            .send(CommandMessage::CreateSessionForPane { name, pane_id });
        self.close_session_sheet(window, cx);
    }

    fn rename_session_in_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.session_sheet_choice().cloned() else {
            return;
        };
        let input = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_value(session.name.clone(), window, cx);
            input.select_all(window, cx);
            input
        });
        let session_id = session.id;
        let subscription = cx.subscribe_in(
            &input,
            window,
            move |this, input, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } | InputEvent::Blur => {
                    let Some(sheet) = this.session_sheet.as_mut() else {
                        return;
                    };
                    if sheet.rename.take().is_none() {
                        return;
                    }
                    let name = input.read(cx).value().trim().to_owned();
                    if !name.is_empty() && name != session.name {
                        this.backend
                            .send(CommandMessage::RenameSession { session_id, name });
                    }
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        sheet.focus.focus(window, cx);
                    }
                    cx.notify();
                }
                InputEvent::Change => cx.notify(),
                InputEvent::Focus => {}
            },
        );
        let focus = input.clone();
        window.on_next_frame(move |window, cx| {
            focus.update(cx, |input, cx| input.focus(window, cx));
        });
        if let Some(sheet) = self.session_sheet.as_mut() {
            sheet.selected = Some(session_id);
            sheet.rename = Some(SessionRename {
                session_id,
                input,
                _subscription: subscription,
            });
        }
    }

    /// The sheet over everything, under the strip's right end, with a
    /// clear backdrop that closes it when clicked.
    pub(super) fn render_session_sheet(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let sheet = self.session_sheet.as_ref()?;
        let current = self.session.as_ref().map(|session| session.id);
        let chosen = self.session_sheet_row();
        let mut rows = v_flex().gap(px(2.0));
        if self.sessions.is_empty() {
            rows = rows.child(
                h_flex()
                    .h(px(ROW_HEIGHT))
                    .pl(px(24.0))
                    .text_color(color(FAINT_TEXT))
                    .child("looking for sessions…"),
            );
        }
        for (index, session) in self.sessions.iter().enumerate() {
            rows = rows.child(Self::session_sheet_line(
                sheet,
                session,
                chosen == Some(index),
                current == Some(session.id),
                cx,
            ));
        }
        let panel = v_flex()
            .id("session-sheet")
            .track_focus(&sheet.focus)
            .on_key_down(cx.listener(Self::session_sheet_key))
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .absolute()
            .top(px(layout::TAB_BAR_HEIGHT + 2.0))
            .right(px(layout::WORKSPACE_INSET))
            .w(px(SHEET_WIDTH))
            .p(px(6.0))
            .pb(px(10.0))
            .gap(px(8.0))
            .rounded(px(12.0))
            .border_1()
            .border_color(hairline(0.10))
            .bg(color(SURFACE))
            .shadow_lg()
            .font_family(EMBEDDED_TERMINAL_FONT)
            .text_size(px(13.0))
            .text_color(color(TEXT))
            .child(
                h_flex()
                    .h(px(28.0))
                    .pl(px(24.0))
                    .pr(px(10.0))
                    .justify_between()
                    .child(div().font_weight(FontWeight::BOLD).child("sessions"))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(color(FAINT_TEXT))
                            .child("they outlive this window"),
                    ),
            )
            .child(rows)
            .child(session_sheet_hints(sheet));
        let panel = if self.motion == MotionPreference::Reduced {
            panel.into_any_element()
        } else {
            panel
                .with_animation(
                    "session-sheet-in",
                    interface_animation(140),
                    |panel, delta| {
                        panel
                            .opacity(delta)
                            .top(px(layout::TAB_BAR_HEIGHT + 2.0 - 6.0 * (1.0 - delta)))
                    },
                )
                .into_any_element()
        };
        Some(
            div()
                .id("session-sheet-backdrop")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .occlude()
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close_session_sheet(window, cx)),
                )
                .child(panel)
                .into_any_element(),
        )
    }

    fn session_sheet_line(
        sheet: &SessionSheet,
        session: &SessionSummary,
        chosen: bool,
        current: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let session_id = session.id;
        let ending = sheet.ending == Some(session_id);
        let mut line = h_flex()
            .id(SharedString::from(format!("session-line-{session_id}")))
            .relative()
            .h(px(ROW_HEIGHT))
            .pl(px(24.0))
            .pr(px(10.0))
            .gap(px(12.0))
            .rounded(px(6.0))
            .cursor_pointer()
            .when(chosen, |line| line.bg(wash(0.08)))
            .when(!chosen, |line| line.hover(|line| line.bg(wash(0.04))))
            .on_click(cx.listener(move |this, _, window, cx| {
                if let Some(sheet) = this.session_sheet.as_mut() {
                    sheet.selected = Some(session_id);
                }
                this.open_session_sheet_choice(window, cx);
            }))
            .child(
                div()
                    .absolute()
                    .left(px(8.0))
                    .text_color(color(SIGNAL))
                    .when(!chosen, gpui::Styled::invisible)
                    .child("›"),
            );
        if let Some(rename) = sheet
            .rename
            .as_ref()
            .filter(|rename| rename.session_id == session_id)
        {
            line = line.child(
                div()
                    .flex_1()
                    .min_w_0()
                    // The field pads its text by 4px; shift it onto the name.
                    .relative()
                    .left(px(-4.0))
                    .child(
                        Input::new(&rename.input)
                            .appearance(false)
                            .xsmall()
                            .font_family(EMBEDDED_TERMINAL_FONT)
                            .text_size(px(13.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(color(TEXT)),
                    ),
            );
        } else {
            line = line
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .when(chosen, |name| name.font_weight(FontWeight::BOLD))
                        .when(ending, |name| name.text_color(color(SIGNAL)))
                        .child(if ending {
                            format!("end {}?", session.name)
                        } else {
                            session.name.clone()
                        }),
                )
                .when(current && !ending, |line| {
                    line.child(div().flex_none().text_color(color(SAGE)).child("current"))
                })
                .child(div().flex_1());
        }
        // While the sheet asks, the count gives way to what ending costs.
        line.child(
            div()
                .flex_none()
                .text_color(color(FAINT_TEXT))
                .child(if ending {
                    "its shells exit".to_owned()
                } else {
                    format_session_pane_count(session.pane_count)
                }),
        )
        .into_any_element()
    }
}

/// The keys that work right now, in gofer's `[key] label` form.
fn session_sheet_hints(sheet: &SessionSheet) -> impl IntoElement {
    let hints: &[(&str, &str)] = if sheet.ending.is_some() {
        &[("y", "end"), ("n", "keep")]
    } else if sheet.rename.is_some() {
        &[("↵", "save"), ("esc", "cancel")]
    } else {
        &[
            ("↑↓", "choose"),
            ("↵", "open"),
            ("n", "new"),
            ("r", "rename"),
            ("x", "end"),
        ]
    };
    h_flex()
        .gap(px(14.0))
        .pl(px(24.0))
        .text_size(px(12.0))
        .text_color(color(FAINT_TEXT))
        .children(hints.iter().map(|(key, label)| {
            let key_color = if *key == "y" { SIGNAL } else { MUTED_TEXT };
            h_flex()
                .flex_none()
                .gap(px(6.0))
                .child(kbd(*key, color(key_color)))
                .child(*label)
        }))
}
