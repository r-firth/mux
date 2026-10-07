//! Motion in the workspace: slabs glide to where a change in the layout puts
//! them, and the focus ring crosses to the pane focus moves to.
//!
//! A split slides the new pane in from the far edge of the pane it came
//! from, the gap between them opening as it comes. A closed pane fades where
//! it stood while its neighbours grow over it. Zoom grows a pane over the
//! rest, which fade behind it; coming out of zoom shrinks it back as they
//! rise into place. Only the slabs move: each terminal takes its final size
//! at once and rides its slab's corner at that size, so a program redraws
//! once for the change rather than on every frame of the glide.
//!
//! When focus moves and the layout holds still, the light in the ground's
//! grain lifts off the slab that had focus and slides beneath the slabs to
//! the one that has it now, lighting the gaps on the way; that slab takes its
//! ink edge and grows its notch once the light arrives.

use super::*;

/// How long slabs take to reach where a change in the layout puts them.
const GLIDE: Duration = Duration::from_millis(200);
/// How long the focus glow takes to cross to the pane focus moved to.
const CROSSING: Duration = Duration::from_millis(180);
/// How far inside its place a pane coming out from behind a zoomed one
/// starts, in points.
const RISE: f32 = 8.0;

/// A layout as laid out: where a glide is headed, and what the next layout
/// is told apart from.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct LaidOut {
    tab: TabId,
    viewport: (f32, f32),
    zoomed: Option<PaneId>,
    frames: Vec<(PaneId, layout::Rect)>,
}

/// The window's memory of the layout it drew, so a change glides on from
/// wherever the slabs were.
#[derive(Default)]
pub(super) struct LayoutMotion {
    laid_out: Option<LaidOut>,
    /// Where each pane's slab was drawn last frame.
    drawn: Vec<(PaneId, layout::Rect)>,
    glide: Option<Glide>,
    /// What each pane that just closed last showed, to fade out on.
    departed: HashMap<PaneId, Departed>,
    /// The pane drawn with focus, and its tab.
    focused: Option<(TabId, PaneId)>,
    /// The focus glow on its way to the focused pane: where it set off from,
    /// and when.
    crossing: Option<(layout::Rect, Instant)>,
    /// Where the focus glow was drawn last frame.
    ring_drawn: Option<layout::Rect>,
}

/// Slabs on their way to a new layout.
#[derive(Clone, Debug)]
struct Glide {
    began: Instant,
    /// Where each pane in the new layout starts from, and whether it is new
    /// to the screen, and so fades in.
    from: HashMap<PaneId, (layout::Rect, bool)>,
    /// The panes the new layout leaves out, where each stood.
    leaving: Vec<(PaneId, layout::Rect)>,
}

/// What a closed pane last showed.
pub(super) struct Departed {
    title: Option<String>,
    frame: Rc<RenderFrame>,
    cache: Rc<RefCell<TerminalRenderCache>>,
}

/// A pane as this frame draws it.
#[derive(Clone, Copy, Debug)]
pub(super) struct DrawnPane {
    /// Its place in the layout's order, which numbers it.
    pub(super) index: usize,
    /// Its slab where the glide has it, with its grid (`rect`) at the size it
    /// settles at, riding the slab's corner.
    pub(super) geometry: layout::PaneGeometry,
    /// A pane new to the screen fades in.
    pub(super) opacity: f32,
    /// Whether its slab wears the focus ring and glow: the focused pane's
    /// does, once the glow has crossed to it.
    pub(super) ringed: bool,
}

/// A pane the layout no longer has, fading where it stood.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct LeavingPane {
    pane_id: PaneId,
    frame: layout::Rect,
    opacity: f32,
}

