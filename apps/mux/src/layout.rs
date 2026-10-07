use mux_workspace::{PaneId, PaneLayout, Session, SplitAxis};

/// The single top strip: traffic lights, tabs, mode and session.
pub const TAB_BAR_HEIGHT: f32 = 36.0;
/// Breathing room between the strip and the first row of slabs.
pub const WORKSPACE_TOP_GAP: f32 = 4.0;
/// Ground left visible around the slabs on the left, right and bottom. The
/// gaps match gofer's: wide enough to read as ground, not as a seam.
pub const WORKSPACE_INSET: f32 = 10.0;
pub const PANE_GAP: f32 = 10.0;
/// The slim title row at the top of every pane slab.
pub const PANE_HEAD_HEIGHT: f32 = 24.0;
const PANE_BODY_INSET_X: f32 = 6.0;
const PANE_BODY_INSET_Y: f32 = 2.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaneGeometry {
    pub pane_id: PaneId,
    /// The whole slab, including its head.
    pub frame: Rect,
    /// The terminal surface inside the slab. Pointer and grid maths use this.
    pub rect: Rect,
    pub focused: bool,
}

#[derive(Clone, Debug, Default)]
pub struct WorkspaceGeometry {
    pub panes: Vec<PaneGeometry>,
}

#[must_use]
pub fn calculate(session: &Session, width: f32, height: f32) -> WorkspaceGeometry {
    let mut geometry = WorkspaceGeometry::default();
    let top = TAB_BAR_HEIGHT + WORKSPACE_TOP_GAP;
    let bounds = Rect {
        x: WORKSPACE_INSET,
        y: top,
        width: (width - WORKSPACE_INSET * 2.0).max(1.0),
        height: (height - top - WORKSPACE_INSET).max(1.0),
    };

    if let Some(tab) = session.active_tab() {
        if let Some(zoomed) = tab.zoomed_pane {
            geometry.panes.push(pane_geometry(zoomed, bounds, true));
        } else {
            layout_panes(&tab.layout, bounds, tab.focused_pane, &mut geometry.panes);
        }
    }
    geometry
}

fn pane_geometry(pane_id: PaneId, frame: Rect, focused: bool) -> PaneGeometry {
    PaneGeometry {
        pane_id,
        frame,
        rect: Rect {
            x: frame.x + PANE_BODY_INSET_X,
            y: frame.y + PANE_HEAD_HEIGHT + PANE_BODY_INSET_Y,
            width: (frame.width - PANE_BODY_INSET_X * 2.0).max(1.0),
            height: (frame.height - PANE_HEAD_HEIGHT - PANE_BODY_INSET_Y * 2.0).max(1.0),
        },
        focused,
    }
}

fn layout_panes(
    layout: &PaneLayout,
    bounds: Rect,
    focused_pane: PaneId,
    output: &mut Vec<PaneGeometry>,
) {
    match layout {
        PaneLayout::Leaf(pane_id) => {
            output.push(pane_geometry(*pane_id, bounds, *pane_id == focused_pane));
        }
        PaneLayout::Split {
            axis,
            ratio,
            first,
            second,
        } => {
            let ratio = f32::from(ratio.thousandths()) / 1_000.0;
            let (first_rect, second_rect) = match axis {
                SplitAxis::Horizontal => {
                    let available = (bounds.width - PANE_GAP).max(0.0);
                    let first_width = (available * ratio).round();
                    (
                        Rect {
                            width: first_width,
                            ..bounds
                        },
                        Rect {
                            x: bounds.x + first_width + PANE_GAP,
                            width: available - first_width,
                            ..bounds
                        },
                    )
                }
                SplitAxis::Vertical => {
                    let available = (bounds.height - PANE_GAP).max(0.0);
                    let first_height = (available * ratio).round();
                    (
                        Rect {
                            height: first_height,
                            ..bounds
                        },
                        Rect {
                            y: bounds.y + first_height + PANE_GAP,
                            height: available - first_height,
                            ..bounds
                        },
                    )
                }
            };
            layout_panes(first, first_rect, focused_pane, output);
            layout_panes(second, second_rect, focused_pane, output);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_slab_sits_on_the_ground_below_the_strip() {
        let pane = PaneId::new();
        let session = Session::with_panes("daily", &[pane]).expect("session");
        let geometry = calculate(&session, 800.0, 600.0);

        assert_eq!(
            geometry.panes[0].frame,
            Rect {
                x: WORKSPACE_INSET,
                y: TAB_BAR_HEIGHT + WORKSPACE_TOP_GAP,
                width: 800.0 - WORKSPACE_INSET * 2.0,
                height: 600.0 - TAB_BAR_HEIGHT - WORKSPACE_TOP_GAP - WORKSPACE_INSET,
            }
        );
    }

    #[test]
    fn terminal_surface_sits_inside_the_slab_below_its_head() {
        let pane = PaneId::new();
        let session = Session::with_panes("daily", &[pane]).expect("session");
        let geometry = calculate(&session, 800.0, 600.0);
        let pane = geometry.panes[0];

        assert_eq!(
            pane.rect,
            Rect {
                x: pane.frame.x + PANE_BODY_INSET_X,
                y: pane.frame.y + PANE_HEAD_HEIGHT + PANE_BODY_INSET_Y,
                width: pane.frame.width - PANE_BODY_INSET_X * 2.0,
                height: pane.frame.height - PANE_HEAD_HEIGHT - PANE_BODY_INSET_Y * 2.0,
            }
        );
    }
}
