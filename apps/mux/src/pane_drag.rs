//! Moving a pane by its head.
//!
//! A pane's head can be dragged onto another pane of the tab, and the two
//! trade places, as Shift with h, j, k or l does in Pane mode. The dragged
//! pane keeps the keys; the pane it is over shows it will be the one moved.
//! Dropped on another tab's chip, the pane moves into that tab; dropped on
//! the strip's + it gets a tab of its own.

use super::*;

/// A pane on its way to another pane's place.
#[derive(Clone)]
pub(super) struct DraggedPane {
    pane_id: PaneId,
    label: SharedString,
    ink: Ink,
}

impl Render for DraggedPane {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .h(px(24.0))
            .px(px(10.0))
            .gap(px(8.0))
            .items_center()
            .rounded(px(6.0))
            .bg(color(SURFACE))
            .border_1()
            .border_color(self.ink.color().opacity(0.6))
            .shadow_md()
            .font_family(EMBEDDED_TERMINAL_FONT)
            .text_size(px(12.0))
            .text_color(color(TEXT))
            .child(div().size(px(6.0)).rounded_full().bg(self.ink.color()))
            .child(self.label.clone())
    }
}

impl MuxApp {
    /// Let a pane's head be dragged onto another pane.
    pub(super) fn with_pane_drag(
        head: gpui::Stateful<gpui::Div>,
        pane_id: PaneId,
        label: SharedString,
        ink: Ink,
    ) -> gpui::Stateful<gpui::Div> {
        head.on_drag(
            DraggedPane {
                pane_id,
                label,
                ink,
            },
            |dragged, _, _, cx| cx.new(|_| dragged.clone()),
        )
    }

    /// Let a pane take another pane dropped on it: the two trade places.
    pub(super) fn with_pane_drop(
        surface: gpui::Div,
        pane_id: PaneId,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        surface
            .drag_over::<DraggedPane>(move |style, dragged, _, _| {
                if dragged.pane_id == pane_id {
                    style
                } else {
                    style
                        .border_2()
                        .border_color(dragged.ink.color().opacity(0.7))
                }
            })
            .on_drop(cx.listener(move |this, dragged: &DraggedPane, _, _| {
                if dragged.pane_id != pane_id {
                    this.send_workspace(WorkspaceCommand::SwapPanes {
                        moved: dragged.pane_id,
                        with: pane_id,
                    });
                }
            }))
    }

    /// Let a tab's chip take a pane dropped on it: the pane moves into the
    /// tab.
    pub(super) fn with_chip_pane_drop(
        chip: gpui::Stateful<gpui::Div>,
        tab_id: TabId,
        active: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        if active {
            return chip;
        }
        chip.drag_over::<DraggedPane>(|style, dragged, _, _| {
            style
                .bg(wash(0.10))
                .border_b_2()
                .border_color(dragged.ink.color())
        })
        .on_drop(cx.listener(move |this, dragged: &DraggedPane, _, _| {
            this.send_workspace(WorkspaceCommand::MovePaneToTab {
                pane: dragged.pane_id,
                tab: tab_id,
            });
        }))
    }

    /// Let the strip's + take a pane dropped on it: the pane gets a tab of
    /// its own.
    pub(super) fn with_new_tab_pane_drop(
        button: gpui::Stateful<gpui::Div>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        button
            .drag_over::<DraggedPane>(|style, dragged, _, _| {
                style.bg(wash(0.10)).text_color(dragged.ink.color())
            })
            .on_drop(cx.listener(|this, dragged: &DraggedPane, _, _| {
                this.send_workspace(WorkspaceCommand::SetFocusedPane(dragged.pane_id));
                this.send_workspace(WorkspaceCommand::BreakPaneToTab);
            }))
    }
}
