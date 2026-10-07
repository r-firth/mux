//! The ground the slabs sit on: gofer's grain, alive.
//!
//! A few large, slow fields of ink are resolved into grain against a fixed
//! tile of blue noise, so as a field moves its dots switch on and off where
//! they are instead of sliding: it reads as dither, not video. Only the cells
//! the slabs leave showing are worked out: the gaps, the margins and the
//! strip.
//!
//! The light says something. It gathers behind the focused pane and goes
//! with focus. It breathes along the edge of a pane busy printing. A pane
//! that rang or sent a notification, or an agent waiting on you, sends rings
//! out through the grain until it is looked at; a tab out of sight rings from
//! its chip. A pane showing its history cools the ground, and a new tab's ink
//! dissolves in from its chip. With reduced motion the ground holds still,
//! and its clock stops while the window is in the background.

use super::*;
use gpui::{AnyElement, Edges, Pixels, canvas, fill};

#[cfg(target_os = "macos")]
#[path = "ground_gpu.rs"]
mod gpu;

/// A 64×64 tile of blue noise: each cell's threshold, every value from 0 to
/// 255 sixteen times, so any density lights evenly spread cells.
static NOISE: &[u8; 4096] = include_bytes!("../assets/blue-noise-64.bin");

/// One step of the grain's clock: finer than any display's frame, so the
/// light moves on every frame the ground is drawn.
const STEP_MS: u64 = 4;
/// The slow fields are worked out afresh this often, in steps: they move too
/// slowly to need more.
const IDLE_STEPS: u64 = 8;
/// How often the grain shimmers, in steps: a dozen times a second.
const SHIMMER_STEPS: u64 = 20;
/// The ground draws no oftener than this, however fast the display: sixty
/// frames a second is smooth, and twice that would cost twice as much.
const FRAME: Duration = Duration::from_millis(14);
/// The tide: swells that cross the ground all the time and carry its light
/// with them, so the grain is never still. How little of the ground's light
/// a trough keeps and how much a crest adds of its own; then how far apart
/// crests are in points and how many pass a point each second, for the swell
/// and for the slower one that crosses it and breaks it up.
const TIDE_TROUGH: f32 = 0.3;
const TIDE_CREST: f32 = 0.1;
const TIDE_WAVE: (f32, f32) = (260.0, 0.27);
const TIDE_CROSS: (f32, f32) = (410.0, 0.12);
/// A wake: one ring of light that runs out through the grain when the tab
/// changes or the window first shows. Like the rest of the ground it only
/// ever shows where the grain does, never over a pane. How long it takes,
/// how far it trails behind its front in points, and how much light it adds
/// just behind the front, where 1 lights every dot.
const WAKE: Duration = Duration::from_millis(700);
const WAKE_TRAIL: f32 = 170.0;
const WAKE_PEAK: f32 = 2.4;
/// How much of the tide reaches the strip, where the words are.
const TIDE_IN_STRIP: f32 = 0.3;
/// The share of dots each step that light a little early, out of 256, and by
/// how much: the grain's shimmer.
const SHIMMER_SHARE: u64 = 22;
const SHIMMER: f32 = 0.07;
/// How long a new ink takes to dissolve across the ground.
const DISSOLVE: Duration = Duration::from_millis(640);
/// How far a dissolving edge frays either side of where it has got, in points.
const FRAY: f32 = 40.0;
/// How long the ground takes to cool when a pane looks into its history, and
/// to warm again after.
const COOLING: Duration = Duration::from_millis(500);
/// The grid the slow fields are worked out on, in points. A cell between its
/// points takes the blend of the four around it.
const FIELD_CELL: f32 = 12.0;
/// A slab's corner radius: cells this close to a corner can show past it.
const CORNER: f32 = 12.0;
/// A cell of grain, in points: two a side, as gofer's is.
const CELL: f32 = 2.0;
/// The warm ink under everything, as red, green and blue.
const GROUND_RGB: [f32; 3] = [12.0, 10.0, 9.0];

/// How far the focus light reaches past its slab, and the brighter rim right
/// at its edge, in points.
const FOCUS_REACH: f32 = 56.0;
const FOCUS_RIM: f32 = 8.0;
/// How far the light under the active tab's chip reaches.
const CHIP_GLOW: f32 = 22.0;
/// How far a busy pane's breath reaches past its edge, and how long one
/// breath takes, in seconds.
const BUSY_REACH: f32 = 22.0;
const BREATH: f32 = 2.8;
/// How long a pane must keep printing to count as busy, and how long after
/// it stops it still breathes, in seconds.
const BUSY_AFTER: f32 = 0.8;
const BUSY_FADE: f32 = 1.6;
/// Output this soon after a key reached the pane is its echo, not work.
const ECHO: Duration = Duration::from_millis(150);
/// A pause this long ends a burst of output.
const BURST_GAP: Duration = Duration::from_secs(1);
/// One pair of rings from something calling, in seconds; how wide a ring
/// is; and how far they spread from a pane on screen and from a chip.
const RING_PERIOD: f32 = 2.4;
const RING_WIDTH: f32 = 12.0;
pub(super) const PANE_REACH: f32 = 300.0;
pub(super) const CHIP_REACH: f32 = 150.0;

/// How much of its ink each level of dot carries over the ground. Calls
/// carry a little more, so their rings stand out of a peach tab's grain.
const LEVELS: [f32; 4] = [0.0, 0.38, 0.58, 0.8];
const CALL_LEVELS: [f32; 4] = [0.0, 0.46, 0.68, 0.9];
/// The thinnest the grain gets, in the tab's own ink.
const FLOOR: f32 = 0.035;

/// A slow field of colour: which ink (0 the tab's own, 1 and 2 its accents),
/// where it sits and how wide, as fractions of the window, how strong, and
/// how far and how fast it drifts.
struct Blob {
    ink: usize,
    x: f32,
    y: f32,
    radius: f32,
    strength: f32,
    drift: (f32, f32),
    pace: f32,
    phase: f32,
}

/// Brightest low and to the left, as gofer's ground is, thinning towards the
/// strip so the tabs stay easy to read; a few motes of the accents.
const BLOBS: [Blob; 8] = [
    Blob {
        ink: 0,
        x: 0.12,
        y: 1.02,
        radius: 0.52,
        strength: 0.34,
        drift: (0.035, 0.025),
        pace: 0.055,
        phase: 0.0,
    },
    Blob {
        ink: 0,
        x: 0.58,
        y: 1.08,
        radius: 0.42,
        strength: 0.24,
        drift: (0.04, 0.02),
        pace: 0.045,
        phase: 2.1,
    },
    Blob {
        ink: 0,
        x: 0.96,
        y: 0.42,
        radius: 0.3,
        strength: 0.1,
        drift: (0.03, 0.04),
        pace: 0.06,
        phase: 4.0,
    },
    Blob {
        ink: 0,
        x: 0.03,
        y: 0.3,
        radius: 0.26,
        strength: 0.08,
        drift: (0.03, 0.03),
        pace: 0.07,
        phase: 1.3,
    },
    Blob {
        ink: 1,
        x: 0.3,
        y: 0.8,
        radius: 0.28,
        strength: 0.045,
        drift: (0.04, 0.03),
        pace: 0.05,
        phase: 3.3,
    },
    Blob {
        ink: 1,
        x: 0.88,
        y: 0.98,
        radius: 0.22,
        strength: 0.035,
        drift: (0.03, 0.02),
        pace: 0.06,
        phase: 0.6,
    },
    Blob {
        ink: 2,
        x: 0.72,
        y: 0.62,
        radius: 0.26,
        strength: 0.04,
        drift: (0.04, 0.03),
        pace: 0.05,
        phase: 5.0,
    },
    Blob {
        ink: 2,
        x: 0.08,
        y: 0.66,
        radius: 0.2,
        strength: 0.03,
        drift: (0.03, 0.02),
        pace: 0.07,
        phase: 2.4,
    },
];

/// The two inks a tab's grain is flecked with besides its own.
const fn accents(ink: Ink) -> [Ink; 2] {
    match ink {
        Ink::Peach => [Ink::Rose, Ink::Gold],
        Ink::Rose => [Ink::Peach, Ink::Teal],
        Ink::Teal => [Ink::Gold, Ink::Peach],
        Ink::Gold => [Ink::Peach, Ink::Rose],
    }
}

/// What can call: a pane, or a tab out of sight.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum Caller {
    Pane(PaneId),
    Tab(TabId),
}

/// Something calling: who, where its rings start, and how far they go.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Call {
    pub(super) caller: Caller,
    pub(super) from: layout::Rect,
    pub(super) reach: f32,
}