/// The layout as this frame draws it.
#[derive(Debug, Default)]
pub(super) struct DrawnLayout {
    /// Panes on their way out, drawn first for the rest to pass over.
    pub(super) leaving: Vec<LeavingPane>,
    /// The layout's panes in the order to draw them: those new to the screen
    /// first, so a pane shrinking out of zoom stays in front of them.
    pub(super) panes: Vec<DrawnPane>,
    /// The focus glow on its own while it crosses beneath the slabs, and how
    /// brightly it blooms on the way, from nothing at either end to 1.
    pub(super) ring: Option<(layout::Rect, f32)>,
    pub(super) gliding: bool,
}

impl LayoutMotion {
    /// Draw `settled`, the layout as `laid_out`, gliding there from wherever
    /// the slabs were when it changed if `may_glide`. A new window size or
    /// another tab is no change to glide through: the slabs are just there.
    fn draw(
        &mut self,
        settled: &layout::WorkspaceGeometry,
        laid_out: Option<LaidOut>,
        may_glide: bool,
        now: Instant,
    ) -> DrawnLayout {
        let relaid = self.laid_out != laid_out;
        if relaid {
            self.glide = match (&self.laid_out, &laid_out) {
                (Some(previous), Some(next))
                    if may_glide
                        && previous.tab == next.tab
                        && previous.viewport == next.viewport =>
                {
                    Some(Glide::toward(previous, next, &self.drawn, now))
                }
                _ => None,
            };
            self.laid_out = laid_out;
        }
        let eased = self
            .glide
            .as_ref()
            .and_then(|glide| eased_at(glide.began, GLIDE, now));
        let glide = if eased.is_some() {
            self.glide.as_ref()
        } else {
            self.glide = None;
            self.departed.clear();
            None
        };
        let mut layout = DrawnLayout::default();
        let mut staying = Vec::new();
        for (index, pane) in settled.panes.iter().enumerate() {
            let start = glide.and_then(|glide| glide.from.get(&pane.pane_id).copied());
            let (frame, opacity, new) = match (start, eased) {
                (Some((from, new)), Some(eased)) => (
                    between(from, pane.frame, eased),
                    if new { eased } else { 1.0 },
                    new,
                ),
                _ => (pane.frame, 1.0, false),
            };
            let body = layout::surface_of(frame);
            let drawn = DrawnPane {
                index,
                geometry: layout::PaneGeometry {
                    frame,
                    rect: layout::Rect {
                        width: pane.rect.width,
                        height: pane.rect.height,
                        ..body
                    },
                    ..*pane
                },
                opacity,
                ringed: false,
            };
            if new {
                layout.panes.push(drawn);
            } else {
                staying.push(drawn);
            }
        }
        layout.panes.extend(staying);
        if let (Some(glide), Some(eased)) = (glide, eased) {
            layout.leaving = glide
                .leaving
                .iter()
                .map(|&(pane_id, frame)| LeavingPane {
                    pane_id,
                    frame,
                    opacity: 1.0 - eased,
                })
                .collect();
        }
        let glide_running = glide.is_some();
        let focused = self.laid_out.as_ref().map(|laid_out| laid_out.tab).zip(
            settled
                .panes
                .iter()
                .find(|pane| pane.focused)
                .map(|pane| pane.pane_id),
        );
        let steady = may_glide && !relaid && !glide_running;
        layout.ring = self.cross(focused, steady, &layout.panes, now);
        for pane in &mut layout.panes {
            pane.ringed = pane.geometry.focused && layout.ring.is_none();
        }
        layout.gliding = glide_running || layout.ring.is_some();
        self.drawn = layout
            .panes
            .iter()
            .map(|pane| (pane.geometry.pane_id, pane.geometry.frame))
            .collect();
        layout
    }

