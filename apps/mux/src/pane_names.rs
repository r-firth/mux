//! Naming a pane, as Zellij's Pane mode `c` does: `server`, `logs`, `tests`.
//! The name turns into a field in the pane's own head, Return keeps it and
//! Escape puts it back. A named pane's head shows the name, with what its
//! shell says beside it, and a tab without a name of its own goes by its
//! focused pane's name. Clearing the field hands the head back to the shell.

use gpui::{
    AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    KeyDownEvent, ParentElement as _, Styled as _, Window, div, px,
};
use gpui_component::Sizable as _;
use gpui_component::input::{Input, InputEvent, InputState};

use super::{
    EMBEDDED_TERMINAL_FONT, InputMode, MuxApp, PaneId, PaneReplica, STRIP_CHAR_WIDTH, TEXT,
    WorkspaceCommand, color,
};

/// A pane being named where its name is shown, in its head.
pub(super) struct PaneRename {
    pub(super) pane_id: PaneId,
    input: Entity<InputState>,
    /// What the head said before. Entering it unchanged leaves the pane as
    /// it was, so a pane that follows its shell keeps following it.
    shown: String,
    _subscription: gpui::Subscription,
}

impl MuxApp {
    /// The name given to a pane, if it has one.
    pub(super) fn pane_name(&self, pane_id: PaneId) -> Option<&str> {
        self.session.as_ref()?.pane_name(pane_id)
    }

    /// Name the focused terminal pane in its head.
    pub(super) fn open_rename_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pane_id) = self.focused_pane_id() else {
            return;
        };
        if self.active_agent_pane() == Some(pane_id) {
            return;
        }
        let shown = self.pane_name(pane_id).map_or_else(
            || {
                self.panes
                    .get(&pane_id)
                    .and_then(PaneReplica::head_title)
                    .unwrap_or_default()
            },
            str::to_owned,
        );
        let input = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_value(shown.clone(), window, cx);
            input.select_all(window, cx);
            input
        });
        let subscription = cx.subscribe_in(
            &input,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } | InputEvent::Blur => {
                    this.finish_pane_rename(true, window, cx);
                }
                InputEvent::Change => cx.notify(),
                InputEvent::Focus => {}
            },
        );
        let focus = input.clone();
        window.on_next_frame(move |window, cx| {
            focus.update(cx, |input, cx| input.focus(window, cx));
        });
        self.pane_rename = Some(PaneRename {
            pane_id,
            input,
            shown,
            _subscription: subscription,
        });
        self.mode = InputMode::Normal;
        cx.notify();
    }

    pub(super) fn finish_pane_rename(
        &mut self,
        keep: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(rename) = self.pane_rename.take() else {
            return;
        };
        let value = rename.input.read(cx).value().trim().to_owned();
        if keep && value != rename.shown {
            self.send_workspace(WorkspaceCommand::RenamePane {
                pane: rename.pane_id,
                name: value,
            });
        }
        self.focus_handle.focus(window, cx);
        cx.notify();
    }

    /// The head's title as a field while the pane is being named, as wide
    /// as what is typed.
    pub(super) fn render_pane_rename_field(
        &self,
        pane_id: PaneId,
        text_size: f32,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let rename = self
            .pane_rename
            .as_ref()
            .filter(|rename| rename.pane_id == pane_id)?;
        #[allow(clippy::cast_precision_loss)]
        let chars = rename.input.read(cx).value().chars().count().clamp(6, 40) as f32;
        Some(
            div()
                .flex_none()
                .relative()
                .left(px(-4.0))
                // The strip's character width is for its 12.5pt text.
                .w(px((chars + 2.0) * STRIP_CHAR_WIDTH * text_size / 12.5 + 8.0))
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        this.finish_pane_rename(false, window, cx);
                        cx.stop_propagation();
                    }
                }))
                .child(
                    Input::new(&rename.input)
                        .appearance(false)
                        .xsmall()
                        .font_family(EMBEDDED_TERMINAL_FONT)
                        .text_size(px(text_size))
                        .font_weight(FontWeight::BOLD)
                        .text_color(color(TEXT)),
                )
                .into_any_element(),
        )
    }
}
