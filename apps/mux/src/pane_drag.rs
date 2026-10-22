//! Moving a pane by its head.
//!
//! A pane's head can be dragged onto another pane of the tab. Dropped in the
//! middle, the two trade places, as Shift with h, j, k or l does in Pane
//! mode; dropped near an edge, the dragged pane moves to that side of the
//! other and they share its room. The pane it is over shows which, and the
//! dragged pane keeps the keys.
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

    /// Let a pane take another pane dropped on its `body`: in the middle
    /// the two trade places, near an edge the dropped pane goes to that
    /// side.
    pub(super) fn with_pane_drop(
        surface: gpui::Div,
        pane_id: PaneId,
        body: layout::Rect,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        surface
            .drag_over::<DraggedPane>(move |style, dragged, window, _| {
                if dragged.pane_id == pane_id {
                    return style;
                }
                let edge = dragged.ink.color();
                match drop_side(body, window.mouse_position()) {
                    Some(Direction::Left) => style.border_l_4().border_color(edge),
                    Some(Direction::Right) => style.border_r_4().border_color(edge),
                    Some(Direction::Up) => style.border_t_4().border_color(edge),
                    Some(Direction::Down) => style.border_b_4().border_color(edge),
                    None => style.border_2().border_color(edge.opacity(0.7)),
                }
            })
            .on_drop(cx.listener(move |this, dragged: &DraggedPane, window, _| {
                if dragged.pane_id == pane_id {
                    return;
                }
                this.send_workspace(match drop_side(body, window.mouse_position()) {
                    Some(side) => WorkspaceCommand::MovePaneBeside {
                        pane: dragged.pane_id,
                        target: pane_id,
                        side,
                    },
                    None => WorkspaceCommand::SwapPanes {
                        moved: dragged.pane_id,
                        with: pane_id,
                    },
                });
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

/// The side of `body` a pane dropped at `at` goes to: the nearest edge, when
/// it is within a quarter of the way in, or none in the middle.
fn drop_side(body: layout::Rect, at: gpui::Point<gpui::Pixels>) -> Option<Direction> {
    const EDGE: f32 = 0.25;
    let x = (f32::from(at.x) - body.x) / body.width.max(1.0);
    let y = (f32::from(at.y) - body.y) / body.height.max(1.0);
    [
        (x, Direction::Left),
        (1.0 - x, Direction::Right),
        (y, Direction::Up),
        (1.0 - y, Direction::Down),
    ]
    .into_iter()
    .filter(|(distance, _)| *distance < EDGE)
    .min_by(|(left, _), (right, _)| left.total_cmp(right))
    .map(|(_, side)| side)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_take_the_pane_beside_and_the_middle_swaps() {
        let body = layout::Rect {
            x: 100.0,
            y: 50.0,
            width: 400.0,
            height: 200.0,
        };
        let at = |x: f32, y: f32| gpui::point(px(x), px(y));
        assert_eq!(drop_side(body, at(300.0, 150.0)), None);
        assert_eq!(drop_side(body, at(120.0, 150.0)), Some(Direction::Left));
        assert_eq!(drop_side(body, at(480.0, 150.0)), Some(Direction::Right));
        assert_eq!(drop_side(body, at(300.0, 60.0)), Some(Direction::Up));
        assert_eq!(drop_side(body, at(130.0, 240.0)), Some(Direction::Down));
    }
}
