//! How far along a pane's program says it is.
//!
//! A program that reports its progress with OSC 9;4, as Ghostty
//! shows it, gets a thin bar along the top of its pane: the tab's ink while
//! it goes, peach when it has failed, muted while it waits, and a faint band
//! the full width while it is busy without saying how far. A tab out of
//! sight carries the bar under its chip. A report not renewed for a while
//! goes away, as in Ghostty, so a program that dies part way leaves no bar.

use super::*;
use mux_terminal::Progress;

/// How long a report lasts without being renewed.
const PROGRESS_STALE: Duration = Duration::from_secs(15);
/// How often stale reports are looked for while any are shown.
const PROGRESS_SWEEP: Duration = Duration::from_secs(5);

impl MuxApp {
    /// Take in what panes said about their progress in the output just
    /// applied. Returns whether a bar changed.
    pub(super) fn collect_pane_progress(&mut self, cx: &mut Context<Self>) -> bool {
        let mut changed = false;
        let now = Instant::now();
        for (pane_id, pane) in &mut self.panes {
            match pane.title.take_progress() {
                Some(Progress::Removed) => changed |= self.pane_progress.remove(pane_id).is_some(),
                Some(progress) => {
                    let previous = self.pane_progress.insert(*pane_id, (progress, now));
                    changed |= previous.map(|(previous, _)| previous) != Some(progress);
                }
                None => {}
            }
        }
        self.pane_progress
            .retain(|pane_id, _| self.panes.contains_key(pane_id));
        self.sweep_progress_later(cx);
        changed
    }

    fn sweep_progress_later(&mut self, cx: &mut Context<Self>) {
        if self.progress_sweep.is_some() || self.pane_progress.is_empty() {
            return;
        }
        self.progress_sweep = Some(Instant::now() + PROGRESS_SWEEP);
        cx.spawn(async move |entity, cx| {
            cx.background_executor().timer(PROGRESS_SWEEP).await;
            let _ = entity.update(cx, |this, cx| {
                this.progress_sweep = None;
                let before = this.pane_progress.len();
                this.pane_progress
                    .retain(|_, (_, said)| said.elapsed() < PROGRESS_STALE);
                if this.pane_progress.len() != before {
                    cx.notify();
                }
                this.sweep_progress_later(cx);
            });
        })
        .detach();
    }

    /// The progress of a tab's pane that says it, a failed one first.
    fn tab_progress(&self, tab: &mux_workspace::Tab) -> Option<Progress> {
        let mut panes = Vec::new();
        tab.layout.pane_ids(&mut panes);
        let reports = panes
            .iter()
            .filter_map(|pane_id| self.pane_progress.get(pane_id))
            .map(|(progress, _)| *progress)
            .collect::<Vec<_>>();
        reports
            .iter()
            .find(|progress| matches!(progress, Progress::Error(_)))
            .or_else(|| reports.first())
            .copied()
    }

    /// The bar along the top of a pane's body.
    pub(super) fn render_pane_progress(
        &self,
        geometry: layout::PaneGeometry,
    ) -> Option<gpui::AnyElement> {
        let (progress, _) = self.pane_progress.get(&geometry.pane_id)?;
        let rect = geometry.surface();
        Some(
            progress_bar(*progress, rect.width, self.active_ink())
                .absolute()
                .left(px(rect.x))
                .top(px(rect.y))
                .into_any_element(),
        )
    }

    /// The bar under the chip of a tab out of sight whose pane says how far
    /// along it is.
    pub(super) fn render_chip_progress(
        &self,
        tab: &mux_workspace::Tab,
        active: bool,
    ) -> Option<gpui::AnyElement> {
        if active {
            return None;
        }
        let progress = self.tab_progress(tab)?;
        Some(
            div()
                .absolute()
                .left(px(10.0))
                .right(px(10.0))
                .bottom(px(2.0))
                .h(px(2.0))
                .rounded_full()
                .bg(wash(0.08))
                .child(progress_fill(progress, self.tab_ink(tab.id)))
                .into_any_element(),
        )
    }
}

/// A bar `width` wide showing `progress`.
fn progress_bar(progress: Progress, width: f32, ink: Ink) -> gpui::Div {
    div()
        .w(px(width))
        .h(px(2.0))
        .child(progress_fill(progress, ink))
}

/// The filled part of a bar, as wide as the progress is far along.
fn progress_fill(progress: Progress, ink: Ink) -> gpui::Div {
    let (percent, tone) = match progress {
        Progress::Normal(percent) => (percent, ink.color()),
        Progress::Error(percent) => (percent, color(SIGNAL)),
        Progress::Paused(percent) => (percent, color(MUTED_TEXT)),
        Progress::Indeterminate | Progress::Removed => (100, ink.color().opacity(0.35)),
    };
    div()
        .h_full()
        .w(gpui::relative(f32::from(percent.max(2)) / 100.0))
        .rounded_full()
        .bg(tone)
}
