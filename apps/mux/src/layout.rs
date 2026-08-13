use mux_workspace::{PaneId, PaneLayout, Session, SplitAxis, TabId};

pub const TAB_BAR_HEIGHT: f32 = 26.0;
pub const MODE_BAR_HEIGHT: f32 = 30.0;
pub const PANE_GAP: f32 = 1.0;
// Match Ghostty's default terminal padding. Keeping this small also maximizes
// useful grid columns in narrow panes and minimizes right-edge remainder.
pub const PANE_PADDING_X: f32 = 2.0;
pub const PANE_PADDING_Y: f32 = 2.0;

const TAB_START_X: f32 = 6.0;
const TAB_GAP: f32 = 4.0;
const TAB_MIN_WIDTH: f32 = 56.0;
const TAB_MAX_WIDTH: f32 = 180.0;
const TAB_TITLE_PADDING: f32 = 28.0;
const APPROXIMATE_TAB_GLYPH_WIDTH: f32 = 8.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    #[must_use]
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.width && y < self.y + self.height
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaneGeometry {
    pub pane_id: PaneId,
    pub rect: Rect,
    pub focused: bool,
}

#[derive(Clone, Debug)]
pub struct TabGeometry {
    pub tab_id: TabId,
    pub title: String,
    pub rect: Rect,
    pub active: bool,
}

#[derive(Clone, Debug, Default)]
pub struct WorkspaceGeometry {
    pub panes: Vec<PaneGeometry>,
    pub tabs: Vec<TabGeometry>,
    pub mode_bar: Option<Rect>,
}

#[must_use]
pub fn calculate(
    session: &Session,
    width: f32,
    height: f32,
    show_mode_bar: bool,
) -> WorkspaceGeometry {
    let mut geometry = WorkspaceGeometry::default();
    let mut tab_x = TAB_START_X;
    for tab in &session.tabs {
        let tab_width = compact_tab_width(&tab.title);
        geometry.tabs.push(TabGeometry {
            tab_id: tab.id,
            title: tab.title.clone(),
            rect: Rect {
                x: tab_x,
                y: 3.0,
                width: tab_width,
                height: TAB_BAR_HEIGHT - 6.0,
            },
            active: tab.id == session.active_tab,
        });
        tab_x += tab_width + TAB_GAP;
    }

    if show_mode_bar {
        geometry.mode_bar = Some(Rect {
            x: 0.0,
            y: (height - MODE_BAR_HEIGHT).max(TAB_BAR_HEIGHT),
            width,
            height: MODE_BAR_HEIGHT,
        });
    }
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

fn compact_tab_width(title: &str) -> f32 {
    (title.chars().count() as f32 * APPROXIMATE_TAB_GLYPH_WIDTH + TAB_TITLE_PADDING)
        .clamp(TAB_MIN_WIDTH, TAB_MAX_WIDTH)
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
    fn tabs_are_compact_but_long_titles_are_bounded() {
        assert!((compact_tab_width("2") - TAB_MIN_WIDTH).abs() < f32::EPSILON);
        assert!(compact_tab_width("build logs") > TAB_MIN_WIDTH);
        assert!((compact_tab_width(&"x".repeat(100)) - TAB_MAX_WIDTH).abs() < f32::EPSILON);
    }

    #[test]
    fn mode_bar_overlays_without_changing_pane_geometry() {
        let pane = PaneId::new();
        let session = Session::with_panes("daily", &[pane]).expect("session");
        let normal = calculate(&session, 800.0, 600.0, false);
        let pane_mode = calculate(&session, 800.0, 600.0, true);

        assert_eq!(pane_mode.panes, normal.panes);
        assert!(normal.mode_bar.is_none());
        assert_eq!(
            pane_mode.mode_bar,
            Some(Rect {
                x: 0.0,
                y: 570.0,
                width: 800.0,
                height: MODE_BAR_HEIGHT,
            })
        );
    }
}
