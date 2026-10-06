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

impl PaneGeometry {
    /// The surface the slab holds as drawn. Settled, it is `rect`; while the
    /// slab glides it is the slab's body, which `rect`, at the size the
    /// terminal settles at, rides the corner of.
    #[must_use]
    pub fn surface(&self) -> Rect {
        surface_of(self.frame)
    }
}

/// The gap a split leaves between its two sides, where the split is held to
/// drag it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seam {
    pub axis: SplitAxis,
    /// The gap itself.
    pub rect: Rect,
    /// A pane from each side, which together name the split.
    pub first: PaneId,
    pub second: PaneId,
    /// Where the split's region starts along its axis, and the length its
    /// two sides share once the gap is taken out.
    pub start: f32,
    pub available: f32,
}

impl Seam {
    /// The ratio that puts the middle of the gap under `along`, a point on
    /// the split's axis, in thousandths of the shared length.
    #[must_use]
    pub fn thousandths_at(&self, along: f32) -> u16 {
        let first = along - self.start - PANE_GAP / 2.0;
        let fraction = (first / self.available.max(1.0)).clamp(0.001, 0.999);
        // The clamp keeps the product within 1..=999.
        (fraction * 1_000.0).round() as u16
    }

    /// The point on the split's axis where the gap sits when the split is
    /// even.
    #[must_use]
    pub fn middle(&self) -> f32 {
        self.start + PANE_GAP / 2.0 + self.available / 2.0
    }
}

#[derive(Clone, Debug, Default)]
pub struct WorkspaceGeometry {
    pub panes: Vec<PaneGeometry>,
    pub seams: Vec<Seam>,
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
            layout_panes(&tab.layout, bounds, tab.focused_pane, &mut geometry);
        }
    }
    geometry
}

fn pane_geometry(pane_id: PaneId, frame: Rect, focused: bool) -> PaneGeometry {
    PaneGeometry {
        pane_id,
        frame,
        rect: surface_of(frame),
        focused,
    }
}

/// The terminal surface a slab holds: below its head, inset from its sides.
#[must_use]
pub fn surface_of(frame: Rect) -> Rect {
    Rect {
        x: frame.x + PANE_BODY_INSET_X,
        y: frame.y + PANE_HEAD_HEIGHT + PANE_BODY_INSET_Y,
        width: (frame.width - PANE_BODY_INSET_X * 2.0).max(1.0),
        height: (frame.height - PANE_HEAD_HEIGHT - PANE_BODY_INSET_Y * 2.0).max(1.0),
    }
}

fn layout_panes(
    layout: &PaneLayout,
    bounds: Rect,
    focused_pane: PaneId,
    output: &mut WorkspaceGeometry,
) {
    match layout {
        PaneLayout::Leaf(pane_id) => {
            output
                .panes
                .push(pane_geometry(*pane_id, bounds, *pane_id == focused_pane));
        }
        PaneLayout::Split {
            axis,
            ratio,
            first,
            second,
        } => {
            let ratio = f32::from(ratio.thousandths()) / 1_000.0;
            let (first_rect, second_rect, gap, start, available) = match axis {
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
                        Rect {
                            x: bounds.x + first_width,
                            width: PANE_GAP,
                            ..bounds
                        },
                        bounds.x,
                        available,
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
                        Rect {
                            y: bounds.y + first_height,
                            height: PANE_GAP,
                            ..bounds
                        },
                        bounds.y,
                        available,
                    )
                }
            };
            output.seams.push(Seam {
                axis: *axis,
                rect: gap,
                first: first.first_pane(),
                second: second.first_pane(),
                start,
                available,
            });
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
    fn a_split_leaves_a_seam_that_maps_back_to_its_ratio() {
        let left = PaneId::new();
        let right = PaneId::new();
        let session = Session::with_panes("daily", &[left, right]).expect("session");
        let geometry = calculate(&session, 800.0, 600.0);
        let [seam] = geometry.seams[..] else {
            panic!("one split, one seam");
        };

        let (left_slab, right_slab) = (geometry.panes[0].frame, geometry.panes[1].frame);
        assert_eq!((seam.first, seam.second), (left, right));
        assert_eq!(
            seam.rect,
            Rect {
                x: left_slab.x + left_slab.width,
                width: right_slab.x - (left_slab.x + left_slab.width),
                ..left_slab
            }
        );
        assert_eq!(
            Rect {
                width: PANE_GAP,
                ..seam.rect
            },
            seam.rect
        );
        // Its own middle is the ratio it was laid out at.
        assert_eq!(seam.thousandths_at(seam.rect.x + PANE_GAP / 2.0), 500);
        assert_eq!(seam.thousandths_at(seam.middle()), 500);
        assert_eq!(seam.thousandths_at(seam.start), 1);
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