    /// Where the focus glow is drawn on its own this frame, while it crosses
    /// to the pane focus moved to. Focus that moves within a tab whose layout
    /// holds still sends the glow across; any other move puts it straight
    /// under the focused slab.
    fn cross(
        &mut self,
        focused: Option<(TabId, PaneId)>,
        steady: bool,
        panes: &[DrawnPane],
        now: Instant,
    ) -> Option<(layout::Rect, f32)> {
        if focused != self.focused {
            let same_tab = matches!(
                (self.focused, focused),
                (Some((was, _)), Some((tab, _))) if was == tab
            );
            self.crossing = self
                .ring_drawn
                .filter(|_| steady && same_tab)
                .map(|from| (from, now));
            self.focused = focused;
        }
        let target = panes
            .iter()
            .find(|pane| pane.geometry.focused)
            .map(|pane| pane.geometry.frame);
        let ring = match (self.crossing, target) {
            (Some((from, began)), Some(target)) => {
                eased_at(began, CROSSING, now).map(|eased| {
                    // Brightest while it moves fastest, early on.
                    let bloom = (eased * std::f32::consts::PI).sin();
                    (between(from, target, eased), bloom)
                })
            }
            _ => None,
        };
        if ring.is_none() {
            self.crossing = None;
        }
        self.ring_drawn = ring.map(|(frame, _)| frame).or(target);
        ring
    }
}

impl Glide {
    /// A glide from the slabs as `drawn` to the `next` layout.
    fn toward(
        previous: &LaidOut,
        next: &LaidOut,
        drawn: &[(PaneId, layout::Rect)],
        now: Instant,
    ) -> Self {
        let drawn_at = |pane_id: PaneId| {
            drawn
                .iter()
                .find(|(id, _)| *id == pane_id)
                .map(|&(_, frame)| frame)
        };
        let settles_at = |pane_id: PaneId| {
            next.frames
                .iter()
                .find(|(id, _)| *id == pane_id)
                .map(|&(_, frame)| frame)
        };
        let unzooming = previous.zoomed.is_some() && next.zoomed.is_none();
        let from = next
            .frames
            .iter()
            .map(|&(pane_id, place)| {
                let start = if let Some(frame) = drawn_at(pane_id) {
                    (frame, false)
                } else if unzooming {
                    (risen(place), true)
                } else {
                    // A new pane comes out of the pane whose place it takes
                    // the most of, from the side away from it.
                    let source = drawn
                        .iter()
                        .map(|&(id, frame)| (id, overlap(frame, place)))
                        .filter(|&(_, area)| area > 0.0)
                        .max_by(|a, b| a.1.total_cmp(&b.1))
                        .map(|(id, _)| id);
                    let start = source
                        .and_then(settles_at)
                        .map_or_else(|| risen(place), |beside| far_edge(place, beside));
                    (start, true)
                };
                (pane_id, start)
            })
            .collect();
        let leaving = drawn
            .iter()
            .copied()
            .filter(|&(pane_id, _)| settles_at(pane_id).is_none())
            .collect();
        Self {
            began: now,
            from,
            leaving,
        }
    }
}

/// How far along a motion `length` long that began at `began` is at `now`,
/// eased; `None` once it is over.
fn eased_at(began: Instant, length: Duration, now: Instant) -> Option<f32> {
    let progress = now.saturating_duration_since(began).as_secs_f32() / length.as_secs_f32();
    (progress < 1.0).then(|| cubic_bezier(0.16, 1.0, 0.3, 1.0)(progress))
}

fn between(from: layout::Rect, to: layout::Rect, eased: f32) -> layout::Rect {
    let mix = |from: f32, to: f32| from + (to - from) * eased;
    layout::Rect {
        x: mix(from.x, to.x),
        y: mix(from.y, to.y),
        width: mix(from.width, to.width),
        height: mix(from.height, to.height),
    }
}

fn overlap(a: layout::Rect, b: layout::Rect) -> f32 {
    let width = (a.x + a.width).min(b.x + b.width) - a.x.max(b.x);
    let height = (a.y + a.height).min(b.y + b.height) - a.y.max(b.y);
    width.max(0.0) * height.max(0.0)
}