/// What the ground is to show this frame.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Scene {
    pub(super) viewport: (f32, f32),
    pub(super) ink: Ink,
    /// Slabs drawn solid: the grain under them is never seen.
    pub(super) slabs: Vec<layout::Rect>,
    /// Where the focus light is: on the focused slab, or on its way to it,
    /// and how much brighter it blooms on the way, from 0 to 1.
    pub(super) focus: Option<layout::Rect>,
    pub(super) bloom: f32,
    /// The active tab, whose change sends a wake across the window.
    pub(super) tab: Option<TabId>,
    /// The active tab's chip.
    pub(super) chip: Option<layout::Rect>,
    /// Panes busy printing, and how busy, from 0 to 1.
    pub(super) busy: Vec<(layout::Rect, f32)>,
    pub(super) calls: Vec<Call>,
    /// Whether the focused pane is showing its history.
    pub(super) past: bool,
    /// Whether the window is in front.
    pub(super) active: bool,
    /// Reduced motion: nothing moves.
    pub(super) still: bool,
}

/// A pane's output over time, to tell a pane at work from one that only
/// echoes what is typed.
#[derive(Clone, Copy, Debug)]
pub(super) struct Activity {
    began: Instant,
    last: Instant,
}

impl Activity {
    pub(super) const fn new(now: Instant) -> Self {
        Self {
            began: now,
            last: now,
        }
    }

    /// Output arrived at `now`, `since_key` after the last key reached the
    /// pane. Echo of typing doesn't count.
    pub(super) fn note(&mut self, now: Instant, since_key: Option<Duration>) {
        if since_key.is_some_and(|since| since < ECHO) {
            return;
        }
        if now.saturating_duration_since(self.last) > BURST_GAP {
            self.began = now;
        }
        self.last = now;
    }

    /// How busy the pane is at `now`, from 0 to 1, in sixteenths: nothing
    /// until it has kept printing a while, then fading once it stops.
    pub(super) fn busy(&self, now: Instant) -> f32 {
        let lasted = self
            .last
            .saturating_duration_since(self.began)
            .as_secs_f32();
        let quiet = now.saturating_duration_since(self.last).as_secs_f32();
        let busy = smooth(BUSY_AFTER, BUSY_AFTER * 2.0, lasted)
            * (1.0 - smooth(BUSY_FADE * 0.25, BUSY_FADE, quiet));
        (busy * 16.0).round() / 16.0
    }

    /// Whether it may still be busy at `now`, so worth keeping.
    pub(super) fn is_recent(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.last).as_secs_f32() < BUSY_FADE
    }
}

