//! Seams: the gaps between slabs, held to resize a split.
//!
//! A seam reads as ground until the pointer finds it: then a grip of three
//! dots in the tab's ink sits under the pointer and the pointer becomes a
//! resize arrow. Dragging moves the split with the pointer, the panes and
//! the programs in them following live, each pane's head saying its size;
//! letting go tells the daemon. The middle holds a seam for a few points on
//! the way past, and a double click glides the split to even.

use super::*;
use mux_workspace::{PaneLayout, SplitAxis, SplitRatio};

/// The two panes that name a seam's split, one from each side.
type SeamKey = (PaneId, PaneId);

/// A seam being dragged: which split, and where the pointer has it.
#[derive(Clone, Copy, Debug)]
pub(super) struct SeamDrag {
    seam: layout::Seam,
    /// The ratio when the drag began, to tell whether it moved.
    began: u16,
    ratio: u16,
}

/// Where the pointer is along a seam it is over, for the grip to sit under.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct SeamPointer {
    key: SeamKey,
    along: f32,
}

const GRIP_DOT: f32 = 3.0;
const GRIP_GAP: f32 = 3.0;
/// How near its middle, in points, a dragged seam settles there, so a split
/// can be evened out by hand.
const HALF_DETENT: f32 = 6.0;
/// How long a double-clicked split takes to glide to even, and how often it
/// moves on the way.
const SETTLE: Duration = Duration::from_millis(220);
const SETTLE_FRAME: Duration = Duration::from_millis(16);

/// A double-clicked split on its way to even.
#[derive(Clone, Copy, Debug)]
pub(super) struct SeamSettle {
    seam: layout::Seam,
    from: u16,
    began: Instant,
}

fn key_of(seam: &layout::Seam) -> SeamKey {
    (seam.first, seam.second)
}

/// The pointer's place across a seam (the axis its split divides) and along
/// it (the seam's own length).
fn across_and_along(axis: SplitAxis, position: gpui::Point<gpui::Pixels>) -> (f32, f32) {
    match axis {
        SplitAxis::Horizontal => (f32::from(position.x), f32::from(position.y)),
        SplitAxis::Vertical => (f32::from(position.y), f32::from(position.x)),
    }
}

impl MuxApp {
    pub(super) fn seam_dragging(&self) -> bool {
        self.seam_drag.is_some()
    }

