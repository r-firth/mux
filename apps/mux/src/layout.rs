use mux_workspace::{PaneId, PaneLayout, Session, SplitAxis};

pub const TAB_BAR_HEIGHT: f32 = 28.0;
pub const PANE_GAP: f32 = 1.0;

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
    let bounds = Rect {
        x: 0.0,
        y: TAB_BAR_HEIGHT,
        width,
        height: (height - TAB_BAR_HEIGHT).max(1.0),
    };

    if let Some(tab) = session.active_tab() {
        if let Some(zoomed) = tab.zoomed_pane {
            geometry.panes.push(PaneGeometry {
                pane_id: zoomed,
                rect: bounds,
                focused: true,
            });
        } else {
            layout_panes(&tab.layout, bounds, tab.focused_pane, &mut geometry.panes);
        }
    }
    geometry
}

fn layout_panes(
    layout: &PaneLayout,
    bounds: Rect,
    focused_pane: PaneId,
    output: &mut Vec<PaneGeometry>,
) {
    match layout {
        PaneLayout::Leaf(pane_id) => output.push(PaneGeometry {
            pane_id: *pane_id,
            rect: bounds,
            focused: *pane_id == focused_pane,
        }),
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
    fn pane_geometry_reserves_exact_gpui_tab_bar_height() {
        let pane = PaneId::new();
        let session = Session::with_panes("daily", &[pane]).expect("session");
        let geometry = calculate(&session, 800.0, 600.0);

        assert_eq!(
            geometry.panes[0].rect,
            Rect {
                x: 0.0,
                y: TAB_BAR_HEIGHT,
                width: 800.0,
                height: 600.0 - TAB_BAR_HEIGHT,
            }
        );
    }
}