fn smooth(from: f32, to: f32, value: f32) -> f32 {
    let t = ((value - from) / (to - from)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A falloff from 1 at `distance` 0 to nothing at `reach` and beyond.
fn fall(distance: f32, reach: f32) -> f32 {
    let near = (1.0 - distance / reach).max(0.0);
    near * near
}

fn center(rect: layout::Rect) -> (f32, f32) {
    (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0)
}

/// Where each ink reads the tile from, so their dots fall independently.
const CORNERS: [(i32, i32); 4] = [(0, 0), (23, 41), (47, 13), (31, 29)];

/// The thresholds along one row of cells, a row of the tile for each ink.
struct Thresholds([&'static [u8]; 4]);

impl Thresholds {
    #[allow(clippy::cast_sign_loss)]
    fn new(row: i32) -> Self {
        Self(CORNERS.map(|(_, dy)| &NOISE[(((row + dy) & 63) << 6) as usize..][..64]))
    }

    /// The threshold the dot of `ink` in `column` lights past, from 0 to 1.
    #[allow(clippy::cast_sign_loss)]
    fn at(&self, column: i32, ink: usize) -> f32 {
        (f32::from(self.raw(column, ink)) + 0.5) / 256.0
    }

    #[allow(clippy::cast_sign_loss)]
    fn raw(&self, column: i32, ink: usize) -> u8 {
        self.0[ink][((column + CORNERS[ink].0) & 63) as usize]
    }
}

/// The level of dot a cell shows for a field `value` strong: its whole part,
/// and one more if its fraction passes the cell's threshold, at most 3.
fn level(value: f32, threshold: f32) -> u16 {
    u16::from(value > threshold)
        + u16::from(value > threshold + 1.0)
        + u16::from(value > threshold + 2.0)
}

fn rgb_of(value: u32) -> [f32; 3] {
    #[allow(clippy::cast_precision_loss)]
    let channel = |shift: u32| ((value >> shift) & 0xff) as f32;
    [channel(16), channel(8), channel(0)]
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn pack(rgb: [f32; 3]) -> u32 {
    let channel = |value: f32| value.round().clamp(0.0, 255.0) as u32;
    (channel(rgb[0]) << 16) | (channel(rgb[1]) << 8) | channel(rgb[2])
}

/// What each level of dot adds to the ground, for the tab's ink, its two
/// accents and calls, at a saturation.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Palette([[[f32; 3]; 4]; 4]);

impl Palette {
    fn new(ink: Ink, saturation: f32) -> Self {
        let [first, second] = accents(ink);
        let inks = [ink.rgb(), first.rgb(), second.rgb(), SIGNAL];
        let mut tones = [[[0.0; 3]; 4]; 4];
        for (channel, value) in inks.into_iter().enumerate() {
            let rgb = rgb_of(value);
            let grey = 0.3 * rgb[0] + 0.59 * rgb[1] + 0.11 * rgb[2];
            let levels = if channel == 3 { CALL_LEVELS } else { LEVELS };
            for (step, strength) in levels.into_iter().enumerate() {
                for c in 0..3 {
                    tones[channel][step][c] = (grey + (rgb[c] - grey) * saturation) * strength;
                }
            }
        }
        Self(tones)
    }

    /// The colour of every mix of levels over the ground, in the order
    /// `Field::cell` numbers them.
    fn shades(&self, into: &mut Vec<Hsla>) {
        for mix in 0..256usize {
            let mut rgb = GROUND_RGB;
            for (ink, tones) in self.0.iter().enumerate() {
                let tone = tones[(mix >> (ink * 2)) & 3];
                for c in 0..3 {
                    rgb[c] += tone[c];
                }
            }
            into.push(gpui::rgb(pack(rgb)).into());
        }
    }
}

/// The slow fields over the window at one moment, on a coarse grid.
struct Ambient {
    columns: usize,
    rows: usize,
    values: Vec<[f32; 3]>,
}

impl Ambient {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    fn new(width: f32, height: f32, time: f32) -> Self {
        let columns = (width / FIELD_CELL).ceil().max(0.0) as usize + 2;
        let rows = (height / FIELD_CELL).ceil().max(0.0) as usize + 2;
        let span = width.max(height).max(1.0);
        let mut values = vec![[0.0; 3]; columns * rows];
        for blob in &BLOBS {
            let x = (blob.x + blob.drift.0 * (time * blob.pace + blob.phase).sin()) * width;
            let y = (blob.y + blob.drift.1 * (time * blob.pace * 0.8 + blob.phase * 1.7).cos())
                * height;
            let radius = blob.radius * span;
            for row in 0..rows {
                let dy = (row as f32 * FIELD_CELL - y) / radius;
                let dy2 = dy * dy;
                if dy2 > 3.3 {
                    continue;
                }
                for column in 0..columns {
                    let dx = (column as f32 * FIELD_CELL - x) / radius;
                    values[row * columns + column][blob.ink] +=
                        blob.strength * (-(dx * dx + dy2) * 2.3).exp();
                }
            }
        }
        Self {
            columns,
            rows,
            values,
        }
    }

    /// The fields along the row at `y` into the first three of `values`,
    /// at points `x(index)` going right.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    fn along(&self, y: f32, x: impl Fn(usize) -> f32, values: &mut [Vec<f32>; 4]) {
        let fy = (y / FIELD_CELL).max(0.0);
        let row = (fy as usize).min(self.rows - 2);
        let ty = (fy - row as f32).min(1.0);
        let top = &self.values[row * self.columns..][..self.columns];
        let bottom = &self.values[(row + 1) * self.columns..][..self.columns];
        let blend = |column: usize| {
            let (above, below) = (top[column], bottom[column]);
            [0, 1, 2].map(|ink| above[ink] + (below[ink] - above[ink]) * ty)
        };
        let mut between = (usize::MAX, [0.0; 3], [0.0; 3]);
        let [own, first, second, _] = values;
        for (index, ((own, first), second)) in own.iter_mut().zip(first).zip(second).enumerate() {
            let fx = (x(index) / FIELD_CELL).max(0.0);
            let column = (fx as usize).min(self.columns - 2);
            let tx = (fx - column as f32).min(1.0);
            if between.0 != column {
                between = (column, blend(column), blend(column + 1));
            }
            let (_, left, right) = &between;
            [*own, *first, *second] =
                [0, 1, 2].map(|ink| left[ink] + (right[ink] - left[ink]) * tx);
        }
    }
}

/// Something calling as the field sees it: where from, and how far out
/// each of its pair of rings is this step and how bright, or `None` on a
/// still ground.
#[derive(Clone, Copy, Debug)]
struct Ringing {
    from: layout::Rect,
    rings: Option<[(f32, f32); 2]>,
}

/// An ink dissolving away: where the new one spreads from and how far it
/// has got, and the squared distances from there within which every cell
/// has the new ink, and past which every cell still has the old.
#[derive(Clone, Copy, Debug)]
struct Leaving {
    origin: (f32, f32),
    reached: f32,
    inside: f32,
    beyond: f32,
}

impl Leaving {
    fn new(origin: (f32, f32), reached: f32) -> Self {
        Self {
            origin,
            reached,
            inside: if reached > FRAY {
                (reached - FRAY) * (reached - FRAY)
            } else {
                -1.0
            },
            beyond: (reached + FRAY) * (reached + FRAY),
        }
    }

    /// Whether the cell at `x`, `y` with the threshold `threshold` still
    /// shows the old ink. The edge frays by the cell's threshold.
    fn holds(&self, x: f32, y: f32, threshold: f32) -> bool {
        let dx = x - self.origin.0;
        let dy = y - self.origin.1;
        let squared = dx * dx + dy * dy;
        if squared <= self.inside {
            return false;
        }
        squared > self.beyond || squared.sqrt() + (threshold - 0.5) * FRAY * 2.0 > self.reached
    }
}

/// Everything that sets the ground's light for one step, asked about each
/// cell that shows.
pub(super) struct Field {
    cell: f32,
    ambient: Rc<Ambient>,
    /// How strongly the slow fields show in each ink: less in the past.
    gain: [f32; 3],
    focus: Option<(layout::Rect, f32)>,
    chip: Option<layout::Rect>,
    /// Busy panes, and how strongly each breathes out this step.
    busy: Vec<(layout::Rect, f32)>,
    calls: Vec<Ringing>,
    leaving: Option<Leaving>,
    /// The tide's clock, in seconds and in steps; `None` holds it still.
    tide: Option<(f32, u64)>,
    /// A wake passing: where it set out from, how far its front has got,
    /// and how strong it still is.
    wake: Option<((f32, f32), f32, f32)>,
    /// The colour of each mix of levels a cell can show: two bits for each
    /// of the four inks, then one for showing the ink dissolving away.
    shades: Vec<Hsla>,
}

/// One row's working: each ink's field along it, and the mix of levels
/// each cell shows.
#[derive(Default)]
struct Line {
    values: [Vec<f32>; 4],
    mixes: Vec<u16>,
}

impl Field {
    /// Light the cells of `bounds` that no slab in `slabs` hides, and none
    /// inside `inner`, as runs of one colour.
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    fn lay(
        &self,
        bounds: layout::Rect,
        slabs: &[layout::Rect],
        inner: Option<layout::Rect>,
    ) -> Vec<Run> {
        let cell = self.cell;
        let first_column = (bounds.x / cell).floor() as i32;
        let end_column = ((bounds.x + bounds.width) / cell).ceil() as i32;
        let first_row = (bounds.y / cell).floor().max(0.0) as i32;
        let end_row = ((bounds.y + bounds.height) / cell).ceil() as i32;
        let mut runs = Vec::new();
        let mut line = Line::default();
        let mut hidden: Vec<(i32, i32)> = Vec::new();
        for row in first_row..end_row {
            let top = row as f32 * cell;
            let bottom = top + cell;
            hidden.clear();
            for slab in slabs.iter().chain(inner.as_ref()) {
                if top < slab.y || bottom > slab.y + slab.height {
                    continue;
                }
                // A slab's rounded corners leave its corner cells showing.
                let corner = Some(slab) != inner.as_ref()
                    && (top < slab.y + CORNER || bottom > slab.y + slab.height - CORNER);
                let inset = if corner { CORNER } else { 0.0 };
                let from = ((slab.x + inset) / cell).ceil() as i32;
                let to = ((slab.x + slab.width - inset) / cell).floor() as i32;
                if to > from {
                    hidden.push((from, to));
                }
            }
            hidden.sort_unstable();
            let mut start = first_column;
            for &(from, to) in &hidden {
                if from > start {
                    self.light(row, start, from.min(end_column), &mut line, &mut runs);
                }
                start = start.max(to);
            }
            if start < end_column {
                self.light(row, start, end_column, &mut line, &mut runs);
            }
        }
        runs
    }

    /// Light the cells of `row` from column `from` up to `to`.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss
    )]
    fn light(&self, row: i32, from: i32, to: i32, line: &mut Line, runs: &mut Vec<Run>) {
        if to <= from {
            return;
        }
        let cell = self.cell;
        let count = (to - from) as usize;
        let y = (row as f32 + 0.5) * cell;
        let x = |index: usize| (from as f32 + index as f32 + 0.5) * cell;
        for values in &mut line.values {
            values.clear();
            values.resize(count, 0.0);
        }
        self.ambient.along(y, x, &mut line.values);
        for (ink, values) in line.values.iter_mut().take(3).enumerate() {
            let floor = if ink == 0 { FLOOR } else { 0.0 };
            let gain = self.gain[ink];
            for value in values {
                *value = (floor + *value) * gain;
            }
        }
        let [own, _, _, signal] = &mut line.values;
        self.swell(y, from, own);
        self.wash(y, from, own);
        if let Some((rect, strength)) = self.focus {
            self.around(rect, y, (0.0, FOCUS_REACH), from, own, |distance| {
                strength * (0.42 * fall(distance, FOCUS_REACH) + 0.18 * fall(distance, FOCUS_RIM))
            });
        }
        if let Some(chip) = self.chip {
            self.around(chip, y, (0.0, CHIP_GLOW), from, own, |distance| {
                0.3 * fall(distance, CHIP_GLOW)
            });
        }
        for &(rect, strength) in &self.busy {
            self.around(rect, y, (0.0, BUSY_REACH), from, own, |distance| {
                strength * 0.45 * fall(distance, BUSY_REACH)
            });
        }
        for call in &self.calls {
            self.around(call.from, y, (0.0, 20.0), from, signal, |distance| {
                0.3 * fall(distance, 20.0)
            });
            match call.rings {
                Some(rings) => {
                    for (radius, bright) in rings {
                        let band = (radius - RING_WIDTH, radius + RING_WIDTH);
                        self.around(call.from, y, band, from, signal, |distance| {
                            let band = (1.0 - (distance - radius).abs() / RING_WIDTH).max(0.0);
                            bright * band * band
                        });
                    }
                }
                None => self.around(call.from, y, (0.0, 48.0), from, signal, |distance| {
                    0.3 * fall(distance, 48.0)
                }),
            }
        }

        let thresholds = Thresholds::new(row);
        let [own, first, second, signal] = &line.values;
        // Each step a different few dots light a little early.
        let shimmer = |column: i32| match self.tide {
            Some((_, step))
                if (u64::from(thresholds.raw(column, 3)) + step / SHIMMER_STEPS * 29) & 255
                    < SHIMMER_SHARE =>
            {
                SHIMMER
            }
            _ => 0.0,
        };
        line.mixes.clear();
        line.mixes.extend(
            own.iter()
                .zip(first)
                .zip(second)
                .zip(signal)
                .zip(from..)
                .map(|((((&own, &first), &second), &signal), column)| {
                    level(own + shimmer(column), thresholds.at(column, 0))
                        | level(first, thresholds.at(column, 1)) << 2
                        | level(second, thresholds.at(column, 2)) << 4
                        | level(signal, thresholds.at(column, 3)) << 6
                }),
        );
        if let Some(leaving) = self.leaving {
            for (index, mix) in line.mixes.iter_mut().enumerate() {
                let column = from + index as i32;
                if *mix != 0 && leaving.holds(x(index), y, thresholds.at(column, 0)) {
                    *mix |= 1 << 8;
                }
            }
        }

        self.gather(row, from, &line.mixes, runs);
    }

    /// Gather a row's cells, from column `from`, into runs of one colour.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        clippy::cast_precision_loss
    )]
    fn gather(&self, row: i32, from: i32, mixes: &[u16], runs: &mut Vec<Run>) {
        let cell = self.cell;
        let mut index = 0;
        while index < mixes.len() {
            let mix = mixes[index];
            let mut end = index + 1;
            while end < mixes.len() && mixes[end] == mix {
                end += 1;
            }
            if mix != 0 {
                runs.push(Run {
                    x: (from + index as i32) as f32 * cell,
                    y: row as f32 * cell,
                    width: (end - index) as f32 * cell,
                    color: self.shades[usize::from(mix)],
                });
            }
            index = end;
        }
    }

    /// Carry the row's light on the tide: each cell keeps less of it in a
    /// trough and gains on a crest.
    #[allow(clippy::cast_precision_loss)]
    fn swell(&self, y: f32, from: i32, values: &mut [f32]) {
        let Some((time, _)) = self.tide else {
            return;
        };
        let reach = if y < layout::TAB_BAR_HEIGHT {
            TIDE_IN_STRIP
        } else {
            1.0
        };
        // Each swell's phase grows by the same angle from one cell to the
        // next, so its sine is turned along the row rather than worked out
        // afresh for every cell.
        let turn = std::f32::consts::TAU;
        let x = (from as f32 + 0.5) * self.cell;
        let start = |(across, down): (f32, f32), (apart, pace): (f32, f32)| {
            let phase = ((x * across + y * down) / apart - time * pace) * turn;
            let step = self.cell * across / apart * turn;
            (phase.sin_cos(), step.sin_cos())
        };
        let (mut wave, wave_step) = start((0.86, 0.5), TIDE_WAVE);
        let (mut cross, cross_step) = start((-0.5, 0.86), TIDE_CROSS);
        let turned = |(sin, cos): (f32, f32), (step_sin, step_cos): (f32, f32)| {
            (
                sin * step_cos + cos * step_sin,
                cos * step_cos - sin * step_sin,
            )
        };
        for value in values.iter_mut() {
            let crest = 0.5 + 0.5 * wave.0;
            let swell = crest * crest * (0.6 + 0.4 * cross.0);
            let carried = TIDE_TROUGH + (1.6 - TIDE_TROUGH) * swell;
            *value = *value * (1.0 + (carried - 1.0) * reach) + TIDE_CREST * swell * reach;
            wave = turned(wave, wave_step);
            cross = turned(cross, cross_step);
        }
    }

    /// Add the light of a wake passing to the row at `y`: most just behind
    /// its front, thinning along its trail.
    #[allow(clippy::cast_precision_loss)]
    fn wash(&self, y: f32, from: i32, values: &mut [f32]) {
        let Some(((ox, oy), front, strength)) = self.wake else {
            return;
        };
        let dy = y - oy;
        if dy.abs() > front {
            return;
        }
        for (index, value) in values.iter_mut().enumerate() {
            let x = (from as f32 + index as f32 + 0.5) * self.cell;
            let behind = front - (x - ox).hypot(dy);
            if (0.0..WAKE_TRAIL).contains(&behind) {
                let tail = 1.0 - behind / WAKE_TRAIL;
                *value += strength * tail * tail * smooth(0.0, 8.0, behind);
            }
        }
    }

    /// Add `light` to each of `values`, the cells of the row at `y` from
    /// column `from`, for how far outside `rect` it is. Only the cells
    /// that can lie between `band.0` and `band.1` outside it are worked
    /// out: `light` is nothing past them.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss
    )]
    fn around(
        &self,
        rect: layout::Rect,
        y: f32,
        (inner, outer): (f32, f32),
        from: i32,
        values: &mut [f32],
        light: impl Fn(f32) -> f32,
    ) {
        let dy = (rect.y - y).max(y - rect.y - rect.height).max(0.0);
        if dy >= outer || values.is_empty() {
            return;
        }
        let (left, right) = (rect.x, rect.x + rect.width);
        let count = values.len() as f32;
        // The first cell whose centre is at or past `x`.
        let index = |x: f32| (x / self.cell - 0.5 - from as f32).ceil().clamp(0.0, count) as usize;
        let spread = (outer * outer - dy * dy).sqrt();
        let (first, end) = (index(left - spread), index(right + spread));
        // A band that leaves out the cells nearest the rect skips them.
        let (skip_from, skip_to) = if inner > dy {
            let hollow = (inner * inner - dy * dy).sqrt();
            (index(left - hollow), index(right + hollow))
        } else {
            (end, end)
        };
        let dy2 = dy * dy;
        for (start, stop) in [(first, skip_from), (skip_to, end)] {
            let base = from as f32 + start as f32 + 0.5;
            for (offset, value) in values[start..stop.max(start)].iter_mut().enumerate() {
                let x = (base + offset as f32) * self.cell;
                let dx = (left - x).max(x - right).max(0.0);
                *value += light((dx * dx + dy2).sqrt());
            }
        }
    }
}