    /// A seam's hold: invisible until the pointer is on it.
    pub(super) fn render_seam(
        &self,
        index: usize,
        seam: layout::Seam,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let key = key_of(&seam);
        let rect = seam.rect;
        let side_by_side = seam.axis == SplitAxis::Horizontal;
        let dragging = self.seam_drag.is_some_and(|drag| key_of(&drag.seam) == key);
        let grip_at = self
            .seam_pointer
            .filter(|pointer| pointer.key == key)
            .map(|pointer| pointer.along);
        let hold = div()
            .id(("seam", index))
            .absolute()
            .left(px(rect.x))
            .top(px(rect.y))
            .w(px(rect.width))
            .h(px(rect.height))
            .when(side_by_side, Styled::cursor_col_resize)
            .when(!side_by_side, Styled::cursor_row_resize)
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if !*hovered && !this.seam_dragging() && this.seam_pointer.take().is_some() {
                    cx.notify();
                }
            }))
            .on_mouse_move(
                cx.listener(move |this, event: &gpui::MouseMoveEvent, _, cx| {
                    if this.seam_dragging() {
                        return;
                    }
                    let (_, along) = across_and_along(seam.axis, event.position);
                    let pointer = SeamPointer { key, along };
                    if this.seam_pointer != Some(pointer) {
                        this.seam_pointer = Some(pointer);
                        cx.notify();
                    }
                }),
            )
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    this.begin_seam_drag(seam, event, cx);
                    cx.stop_propagation();
                    cx.notify();
                }),
            );
        let Some(along) = grip_at else {
            return hold.into_any_element();
        };
        let ink = self.active_ink().color();
        let length = GRIP_DOT * 3.0 + GRIP_GAP * 2.0;
        let (start, extent) = if side_by_side {
            (rect.y, rect.height)
        } else {
            (rect.x, rect.width)
        };
        // Keep the grip inside the seam, clear of the slabs' rounded corners.
        let offset = (along - start - length / 2.0).clamp(10.0, (extent - length - 10.0).max(10.0));
        let across = (if side_by_side {
            rect.width
        } else {
            rect.height
        } - GRIP_DOT)
            / 2.0;
        let dots = (0..3).map(|_| {
            div()
                .flex_none()
                .size(px(GRIP_DOT))
                .rounded(px(1.0))
                .bg(ink.opacity(if dragging { 1.0 } else { 0.8 }))
        });
        let grip = if side_by_side {
            v_flex().left(px(across)).top(px(offset))
        } else {
            h_flex().top(px(across)).left(px(offset))
        }
        .absolute()
        .gap(px(GRIP_GAP))
        .children(dots);
        hold.child(grip).into_any_element()
    }

    /// While a seam is dragged, a sheet over everything keeps the resize
    /// arrow and keeps the panes beneath from taking the pointer for theirs.
    pub(super) fn render_seam_drag_shield(&self) -> Option<gpui::AnyElement> {
        let drag = self.seam_drag?;
        let shield = div()
            .id("seam-drag")
            .absolute()
            .top_0()
            .left_0()
            .size_full();
        Some(
            if drag.seam.axis == SplitAxis::Horizontal {
                shield.cursor_col_resize()
            } else {
                shield.cursor_row_resize()
            }
            .into_any_element(),
        )
    }

    fn begin_seam_drag(
        &mut self,
        seam: layout::Seam,
        event: &gpui::MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        // A split still gliding to even stops where it is.
        self.seam_settle = None;
        if event.click_count >= 2 {
            self.seam_drag = None;
            self.even_split(seam, cx);
            return;
        }
        let (across, along) = across_and_along(seam.axis, event.position);
        let ratio = self
            .split_ratio(&seam)
            .unwrap_or_else(|| seam.thousandths_at(across));
        self.seam_pointer = Some(SeamPointer {
            key: key_of(&seam),
            along,
        });
        self.seam_drag = Some(SeamDrag {
            seam,
            began: ratio,
            ratio,
        });
    }

    /// Follow the pointer with the dragged split. Returns whether a seam is
    /// being dragged.
    pub(super) fn drag_seam(&mut self, event: &gpui::MouseMoveEvent) -> bool {
        let Some(drag) = self.seam_drag.as_mut() else {
            return false;
        };
        let (across, along) = across_and_along(drag.seam.axis, event.position);
        let ratio = if (across - drag.seam.middle()).abs() <= HALF_DETENT {
            SplitRatio::HALF.thousandths()
        } else {
            drag.seam.thousandths_at(across)
        };
        let seam = drag.seam;
        drag.ratio = ratio;
        self.seam_pointer = Some(SeamPointer {
            key: key_of(&seam),
            along,
        });
        self.set_split(&seam, ratio);
        true
    }

    /// Let go of a dragged seam, telling the daemon where it ended up.
    /// Returns whether a seam was being dragged.
    pub(super) fn end_seam_drag(&mut self) -> bool {
        let Some(drag) = self.seam_drag.take() else {
            return false;
        };
        // The grip comes back with the pointer's next move over a seam.
        self.seam_pointer = None;
        if drag.ratio != drag.began {
            self.send_split(&drag.seam, drag.ratio);
        }
        true
    }

    /// Even a split out: the daemon hears at once, and the slabs glide there
    /// unless motion is reduced.
    fn even_split(&mut self, seam: layout::Seam, cx: &mut Context<Self>) {
        let half = SplitRatio::HALF.thousandths();
        self.send_split(&seam, half);
        let from = self.split_ratio(&seam).unwrap_or(half);
        if self.motion == MotionPreference::Reduced || from == half {
            self.set_split(&seam, half);
            return;
        }
        self.seam_settle = Some(SeamSettle {
            seam,
            from,
            began: Instant::now(),
        });
        cx.spawn(async move |entity, cx| {
            loop {
                cx.background_executor().timer(SETTLE_FRAME).await;
                let settling = entity
                    .update(cx, |this, cx| {
                        let settling = this.settle_seam();
                        cx.notify();
                        settling
                    })
                    .unwrap_or(false);
                if !settling {
                    break;
                }
            }
        })
        .detach();
    }

    /// Move a gliding split a frame on. Returns whether it is still on its
    /// way.
    fn settle_seam(&mut self) -> bool {
        let Some(settle) = self.seam_settle else {
            return false;
        };
        let progress = (settle.began.elapsed().as_secs_f32() / SETTLE.as_secs_f32()).min(1.0);
        let eased = cubic_bezier(0.16, 1.0, 0.3, 1.0)(progress);
        let (from, half) = (
            f32::from(settle.from),
            f32::from(SplitRatio::HALF.thousandths()),
        );
        // Between two ratios that are both within 1..=999.
        let ratio = (from + (half - from) * eased).round() as u16;
        self.set_split(&settle.seam, ratio);
        if progress < 1.0 {
            true
        } else {
            self.seam_settle = None;
            false
        }
    }

    /// Hold a split being dragged where the pointer has it, over a snapshot
    /// of the workspace that arrived in the middle of the drag.
    pub(super) fn keep_seam_drag(&mut self) {
        if let Some(drag) = self.seam_drag {
            self.set_split(&drag.seam, drag.ratio);
        }
        if self.seam_settle.is_some() {
            self.settle_seam();
        }
    }

    /// The split's ratio as the window has it.
    fn split_ratio(&self, seam: &layout::Seam) -> Option<u16> {
        fn find(layout: &PaneLayout, first: PaneId, second: PaneId) -> Option<u16> {
            let PaneLayout::Split {
                ratio,
                first: first_side,
                second: second_side,
                ..
            } = layout
            else {
                return None;
            };
            match (first_side.contains(first), second_side.contains(second)) {
                (true, true) => Some(ratio.thousandths()),
                (true, false) => find(first_side, first, second),
                (false, true) => find(second_side, first, second),
                (false, false) => None,
            }
        }
        let tab = self.session.as_ref()?.active_tab()?;
        find(&tab.layout, seam.first, seam.second)
    }

    /// Move the window's own copy of the split, so the slabs follow the
    /// pointer before the daemon hears of it.
    fn set_split(&mut self, seam: &layout::Seam, thousandths: u16) {
        if let (Ok(ratio), Some(session)) = (SplitRatio::new(thousandths), self.session.as_mut()) {
            let _ = session.resize_split(seam.first, seam.second, ratio);
        }
    }

    fn send_split(&mut self, seam: &layout::Seam, thousandths: u16) {
        let Ok(ratio) = SplitRatio::new(thousandths) else {
            return;
        };
        self.send_workspace(WorkspaceCommand::ResizeSplit {
            first: seam.first,
            second: seam.second,
            ratio,
        });
    }
}