/// Where a new pane starts: folded flat against the edge of its place that
/// is furthest from `beside`, the pane it came out of.
fn far_edge(place: layout::Rect, beside: layout::Rect) -> layout::Rect {
    let (right, bottom) = (place.x + place.width, place.y + place.height);
    if place.x >= beside.x + beside.width {
        layout::Rect {
            x: right,
            width: 0.0,
            ..place
        }
    } else if right <= beside.x {
        layout::Rect {
            width: 0.0,
            ..place
        }
    } else if place.y >= beside.y + beside.height {
        layout::Rect {
            y: bottom,
            height: 0.0,
            ..place
        }
    } else if bottom <= beside.y {
        layout::Rect {
            height: 0.0,
            ..place
        }
    } else {
        risen(place)
    }
}

/// A place drawn in a little, for a pane to rise from.
fn risen(place: layout::Rect) -> layout::Rect {
    layout::Rect {
        x: place.x + RISE,
        y: place.y + RISE,
        width: (place.width - RISE * 2.0).max(0.0),
        height: (place.height - RISE * 2.0).max(0.0),
    }
}

impl MuxApp {
    /// The layout as this frame draws it: as laid out, or on the way there
    /// from wherever the slabs were when it last changed.
    pub(super) fn draw_layout(
        &mut self,
        settled: &layout::WorkspaceGeometry,
        viewport: gpui::Size<gpui::Pixels>,
        window: &Window,
    ) -> DrawnLayout {
        let laid_out = self
            .session
            .as_ref()
            .and_then(Session::active_tab)
            .map(|tab| LaidOut {
                tab: tab.id,
                viewport: (f32::from(viewport.width), f32::from(viewport.height)),
                zoomed: tab.zoomed_pane,
                frames: settled
                    .panes
                    .iter()
                    .map(|pane| (pane.pane_id, pane.frame))
                    .collect(),
            });
        // A dragged or settling seam moves the slabs itself.
        let may_glide = self.motion == MotionPreference::Full
            && !self.seam_dragging()
            && self.seam_settle.is_none();
        let drawn = self
            .layout_motion
            .draw(settled, laid_out, may_glide, Instant::now());
        if drawn.gliding {
            window.request_animation_frame();
        }
        drawn
    }

    /// Keep what each pane about to close last showed, for it to fade out
    /// on. Only panes on screen are worth keeping.
    pub(super) fn remember_departing_panes(&mut self, staying: &[PaneAttachment]) {
        for (&pane_id, pane) in &self.panes {
            let on_screen = self
                .layout_motion
                .drawn
                .iter()
                .any(|&(id, _)| id == pane_id);
            if on_screen && !staying.iter().any(|kept| kept.pane_id == pane_id) {
                self.layout_motion.departed.insert(
                    pane_id,
                    Departed {
                        title: pane
                            .title
                            .title()
                            .map(|title| chrome::terminal_place(title).to_owned()),
                        frame: Rc::clone(&pane.frame),
                        cache: Rc::clone(&pane.render_cache),
                    },
                );
            }
        }
    }