/// A run of lit cells along a row, one cell tall.
#[derive(Clone, Copy, Debug)]
struct Run {
    x: f32,
    y: f32,
    width: f32,
    color: Hsla,
}

/// The ground worked out for a frame, ready to paint.
pub(super) struct Grain {
    cell: f32,
    runs: Vec<Run>,
    field: Rc<Field>,
}

/// The grain as a layer under everything in the window.
pub(super) fn paint(grain: &Rc<Grain>) -> AnyElement {
    let grain = Rc::clone(grain);
    canvas(
        |_, _, _| (),
        move |bounds, (), window, _| paint_runs(&grain.runs, grain.cell, bounds, window),
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into_any_element()
}

/// The same ground showing through a gap inside a slab: the cells of the
/// element's bounds, leaving out `inset` from its edges.
pub(super) fn window_onto(grain: &Rc<Grain>, inset: Edges<f32>) -> AnyElement {
    let field = Rc::clone(&grain.field);
    canvas(
        |_, _, _| (),
        move |bounds, (), window, _| {
            let outer = rect_of(bounds);
            let inner = layout::Rect {
                x: outer.x + inset.left,
                y: outer.y + inset.top,
                width: outer.width - inset.left - inset.right,
                height: outer.height - inset.top - inset.bottom,
            };
            let runs = field.lay(outer, &[], Some(inner));
            paint_runs(&runs, field.cell, bounds, window);
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into_any_element()
}

impl Grain {
    #[cfg(test)]
    fn lit(&self) -> impl Iterator<Item = (f32, f32, Hsla)> + '_ {
        self.runs.iter().flat_map(move |run| {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let cells = (run.width / self.cell).round() as usize;
            #[allow(clippy::cast_precision_loss)]
            (0..cells).map(move |index| {
                (
                    run.x + (index as f32 + 0.5) * self.cell,
                    run.y + self.cell / 2.0,
                    run.color,
                )
            })
        })
    }
}

fn rect_of(bounds: Bounds<Pixels>) -> layout::Rect {
    layout::Rect {
        x: f32::from(bounds.origin.x),
        y: f32::from(bounds.origin.y),
        width: f32::from(bounds.size.width),
        height: f32::from(bounds.size.height),
    }
}

/// Paint runs as one layer, clipped to `bounds`: many small quads, ordered
/// together rather than each on its own.
fn paint_runs(runs: &[Run], cell: f32, bounds: Bounds<Pixels>, window: &mut Window) {
    window.paint_layer(bounds, |window| {
        for run in runs {
            window.paint_quad(fill(
                Bounds::new(point(px(run.x), px(run.y)), size(px(run.width), px(cell))),
                run.color,
            ));
        }
    });
}

/// A dissolve from one ink to the next.
/// A pulse of grain on its way across the window.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Wake {
    origin: (f32, f32),
    began: Instant,
}

impl Wake {
    /// How far the wake's front has got `through` of the way across a
    /// window `width` by `height`, and how strong it still is.
    fn reached(&self, width: f32, height: f32, through: f32) -> (f32, f32) {
        let (ox, oy) = self.origin;
        let far = ox.max(width - ox).hypot(oy.max(height - oy));
        let eased = 1.0 - (1.0 - through).powf(2.2);
        (
            eased * (far + WAKE_TRAIL),
            WAKE_PEAK * (1.0 - through.powi(3)),
        )
    }
}

#[derive(Clone, Copy, Debug)]
struct Dissolve {
    from: Ink,
    origin: (f32, f32),
    began: Instant,
}

/// What a laid grain was laid from, to tell when it can be painted again.
#[derive(Clone, Debug, PartialEq)]
struct Laid {
    scene: Scene,
    /// The step the slow fields were drawn at, and the one the rest was.
    drift: u64,
    step: u64,
    past: f32,
    dissolve: Option<(Ink, (f32, f32), f32)>,
    wake: Option<((f32, f32), f32, f32)>,
}

/// The ground's memory between frames.
pub(super) struct Ground {
    epoch: Instant,
    ambient: Option<((f32, f32, u64), Rc<Ambient>)>,
    laid: Option<(Laid, Rc<Grain>)>,
    /// When each caller began calling, which its rings keep time from.
    calls: HashMap<Caller, Instant>,
    ink: Option<Ink>,
    dissolve: Option<Dissolve>,
    /// The tab the ground was last laid for, and the wake its change sent.
    tab: Option<TabId>,
    wake: Option<Wake>,
    /// How cool the ground is, from 0 to 1, and when that was worked out.
    past: (f32, Option<Instant>),
    /// Whether the window is in front and moving things may move.
    running: bool,
}

impl Default for Ground {
    fn default() -> Self {
        Self::new(Instant::now())
    }
}

impl Ground {
    pub(super) fn new(epoch: Instant) -> Self {
        Self {
            epoch,
            ambient: None,
            laid: None,
            calls: HashMap::new(),
            ink: None,
            dissolve: None,
            tab: None,
            wake: None,
            past: (0.0, None),
            running: false,
        }
    }

    #[allow(clippy::cast_possible_truncation)]
    fn step(&self, now: Instant) -> u64 {
        (now.saturating_duration_since(self.epoch).as_millis() / u128::from(STEP_MS)) as u64
    }

    /// When the step `now` falls in began: what the ground is drawn for.
    pub(super) fn step_began(&self, now: Instant) -> Instant {
        self.epoch + Duration::from_millis(self.step(now) * STEP_MS)
    }

    #[allow(clippy::cast_precision_loss)]
    fn seconds(step: u64) -> f32 {
        (step * STEP_MS) as f32 / 1000.0
    }

    /// Whether the ground is moving, and so wants drawing again.
    pub(super) fn moving(&self) -> bool {
        self.dissolve.is_some() || self.wake.is_some() || self.running
    }

    /// Send a wake out when the tab has changed, and put away one that has
    /// crossed.
    fn send_wake(&mut self, scene: &Scene, now: Instant) {
        if let Some(tab) = scene.tab
            && self.tab.replace(tab) != Some(tab)
            && !scene.still
            && scene.active
        {
            // From the chip of the tab arrived at; from the middle of the
            // strip when the window first shows.
            let origin = scene.chip.map_or(
                (scene.viewport.0 / 2.0, layout::TAB_BAR_HEIGHT / 2.0),
                center,
            );
            self.wake = Some(Wake { origin, began: now });
        }
        if self
            .wake
            .is_some_and(|wake| scene.still || now.saturating_duration_since(wake.began) >= WAKE)
        {
            self.wake = None;
        }
    }

    /// The ground for `scene` at `now`: the grain laid last, if nothing it
    /// shows has changed since.
    pub(super) fn lay(&mut self, scene: Scene, now: Instant) -> Rc<Grain> {
        let laid = self.frame(scene, now);
        if let Some((last, grain)) = &self.laid
            && *last == laid
        {
            return Rc::clone(grain);
        }
        let grain = Rc::new(self.grain(&laid, true));
        self.laid = Some((laid, Rc::clone(&grain)));
        grain
    }

    /// Move the ground on to `now` for `scene`: what its light is laid from.
    fn frame(&mut self, scene: Scene, now: Instant) -> Laid {
        let step = self.step(now);
        let still = scene.still;
        self.calls
            .retain(|caller, _| scene.calls.iter().any(|call| call.caller == *caller));
        for call in &scene.calls {
            self.calls.entry(call.caller).or_insert(now);
        }

        self.send_wake(&scene, now);

        if let Some(previous) = self.ink.replace(scene.ink)
            && previous != scene.ink
            && !still
        {
            let origin = scene
                .chip
                .or(scene.focus)
                .map_or((scene.viewport.0 / 2.0, 0.0), center);
            self.dissolve = Some(Dissolve {
                from: previous,
                origin,
                began: now,
            });
        }
        let reach = |origin: (f32, f32)| {
            let (width, height) = scene.viewport;
            let far_x = origin.0.max(width - origin.0);
            let far_y = origin.1.max(height - origin.1);
            far_x.hypot(far_y) + FRAY
        };
        let dissolve = self.dissolve.and_then(|dissolve| {
            let through = now.saturating_duration_since(dissolve.began).as_secs_f32()
                / DISSOLVE.as_secs_f32();
            (through < 1.0).then(|| {
                let eased = 1.0 - (1.0 - through).powi(3);
                // Snapped, so frames a hair apart lay the same grain.
                let reached = (eased * reach(dissolve.origin) / 2.0).round() * 2.0 - FRAY;
                (dissolve.from, dissolve.origin, reached)
            })
        });
        if dissolve.is_none() {
            self.dissolve = None;
        }

        let target = if scene.past { 1.0 } else { 0.0 };
        let (mut past, then) = self.past;
        if still || then.is_none() {
            past = target;
        } else if let Some(then) = then {
            let moved = now.saturating_duration_since(then).as_secs_f32() / COOLING.as_secs_f32();
            past = if target > past {
                (past + moved).min(target)
            } else {
                (past - moved).max(target)
            };
        }
        self.past = (past, Some(now));
        let past = (past * 16.0).round() / 16.0;

        self.running = scene.active && !still;
        let drift = if still {
            0
        } else {
            step / IDLE_STEPS * IDLE_STEPS
        };
        let step = if still { 0 } else { step };
        let wake = self.wake.map(|wake| {
            let through =
                now.saturating_duration_since(wake.began).as_secs_f32() / WAKE.as_secs_f32();
            let (front, strength) =
                wake.reached(scene.viewport.0, scene.viewport.1, through.min(1.0));
            // Snapped, so frames a hair apart lay the same grain.
            (
                wake.origin,
                (front / 2.0).round() * 2.0,
                (strength * 32.0).round() / 32.0,
            )
        });
        Laid {
            scene,
            drift,
            step,
            past,
            dissolve,
            wake,
        }
    }

    /// How strongly busy panes breathe out at `laid`'s step.
    fn breath(laid: &Laid) -> f32 {
        if laid.scene.still {
            0.5
        } else {
            let time = Self::seconds(laid.step);
            0.25 + 0.75 * (0.5 - 0.5 * (time * std::f32::consts::TAU / BREATH).cos())
        }
    }

    /// Each caller in `laid`'s scene and where its rings have got to.
    fn ringing(&self, laid: &Laid) -> Vec<Ringing> {
        let scene = &laid.scene;
        scene
            .calls
            .iter()
            .map(|call| Ringing {
                from: call.from,
                rings: (!scene.still).then(|| {
                    let began = self
                        .calls
                        .get(&call.caller)
                        .map_or(laid.step, |&since| self.step(since));
                    let phase =
                        (Self::seconds(laid.step.saturating_sub(began)) / RING_PERIOD).fract();
                    // Each ring fades as it spreads.
                    [phase, (phase + 0.5).fract()]
                        .map(|ring| (ring * call.reach, 0.9 * (1.0 - ring).powf(1.4)))
                }),
            })
            .collect()
    }

    /// How strongly the slow fields show in each ink: less in the past.
    fn gain(laid: &Laid) -> [f32; 3] {
        [
            1.0 - 0.55 * laid.past,
            1.0 - 0.4 * laid.past,
            1.0 - 0.4 * laid.past,
        ]
    }

    /// Where the focus light is and how strong.
    fn focus(scene: &Scene) -> Option<(layout::Rect, f32)> {
        scene.focus.map(|focus| {
            let rest = if scene.active { 1.0 } else { 0.55 };
            (focus, rest * (1.0 + 0.5 * scene.bloom))
        })
    }

    /// Busy panes, and how strongly each breathes out at `laid`'s step.
    fn busy(laid: &Laid) -> Vec<(layout::Rect, f32)> {
        let breath = Self::breath(laid);
        laid.scene
            .busy
            .iter()
            .map(|&(rect, busy)| (rect, busy * breath))
            .filter(|&(_, strength)| strength > 0.0)
            .collect()
    }

    /// The grain for `laid`: its field, and with `runs` the lit cells of the
    /// whole window too, for drawing here rather than on the GPU.
    fn grain(&mut self, laid: &Laid, runs: bool) -> Grain {
        let scene = &laid.scene;
        let (width, height) = scene.viewport;
        let cell = CELL;
        let key = (width, height, laid.drift);
        let ambient = match &self.ambient {
            Some((cached, ambient)) if *cached == key => Rc::clone(ambient),
            _ => {
                let ambient = Rc::new(Ambient::new(width, height, Self::seconds(laid.drift)));
                self.ambient = Some((key, Rc::clone(&ambient)));
                ambient
            }
        };
        let saturation = 1.0 - 0.4 * laid.past;
        let mut shades = Vec::with_capacity(512);
        Palette::new(scene.ink, saturation).shades(&mut shades);
        if let Some((from, _, _)) = laid.dissolve {
            Palette::new(from, saturation).shades(&mut shades);
        }
        let field = Rc::new(Field {
            cell,
            ambient,
            gain: Self::gain(laid),
            focus: Self::focus(scene),
            chip: scene.chip,
            busy: Self::busy(laid),
            calls: self.ringing(laid),
            leaving: laid
                .dissolve
                .map(|(_, origin, reached)| Leaving::new(origin, reached)),
            tide: (!scene.still).then(|| (Self::seconds(laid.step), laid.step)),
            wake: laid.wake,
            shades,
        });
        let window = layout::Rect {
            x: 0.0,
            y: 0.0,
            width,
            height,
        };
        Grain {
            cell,
            runs: if runs {
                field.lay(window, &scene.slabs, None)
            } else {
                Vec::new()
            },
            field,
        }
    }
}

/// What the window's two views share of the ground: the panes' view says
/// what is on screen, and the ground's own view lays and draws the grain.
#[derive(Default)]
pub(super) struct Shared {
    ground: Ground,
    scene: Option<Scene>,
    grain: Option<Rc<Grain>>,
    /// Whether the panes' view is looking at the scene again while panes
    /// are busy.
    watching: bool,
    /// Whether the panes' view shows the ground through a gap of its own,
    /// and so wants the grain's field even while the GPU draws the ground.
    #[cfg(target_os = "macos")]
    wants_field: bool,
}

pub(super) type SharedGround = Rc<RefCell<Shared>>;

/// The ground as a view of its own, under the panes' view. While it moves
/// it draws itself again every frame; the panes' view, which costs far more
/// to draw, is left as it was.
pub(super) struct GroundView {
    shared: SharedGround,
    drawn: Instant,
    waiting: bool,
    /// The ground's own layer on the GPU, where it can be had, and whether
    /// one has been asked for yet.
    #[cfg(target_os = "macos")]
    gpu: Option<gpu::Gpu>,
    #[cfg(target_os = "macos")]
    gpu_asked: bool,
}

impl GroundView {
    pub(super) fn new(shared: SharedGround, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // The ground stops when the window goes behind, and has to be drawn
        // once to stop, and once to start again.
        cx.observe_window_activation(window, |_, _, cx| cx.notify())
            .detach();
        Self {
            shared,
            drawn: Instant::now(),
            waiting: false,
            #[cfg(target_os = "macos")]
            gpu: None,
            #[cfg(target_os = "macos")]
            gpu_asked: false,
        }
    }

    /// Draw the ground on its GPU layer, if it has one. `None` when it has
    /// not; otherwise whether the ground is moving.
    #[cfg(target_os = "macos")]
    fn draw_on_gpu(&mut self, window: &mut Window) -> Option<bool> {
        if !self.gpu_asked {
            self.gpu_asked = true;
            match gpu::Gpu::attach(window) {
                Ok(gpu) => {
                    // The layer sits under the window's content, and shows
                    // wherever that draws nothing.
                    window.set_background_appearance(gpui::WindowBackgroundAppearance::Transparent);
                    self.gpu = Some(gpu);
                }
                Err(error) => warn!(%error, "the ground stays off the GPU"),
            }
        }
        let gpu = self.gpu.as_mut()?;
        let mut shared = self.shared.borrow_mut();
        let shared = &mut *shared;
        let Some(mut scene) = shared.scene.clone() else {
            return Some(false);
        };
        scene.active = window.is_window_active();
        let (frame, grain) = shared.ground.gpu_frame(
            scene,
            Instant::now(),
            window.scale_factor(),
            shared.wants_field,
        );
        shared.grain = grain;
        gpu.draw(&frame);
        Some(shared.ground.moving())
    }

    /// Draw the GPU's ground again on every frame while it moves. Nothing of
    /// the window is drawn for this: only the layer.
    #[cfg(target_os = "macos")]
    fn follow_frames(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.waiting = true;
        cx.on_next_frame(window, |this, window, cx| {
            this.waiting = false;
            if this.draw_on_gpu(window) == Some(true) {
                this.follow_frames(window, cx);
            }
        });
    }

    /// Draw again on the first frame at least `FRAME` after the last.
    fn await_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.waiting = true;
        cx.on_next_frame(window, |this, window, cx| {
            if this.drawn.elapsed() >= FRAME {
                this.waiting = false;
                cx.notify();
            } else {
                this.await_frame(window, cx);
            }
        });
    }
}

