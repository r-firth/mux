//! A selection dragged past the top or bottom of its pane carries on into
//! the history, as in every terminal: the pane scrolls while the pointer
//! stays out there, faster the further out it is, and the selection
//! follows.

use super::*;

/// How often a pane scrolls while a selection is held past its edge.
const TICK: Duration = Duration::from_millis(50);
/// The most rows a pane scrolls a tick, however far out the pointer is.
const MOST_ROWS: f32 = 8.0;

/// A selection drag held past its pane's edge: where the pointer is, and
/// whether the selection is rectangular.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct SelectionEdge {
    position: gpui::Point<gpui::Pixels>,
    rectangular: bool,
}

/// How many rows to scroll a tick for a pointer `beyond` points past the
/// grid, negative going up into the history. Zero inside it.
fn rows_per_tick(above: f32, below: f32, cell_height: f32) -> i64 {
    let rows = |beyond: f32| {
        #[allow(clippy::cast_possible_truncation)]
        let rows = (1.0 + beyond / cell_height).floor().clamp(1.0, MOST_ROWS) as i64;
        rows
    };
    if above > 0.0 {
        -rows(above)
    } else if below > 0.0 {
        rows(below)
    } else {
        0
    }
}

impl MuxApp {
    /// How far past the top and bottom of the held pane's grid the pointer
    /// is, in points.
    fn beyond_grid(&self, position: gpui::Point<gpui::Pixels>) -> Option<(f32, f32)> {
        let capture = self.selection_drag?;
        let frame = &self.panes.get(&capture.pane_id)?.frame;
        let rect = capture.rect;
        let padding =
            self.metrics
                .balanced_padding(rect.width, rect.height, frame.cols, frame.rows);
        let top = rect.y + padding.top;
        let bottom = top + self.metrics.cell_height * f32::from(frame.rows);
        let y = f32::from(position.y);
        Some((top - y, y - bottom))
    }

    /// Note where a selection drag is: past the edge, the pane scrolls.
    pub(super) fn note_selection_edge(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        rectangular: bool,
    ) {
        self.selection_edge = self
            .beyond_grid(position)
            .filter(|&(above, below)| above > 0.0 || below > 0.0)
            .map(|_| SelectionEdge {
                position,
                rectangular,
            });
    }

    /// Keep scrolling while a selection is held past its pane's edge.
    pub(super) fn ensure_selection_scroll(&mut self, cx: &mut Context<Self>) {
        if self.selection_scrolling.is_some() || self.selection_edge.is_none() {
            return;
        }
        self.selection_scrolling = Some(Instant::now());
        cx.spawn(async move |entity, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                let going = entity
                    .update(cx, |this, cx| {
                        let going = this.scroll_selection();
                        if going {
                            cx.notify();
                        } else {
                            this.selection_scrolling = None;
                        }
                        going
                    })
                    .unwrap_or(false);
                if !going {
                    break;
                }
            }
        })
        .detach();
    }

    /// One tick: scroll the held pane toward the pointer and stretch the
    /// selection to the row now at that edge. Returns whether to go on.
    fn scroll_selection(&mut self) -> bool {
        let (Some(capture), Some(edge)) = (self.selection_drag, self.selection_edge) else {
            self.selection_edge = None;
            return false;
        };
        let Some((above, below)) = self.beyond_grid(edge.position) else {
            return false;
        };
        let rows = rows_per_tick(above, below, self.metrics.cell_height);
        if rows == 0 {
            self.selection_edge = None;
            return false;
        }
        self.scroll_viewport(capture.pane_id, TerminalViewportScroll::Delta(rows));
        let Some(frame) = self
            .panes
            .get(&capture.pane_id)
            .map(|pane| Rc::clone(&pane.frame))
        else {
            return false;
        };
        let pointer = self.selection_pointer(capture.rect, &frame, edge.position);
        let _ = self.apply_selection_gesture(
            capture.pane_id,
            TerminalSelectionGestureEvent::Drag {
                point: pointer.clamped_point,
                position: pointer.position,
                rectangular: edge.rectangular,
                geometry: pointer.geometry,
            },
        );
        true
    }
}

#[cfg(test)]
mod tests {
    use super::rows_per_tick;

    #[test]
    fn the_further_out_the_faster_it_scrolls() {
        assert_eq!(rows_per_tick(-5.0, -300.0, 20.0), 0, "inside the grid");
        assert_eq!(rows_per_tick(1.0, -400.0, 20.0), -1);
        assert_eq!(rows_per_tick(45.0, -400.0, 20.0), -3);
        assert_eq!(rows_per_tick(-400.0, 10.0, 20.0), 1);
        assert_eq!(rows_per_tick(-900.0, 900.0, 20.0), 8, "no faster than this");
    }
}