    /// A pane the layout no longer has: its slab, quiet, and what it last
    /// showed, fading where it stood.
    pub(super) fn render_leaving_pane(&self, leaving: LeavingPane) -> Vec<gpui::AnyElement> {
        let (title, shown) = if let Some(pane) = self.panes.get(&leaving.pane_id) {
            (
                pane.title
                    .title()
                    .map(|title| chrome::terminal_place(title).to_owned()),
                Some((Rc::clone(&pane.frame), Rc::clone(&pane.render_cache))),
            )
        } else if let Some(departed) = self.layout_motion.departed.get(&leaving.pane_id) {
            (
                departed.title.clone(),
                Some((Rc::clone(&departed.frame), Rc::clone(&departed.cache))),
            )
        } else {
            (None, None)
        };
        let frame = leaving.frame;
        let head = h_flex()
            .h(px(layout::PANE_HEAD_HEIGHT))
            .flex_none()
            .items_center()
            .gap(px(10.0))
            .px(px(12.0))
            .border_b_1()
            .border_color(hairline(0.07))
            .text_size(px(12.0))
            .child(
                div()
                    .flex_none()
                    .size(px(6.0))
                    .rounded_full()
                    .border_1()
                    .border_color(color(FAINT_TEXT)),
            )
            .child(
                div()
                    .min_w(px(0.0))
                    .truncate()
                    .font_weight(FontWeight::BOLD)
                    .text_color(color(MUTED_TEXT))
                    .child(title.unwrap_or_default()),
            );
        let slab = v_flex()
            .absolute()
            .left(px(frame.x))
            .top(px(frame.y))
            .w(px(frame.width))
            .h(px(frame.height))
            .rounded(px(12.0))
            .bg(color(SURFACE))
            .border_1()
            .border_color(hairline(0.07))
            .overflow_hidden()
            .opacity(leaving.opacity)
            .font_family(EMBEDDED_TERMINAL_FONT)
            .whitespace_nowrap()
            .child(head)
            .into_any_element();
        let surface = shown.map(|(shown, cache)| {
            let body = layout::surface_of(frame);
            div()
                .absolute()
                .left(px(body.x))
                .top(px(body.y))
                .w(px(body.width))
                .h(px(body.height))
                .overflow_hidden()
                .opacity(leaving.opacity)
                .child(gpui_terminal::terminal_canvas(
                    shown,
                    cache,
                    self.terminal_font.clone(),
                    self.metrics,
                    false,
                    gpui_terminal::TerminalChrome {
                        surface: color(SURFACE),
                        cursor: self.active_ink().color(),
                    },
                    Rc::default(),
                ))
                .into_any_element()
        });
        std::iter::once(slab).chain(surface).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, width: f32, height: f32) -> layout::Rect {
        layout::Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn settled(panes: &[(PaneId, layout::Rect)]) -> layout::WorkspaceGeometry {
        focused_on(panes, None)
    }

    fn focused_on(
        panes: &[(PaneId, layout::Rect)],
        focused: Option<PaneId>,
    ) -> layout::WorkspaceGeometry {
        layout::WorkspaceGeometry {
            panes: panes
                .iter()
                .map(|&(pane_id, frame)| layout::PaneGeometry {
                    pane_id,
                    frame,
                    rect: layout::surface_of(frame),
                    focused: Some(pane_id) == focused,
                })
                .collect(),
            seams: Vec::new(),
        }
    }

    fn laid_out(tab: TabId, zoomed: Option<PaneId>, panes: &[(PaneId, layout::Rect)]) -> LaidOut {
        LaidOut {
            tab,
            viewport: (1_000.0, 600.0),
            zoomed,
            frames: panes.to_vec(),
        }
    }

    /// Lay `panes` out at `now` in a fresh motion that last drew `before`.
    fn after(
        before: &[(PaneId, layout::Rect)],
        zoomed_before: Option<PaneId>,
    ) -> (LayoutMotion, TabId, Instant) {
        let (tab, now) = (TabId::new(), Instant::now());
        let mut motion = LayoutMotion::default();
        motion.draw(
            &settled(before),
            Some(laid_out(tab, zoomed_before, before)),
            true,
            now,
        );
        (motion, tab, now)
    }

    fn frame_of(layout: &DrawnLayout, pane_id: PaneId) -> layout::Rect {
        layout
            .panes
            .iter()
            .find(|pane| pane.geometry.pane_id == pane_id)
            .map(|pane| pane.geometry.frame)
            .expect("drawn")
    }

    const WHOLE: layout::Rect = layout::Rect {
        x: 10.0,
        y: 40.0,
        width: 980.0,
        height: 550.0,
    };
    const LEFT: layout::Rect = layout::Rect {
        x: 10.0,
        y: 40.0,
        width: 485.0,
        height: 550.0,
    };
    const RIGHT: layout::Rect = layout::Rect {
        x: 505.0,
        y: 40.0,
        width: 485.0,
        height: 550.0,
    };

    #[test]
    fn a_split_slides_the_new_pane_in_from_the_far_edge() {
        let (old, new) = (PaneId::new(), PaneId::new());
        let (mut motion, tab, now) = after(&[(old, WHOLE)], None);
        let next = [(old, LEFT), (new, RIGHT)];

        let start = motion.draw(&settled(&next), Some(laid_out(tab, None, &next)), true, now);
        assert!(start.gliding);
        assert_eq!(frame_of(&start, old), WHOLE);
        assert_eq!(frame_of(&start, new), rect(990.0, 40.0, 0.0, 550.0));
        assert!(start.panes[0].opacity < 0.01, "the new pane fades in");

        // On the way, the gap opens as the new pane comes in.
        let midway = motion.draw(
            &settled(&next),
            Some(laid_out(tab, None, &next)),
            true,
            now + GLIDE / 4,
        );
        let (shrinking, coming) = (frame_of(&midway, old), frame_of(&midway, new));
        let gap = coming.x - (shrinking.x + shrinking.width);
        assert!(gap > 0.0 && gap < layout::PANE_GAP, "{gap}");
        // The grid rides the slab's corner at the size it settles at.
        let pane = midway
            .panes
            .iter()
            .find(|pane| pane.geometry.pane_id == new);
        let (body, settles) = (layout::surface_of(coming), layout::surface_of(RIGHT));
        assert_eq!(
            pane.expect("drawn").geometry.rect,
            layout::Rect {
                width: settles.width,
                height: settles.height,
                ..body
            }
        );

        let end = motion.draw(
            &settled(&next),
            Some(laid_out(tab, None, &next)),
            true,
            now + GLIDE,
        );
        assert!(!end.gliding);
        assert_eq!((frame_of(&end, old), frame_of(&end, new)), (LEFT, RIGHT));
    }

    #[test]
    fn a_closed_pane_fades_where_it_stood_as_its_neighbour_grows_over_it() {
        let (kept, closed) = (PaneId::new(), PaneId::new());
        let (mut motion, tab, now) = after(&[(kept, LEFT), (closed, RIGHT)], None);
        let next = [(kept, WHOLE)];

        motion.draw(&settled(&next), Some(laid_out(tab, None, &next)), true, now);
        let midway = motion.draw(
            &settled(&next),
            Some(laid_out(tab, None, &next)),
            true,
            now + GLIDE / 4,
        );
        let [leaving] = midway.leaving[..] else {
            panic!("one pane leaving");
        };
        assert_eq!((leaving.pane_id, leaving.frame), (closed, RIGHT));
        assert!(leaving.opacity > 0.0 && leaving.opacity < 1.0);
        let growing = frame_of(&midway, kept);
        assert!(growing.width > LEFT.width && growing.width < WHOLE.width);
    }

    #[test]
    fn zoom_grows_the_pane_over_the_rest_and_back_out_they_rise_behind_it() {
        let (zoomed, other) = (PaneId::new(), PaneId::new());
        let (mut motion, tab, now) = after(&[(zoomed, LEFT), (other, RIGHT)], None);
        let alone = [(zoomed, WHOLE)];
        let zooming = motion.draw(
            &settled(&alone),
            Some(laid_out(tab, Some(zoomed), &alone)),
            true,
            now,
        );
        assert_eq!(frame_of(&zooming, zoomed), LEFT);
        assert_eq!(zooming.leaving[0].pane_id, other);

        let later = now + GLIDE;
        motion.draw(
            &settled(&alone),
            Some(laid_out(tab, Some(zoomed), &alone)),
            true,
            later,
        );
        let both = [(zoomed, LEFT), (other, RIGHT)];
        let unzooming = motion.draw(
            &settled(&both),
            Some(laid_out(tab, None, &both)),
            true,
            later,
        );
        assert_eq!(frame_of(&unzooming, zoomed), WHOLE);
        assert_eq!(frame_of(&unzooming, other), risen(RIGHT));
        // Drawn behind the pane shrinking back in front of them.
        assert_eq!(unzooming.panes[0].geometry.pane_id, other);
        assert_eq!(unzooming.panes[1].geometry.pane_id, zoomed);
    }

    #[test]
    fn another_tab_a_new_window_size_or_a_dragged_seam_is_just_there() {
        let (left, right) = (PaneId::new(), PaneId::new());
        let next = [(left, LEFT), (right, RIGHT)];

        let (mut motion, _, now) = after(&[(left, WHOLE)], None);
        let other_tab = motion.draw(
            &settled(&next),
            Some(laid_out(TabId::new(), None, &next)),
            true,
            now,
        );
        assert!(!other_tab.gliding);

        let (mut motion, tab, now) = after(&[(left, WHOLE)], None);
        let resized = LaidOut {
            viewport: (1_200.0, 600.0),
            ..laid_out(tab, None, &next)
        };
        assert!(
            !motion
                .draw(&settled(&next), Some(resized), true, now)
                .gliding
        );

        let (mut motion, tab, now) = after(&[(left, WHOLE)], None);
        let dragged = motion.draw(
            &settled(&next),
            Some(laid_out(tab, None, &next)),
            false,
            now,
        );
        assert!(!dragged.gliding);
        assert_eq!(frame_of(&dragged, right), RIGHT);
    }

    #[test]
    fn moving_focus_sends_the_ring_across_and_the_slab_wears_it_once_it_lands() {
        let (left, right) = (PaneId::new(), PaneId::new());
        let panes = [(left, LEFT), (right, RIGHT)];
        let (tab, now) = (TabId::new(), Instant::now());
        let mut motion = LayoutMotion::default();
        let mut draw = |focused, at| {
            motion.draw(
                &focused_on(&panes, Some(focused)),
                Some(laid_out(tab, None, &panes)),
                true,
                at,
            )
        };
        let ringed = |layout: &DrawnLayout| {
            layout
                .panes
                .iter()
                .filter(|pane| pane.ringed)
                .map(|pane| pane.geometry.pane_id)
                .collect::<Vec<_>>()
        };

        let still = draw(left, now);
        assert_eq!((still.ring, ringed(&still)), (None, vec![left]));

        let lifted = draw(right, now);
        assert_eq!(
            lifted.ring,
            Some((LEFT, 0.0)),
            "the glow sets off from where it was, unbloomed"
        );
        assert!(lifted.gliding && ringed(&lifted).is_empty());

        let (crossing, bloom) = draw(right, now + CROSSING / 4).ring.expect("crossing");
        assert!(crossing.x > LEFT.x && crossing.x < RIGHT.x, "{crossing:?}");
        assert!(bloom > 0.0);

        let landed = draw(right, now + CROSSING);
        assert_eq!((landed.ring, ringed(&landed)), (None, vec![right]));
        assert!(!landed.gliding);
    }

    #[test]
    fn focus_that_moves_with_the_layout_is_ringed_at_once() {
        let (old, new) = (PaneId::new(), PaneId::new());
        let (tab, now) = (TabId::new(), Instant::now());
        let mut motion = LayoutMotion::default();
        let before = [(old, WHOLE)];
        motion.draw(
            &focused_on(&before, Some(old)),
            Some(laid_out(tab, None, &before)),
            true,
            now,
        );
        let split = [(old, LEFT), (new, RIGHT)];
        let drawn = motion.draw(
            &focused_on(&split, Some(new)),
            Some(laid_out(tab, None, &split)),
            true,
            now,
        );
        assert_eq!(drawn.ring, None);
        assert!(
            drawn
                .panes
                .iter()
                .any(|pane| pane.ringed && pane.geometry.pane_id == new)
        );
    }

    #[test]
    fn a_new_pane_comes_from_the_side_away_from_the_pane_it_split() {
        let beside = rect(10.0, 40.0, 485.0, 270.0);
        let below = rect(10.0, 320.0, 485.0, 270.0);
        assert_eq!(far_edge(below, beside), rect(10.0, 590.0, 485.0, 0.0));
        assert_eq!(far_edge(beside, below), rect(10.0, 40.0, 485.0, 0.0));
        assert_eq!(far_edge(LEFT, RIGHT), rect(10.0, 40.0, 0.0, 550.0));
    }
}