impl Render for GroundView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(target_os = "macos")]
        {
            // While the ground is following the display's frames it has
            // already been drawn for this one. Drawing it here as well would
            // ask the layer for a second buffer in the same frame, and the
            // main thread would wait a whole frame for one to come free:
            // with a pane printing, that left the app stalled almost all of
            // the time. The scene handed over is picked up on the next frame.
            let drawn = if self.waiting && self.gpu.is_some() {
                Some(false)
            } else {
                self.draw_on_gpu(window)
            };
            if let Some(moving) = drawn {
                if moving && !self.waiting {
                    self.follow_frames(window, cx);
                }
                return div().absolute().top_0().left_0().size_full();
            }
        }
        let now = Instant::now();
        let laid = {
            let mut shared = self.shared.borrow_mut();
            let shared = &mut *shared;
            shared.scene.clone().map(|mut scene| {
                scene.active = window.is_window_active();
                let grain = shared.ground.lay(scene, now);
                shared.grain = Some(Rc::clone(&grain));
                (grain, shared.ground.moving())
            })
        };
        self.drawn = now;
        let ground = div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .bg(color(GROUND));
        let Some((grain, moving)) = laid else {
            return ground;
        };
        if moving && !self.waiting {
            self.await_frame(window, cx);
        }
        ground.child(paint(&grain))
    }
}

impl MuxApp {
    /// What the ground shows this frame, given the layout as drawn.
    pub(super) fn ground_scene(
        &self,
        drawn: &motion::DrawnLayout,
        viewport: gpui::Size<Pixels>,
        window: &Window,
        now: Instant,
    ) -> Scene {
        let at = self.ground.borrow().ground.step_began(now);
        let agent_pane = self.active_agent_pane();
        let mut slabs = Vec::new();
        let mut focus = drawn.ring.map(|(frame, _)| frame);
        let bloom = drawn
            .ring
            .map_or(0.0, |(_, bloom)| (bloom * 16.0).round() / 16.0);
        let mut busy = Vec::new();
        let mut calls = Vec::new();
        let mut past = false;
        for pane in &drawn.panes {
            let geometry = pane.geometry;
            let frame = geometry.frame;
            if pane.opacity >= 0.99 {
                slabs.push(frame);
            }
            if geometry.focused && focus.is_none() {
                focus = Some(frame);
            }
            let pane_id = geometry.pane_id;
            let caller = Caller::Pane(pane_id);
            if agent_pane == Some(pane_id) {
                match self.active_agent().map(|agent| agent.status) {
                    Some(
                        AgentSessionStatus::WaitingForPermission
                        | AgentSessionStatus::WaitingForAuthentication,
                    ) => calls.push(Call {
                        caller,
                        from: frame,
                        reach: PANE_REACH,
                    }),
                    Some(AgentSessionStatus::Working | AgentSessionStatus::Starting) => {
                        busy.push((frame, 1.0));
                    }
                    _ => {}
                }
                continue;
            }
            if self.pane_call(pane_id).is_some() {
                calls.push(Call {
                    caller,
                    from: frame,
                    reach: PANE_REACH,
                });
            }
            if let Some(activity) = self.pane_activity.get(&pane_id) {
                let level = activity.busy(at);
                if level > 0.0 {
                    busy.push((frame, level));
                }
            }
            if geometry.focused
                && self
                    .panes
                    .get(&pane_id)
                    .is_some_and(|pane| pane.frame.scroll.is_scrolled())
            {
                past = true;
            }
        }
        let chip = self.ground_chips(&mut calls);
        Scene {
            viewport: (f32::from(viewport.width), f32::from(viewport.height)),
            ink: self.active_ink(),
            slabs,
            focus,
            bloom,
            tab: self.active_tab_id(),
            chip,
            busy,
            calls,
            past,
            active: window.is_window_active(),
            still: self.motion == MotionPreference::Reduced,
        }
    }

    /// Where the active tab's chip is, adding a call from each tab out of
    /// sight that is calling.
    fn ground_chips(&self, calls: &mut Vec<Call>) -> Option<layout::Rect> {
        let session = self.session.as_ref()?;
        let scrolled = self.tab_strip_scroll.offset();
        let mut chip = None;
        for (position, tab) in session.tabs.iter().enumerate() {
            let Some(bounds) = self.tab_strip_scroll.bounds_for_item(position) else {
                continue;
            };
            let mut rect = rect_of(bounds);
            rect.x += f32::from(scrolled.x);
            if tab.id == session.active_tab {
                chip = Some(rect);
                continue;
            }
            let agent_calls = self.agents.iter().any(|agent| {
                agent.tab_id == Some(tab.id)
                    && matches!(
                        agent.status,
                        AgentSessionStatus::WaitingForPermission
                            | AgentSessionStatus::WaitingForAuthentication
                    )
            });
            if agent_calls || self.tab_call(tab).is_some() {
                calls.push(Call {
                    caller: Caller::Tab(tab.id),
                    from: rect,
                    reach: CHIP_REACH,
                });
            }
        }
        chip
    }

    /// Hand the ground this frame's scene. The ground draws itself, in its
    /// own view under this one, so its light can move on every frame without
    /// the panes being drawn again.
    pub(super) fn publish_ground(
        &mut self,
        drawn: &motion::DrawnLayout,
        viewport: gpui::Size<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let now = Instant::now();
        let scene = self.ground_scene(drawn, viewport, window, now);
        self.pane_activity
            .retain(|_, activity| activity.is_recent(now));
        let changed = {
            let mut shared = self.ground.borrow_mut();
            let changed = shared.scene.as_ref() != Some(&scene);
            shared.scene = Some(scene);
            #[cfg(target_os = "macos")]
            {
                shared.wants_field = self.active_agent_pane().is_some();
            }
            self.grain.clone_from(&shared.grain);
            changed
        };
        if changed {
            let ground = self.ground_view.clone();
            cx.defer(move |cx| ground.update(cx, |_, cx| cx.notify()));
        }
        // A pane's busyness fades with time, not with anything happening,
        // so while any is busy the scene is looked at again a few times a
        // second.
        let watching = std::mem::replace(
            &mut self.ground.borrow_mut().watching,
            !self.pane_activity.is_empty(),
        );
        if !self.pane_activity.is_empty() && !watching {
            cx.spawn(async move |entity, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(250))
                        .await;
                    let watching = entity
                        .update(cx, |this, cx| {
                            cx.notify();
                            let watching = !this.pane_activity.is_empty();
                            this.ground.borrow_mut().watching = watching;
                            watching
                        })
                        .unwrap_or(false);
                    if !watching {
                        break;
                    }
                }
            })
            .detach();
        }
    }

    /// Output reached a pane: a sign of work, unless it echoes typing.
    pub(super) fn note_pane_output(&mut self, pane_id: PaneId) {
        let now = Instant::now();
        let since_key = self
            .last_key
            .get()
            .filter(|&(pane, _)| pane == pane_id)
            .map(|(_, at)| now.saturating_duration_since(at));
        self.pane_activity
            .entry(pane_id)
            .or_insert_with(|| Activity::new(now))
            .note(now, since_key);
    }

    /// A key reached a pane, so its next output is likely the echo.
    pub(super) fn note_pane_key(&self, pane_id: PaneId) {
        self.last_key.set(Some((pane_id, Instant::now())));
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

    fn scene() -> Scene {
        Scene {
            viewport: (800.0, 500.0),
            ink: Ink::Peach,
            slabs: vec![
                rect(10.0, 40.0, 385.0, 450.0),
                rect(405.0, 40.0, 385.0, 450.0),
            ],
            focus: Some(rect(10.0, 40.0, 385.0, 450.0)),
            bloom: 0.0,
            tab: None,
            chip: None,
            busy: Vec::new(),
            calls: Vec::new(),
            past: false,
            active: true,
            still: false,
        }
    }

    fn lit(grain: &Grain, area: layout::Rect) -> usize {
        grain
            .lit()
            .filter(|&(x, y, _)| {
                x >= area.x && x < area.x + area.width && y >= area.y && y < area.y + area.height
            })
            .count()
    }

    fn cells(area: layout::Rect) -> f32 {
        area.width * area.height / (CELL * CELL)
    }

    #[allow(clippy::cast_precision_loss)]
    fn density(grain: &Grain, area: layout::Rect) -> f32 {
        lit(grain, area) as f32 / cells(area)
    }

    #[test]
    fn the_noise_tile_holds_every_threshold_equally() {
        let mut counts = [0usize; 256];
        for &value in NOISE {
            counts[usize::from(value)] += 1;
        }
        assert!(counts.iter().all(|&count| count == 16));
    }

    #[test]
    fn nothing_is_worked_out_under_a_slab_but_its_corners() {
        let mut ground = Ground::new(Instant::now());
        let grain = ground.lay(scene(), Instant::now());
        // Well inside the left slab: never lit.
        assert_eq!(lit(&grain, rect(30.0, 60.0, 340.0, 400.0)), 0);
        // Its top-left corner square can show past the rounded corner.
        let corner = grain
            .lit()
            .filter(|&(x, y, _)| (10.0..12.0).contains(&x) && (40.0..52.0).contains(&y))
            .count();
        let edge = grain
            .lit()
            .filter(|&(x, y, _)| (10.0..12.0).contains(&x) && (100.0..112.0).contains(&y))
            .count();
        assert!(corner > 0, "the corner shows");
        assert_eq!(edge, 0, "the slab's edge, past its corner, hides the grain");
    }

    #[test]
    fn the_focused_pane_is_lit_from_behind() {
        let mut ground = Ground::new(Instant::now());
        let grain = ground.lay(scene(), Instant::now());
        // The margin beside the focused slab against the one beside the other.
        let near = density(&grain, rect(0.0, 100.0, 10.0, 300.0));
        let far = density(&grain, rect(790.0, 100.0, 10.0, 300.0));
        assert!(near > far * 2.0, "near {near} far {far}");
        assert!(near > 0.35, "near {near}");
    }

    #[test]
    fn the_light_follows_focus() {
        let mut ground = Ground::new(Instant::now());
        let mut moved = scene();
        moved.focus = Some(rect(405.0, 40.0, 385.0, 450.0));
        let grain = ground.lay(moved, Instant::now());
        let left = density(&grain, rect(0.0, 100.0, 10.0, 300.0));
        let right = density(&grain, rect(790.0, 100.0, 10.0, 300.0));
        assert!(right > left * 2.0, "left {left} right {right}");
    }

    #[test]
    fn dots_switch_where_they_are_as_the_ground_drifts() {
        let epoch = Instant::now();
        let mut ground = Ground::new(epoch);
        let first = ground.lay(scene(), epoch);
        let later = ground.lay(scene(), epoch + Duration::from_secs(20));
        assert!(!Rc::ptr_eq(&first, &later), "the ground drifted");
        for grain in [&first, &later] {
            for run in &grain.runs {
                let column = run.x / grain.cell;
                assert!((column - column.round()).abs() < 1e-3, "on the grid");
            }
        }
    }

    #[test]
    fn the_same_step_paints_the_same_grain() {
        let epoch = Instant::now();
        let mut ground = Ground::new(epoch);
        let first = ground.lay(scene(), epoch + Duration::from_millis(STEP_MS));
        let again = ground.lay(scene(), epoch + Duration::from_millis(STEP_MS * 2 - 1));
        assert!(Rc::ptr_eq(&first, &again));
    }

    #[test]
    fn a_pane_calling_sends_rings_out_through_the_grain() {
        let epoch = Instant::now();
        let mut quiet = scene();
        quiet.focus = None;
        let mut calling = quiet.clone();
        calling.calls = vec![Call {
            caller: Caller::Pane(PaneId::new()),
            from: rect(405.0, 40.0, 385.0, 450.0),
            reach: PANE_REACH,
        }];
        let mut still_ground = Ground::new(epoch);
        let mut ground = Ground::new(epoch);
        let _ = ground.lay(calling.clone(), epoch);
        // The clock keeps whole steps, so 800ms in is 747ms: the first ring
        // is 93 points out, in the strip left of the calling pane's corner.
        let first = epoch + Duration::from_millis(800);
        let ringing = ground.lay(calling.clone(), first);
        let base = still_ground.lay(quiet.clone(), first);
        let near = rect(307.0, 25.0, 10.0, 15.0);
        let further = rect(244.0, 25.0, 10.0, 15.0);
        assert!(density(&ringing, near) > density(&base, near) + 0.1);
        assert!(density(&ringing, further) < density(&base, further) + 0.05);
        // Half a second on it has spread to 156 points.
        let later = first + Duration::from_millis(500);
        let ringing = ground.lay(calling, later);
        let base = still_ground.lay(quiet, later);
        assert!(density(&ringing, further) > density(&base, further) + 0.1);
        // The margin far beyond its reach is untouched.
        let far = rect(0.0, 100.0, 10.0, 300.0);
        assert!((density(&ringing, far) - density(&base, far)).abs() < 0.01);
    }

    #[test]
    fn a_new_ink_dissolves_in_from_the_chip() {
        let epoch = Instant::now();
        let mut ground = Ground::new(epoch);
        let mut first = scene();
        first.chip = Some(rect(80.0, 6.0, 60.0, 24.0));
        let _ = ground.lay(first.clone(), epoch);
        let mut next = first;
        next.ink = Ink::Teal;
        let _ = ground.lay(next.clone(), epoch + Duration::from_millis(1));
        let grain = ground.lay(next.clone(), epoch + Duration::from_millis(150));
        let teal: Hsla = gpui::rgb(pack({
            let tone = Palette::new(Ink::Teal, 1.0).0[0][1];
            [
                GROUND_RGB[0] + tone[0],
                GROUND_RGB[1] + tone[1],
                GROUND_RGB[2] + tone[2],
            ]
        }))
        .into();
        let teal_near = grain
            .lit()
            .filter(|&(x, y, color)| x < 200.0 && y < 40.0 && color == teal)
            .count();
        let teal_far = grain
            .lit()
            .filter(|&(x, y, color)| x > 600.0 && y > 400.0 && color == teal)
            .count();
        assert!(teal_near > 0, "the new ink has reached the chip's corner");
        assert_eq!(teal_far, 0, "and not yet the far corner");
        // Once it has dissolved the ground is the new tab's own.
        let done_at = epoch + Duration::from_secs(2);
        let done = ground.lay(next.clone(), done_at);
        let fresh = Ground::new(epoch).lay(next, done_at);
        assert!(done.lit().eq(fresh.lit()));
    }

    #[test]
    fn the_tide_keeps_a_resting_ground_moving() {
        let mut ground = Ground::new(Instant::now());
        let epoch = ground.epoch;
        let margin = rect(0.0, 300.0, 10.0, 300.0);
        let lit_at = |ground: &mut Ground, seconds: u64| {
            let grain = ground.lay(scene(), epoch + Duration::from_secs(seconds));
            lit(&grain, margin)
        };
        let counts = (0..8)
            .map(|second| lit_at(&mut ground, second))
            .collect::<Vec<_>>();
        let (least, most) = (
            counts.iter().copied().min().unwrap_or_default(),
            counts.iter().copied().max().unwrap_or_default(),
        );
        assert!(
            most as f32 > least as f32 * 1.4,
            "the margin's grain barely changed as the tide passed: {counts:?}"
        );
    }

    #[test]
    fn a_change_of_tab_sends_a_wake_out_from_its_chip() {
        let mut ground = Ground::new(Instant::now());
        let epoch = ground.epoch;
        let chip = rect(90.0, 5.0, 60.0, 26.0);
        let on_tab = |tab| Scene {
            tab: Some(tab),
            chip: Some(chip),
            ..scene()
        };
        let (first, second) = (TabId::new(), TabId::new());
        ground.lay(on_tab(first), epoch);
        // The window's first showing sends one too; let it pass.
        ground.lay(on_tab(first), epoch + WAKE);
        assert!(ground.wake.is_none());

        let switched = epoch + Duration::from_secs(5);
        ground.lay(on_tab(second), switched);
        let wake = ground.wake.expect("a wake for the new tab");
        assert_eq!(wake.origin, center(chip));

        // Early on it lights the strip round the chip; later it has left
        // there and reached the far margin.
        let by_chip = rect(200.0, 0.0, 110.0, 36.0);
        let far_margin = rect(790.0, 200.0, 10.0, 200.0);
        let early = ground.lay(on_tab(second), switched + WAKE.mul_f32(0.1));
        let (chip_early, far_early) = (lit(&early, by_chip), lit(&early, far_margin));
        let late = ground.lay(on_tab(second), switched + WAKE.mul_f32(0.6));
        let (chip_late, far_late) = (lit(&late, by_chip), lit(&late, far_margin));
        assert!(
            chip_early * 2 > chip_late * 3 && far_late > far_early * 2,
            "the wake did not travel: chip {chip_early} then {chip_late}, far {far_early} then {far_late}"
        );
        // It never lights anything under a slab.
        assert_eq!(lit(&early, rect(40.0, 80.0, 300.0, 300.0)), 0);

        ground.lay(on_tab(second), switched + WAKE);
        assert!(ground.wake.is_none(), "the wake outstayed its crossing");
    }

    #[test]
    fn a_still_ground_sends_no_wake() {
        let mut ground = Ground::new(Instant::now());
        let epoch = ground.epoch;
        ground.lay(
            Scene {
                tab: Some(TabId::new()),
                still: true,
                ..scene()
            },
            epoch,
        );
        assert!(ground.wake.is_none());
    }

    #[test]
    fn a_still_ground_never_moves_and_its_clock_stays_stopped() {
        let epoch = Instant::now();
        let mut ground = Ground::new(epoch);
        let mut still = scene();
        still.still = true;
        still.calls = vec![Call {
            caller: Caller::Pane(PaneId::new()),
            from: rect(405.0, 40.0, 385.0, 450.0),
            reach: PANE_REACH,
        }];
        let first = ground.lay(still.clone(), epoch);
        let later = ground.lay(still, epoch + Duration::from_secs(30));
        assert!(Rc::ptr_eq(&first, &later));
        assert!(!ground.moving());
    }

    #[test]
    fn the_clock_stops_while_the_window_is_away() {
        let epoch = Instant::now();
        let mut ground = Ground::new(epoch);
        let mut away = scene();
        away.active = false;
        let _ = ground.lay(away, epoch);
        assert!(!ground.moving());
        let _ = ground.lay(scene(), epoch);
        assert!(ground.moving());
    }

    #[test]
    fn a_pane_is_busy_once_it_keeps_printing_and_not_for_echo() {
        let start = Instant::now();
        let mut activity = Activity::new(start);
        for tick in 0..30 {
            activity.note(start + Duration::from_millis(tick * 60), None);
        }
        let at = start + Duration::from_millis(29 * 60);
        assert!(activity.busy(at) > 0.9);
        assert!(activity.busy(at + Duration::from_secs(3)) < 0.01);

        let mut typing = Activity::new(start);
        for tick in 0..30 {
            typing.note(
                start + Duration::from_millis(tick * 60),
                Some(Duration::from_millis(20)),
            );
        }
        assert!(typing.busy(at) < 0.01);
    }

    /// How long laying the ground takes for a laptop-sized window with three
    /// panes, one busy and one calling. Run by hand:
    /// `cargo test --release -p mux --bin mux ground::tests::timing -- --ignored --nocapture`
    #[test]
    #[ignore = "a timing, run by hand with --release"]
    fn timing() {
        let epoch = Instant::now();
        let mut ground = Ground::new(epoch);
        let left = rect(10.0, 40.0, 740.0, 895.0);
        let top = rect(760.0, 40.0, 742.0, 442.0);
        let bottom = rect(760.0, 492.0, 742.0, 443.0);
        let scene = Scene {
            viewport: (1512.0, 945.0),
            ink: Ink::Peach,
            slabs: vec![left, top, bottom],
            focus: Some(left),
            bloom: 0.0,
            tab: None,
            chip: Some(rect(80.0, 5.0, 70.0, 26.0)),
            busy: vec![(bottom, 1.0)],
            calls: vec![Call {
                caller: Caller::Pane(PaneId::new()),
                from: top,
                reach: PANE_REACH,
            }],
            past: false,
            active: true,
            still: false,
        };
        let steps = 120u32;
        let began = Instant::now();
        let mut runs = 0;
        for step in 0..steps {
            let grain = ground.lay(
                scene.clone(),
                epoch + Duration::from_millis(u64::from(step) * STEP_MS),
            );
            runs += grain.runs.len();
        }
        let each = began.elapsed() / steps;
        eprintln!(
            "ground: {each:?} a step, {} runs a step",
            runs / steps as usize
        );
    }

    #[test]
    fn looking_into_history_cools_the_ground() {
        let epoch = Instant::now();
        let mut ground = Ground::new(epoch);
        let now = ground.lay(scene(), epoch);
        let mut looking = scene();
        looking.past = true;
        let _ = ground.lay(looking.clone(), epoch + Duration::from_millis(1));
        let past = ground.lay(looking, epoch + Duration::from_secs(1));
        let area = rect(0.0, 0.0, 800.0, 40.0);
        assert!(lit(&past, area) < lit(&now, area));
    }
}
