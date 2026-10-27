//! How long a program has been running, in its pane's head: once a build or
//! a test run has gone ten seconds, its head says so (`cargo · ~/src · 42s`)
//! and counts up, by the second for its first minute and by the minute
//! after, so a glance says whether it is still worth waiting for.
//!
//! A tab out of sight with a program running in one of its panes breathes
//! its ink dot, as a tab whose agent is at work does, until the program
//! ends. A full-screen program, such as an editor or a pager, is left out,
//! so a tab with one parked in it stays still.

use super::*;

/// A program running at least this long has its time shown.
const SHOWN_FROM: Duration = Duration::from_secs(10);

impl PaneReplica {
    /// How long the program in front has run, once it is worth saying.
    fn running_for(&self) -> Option<Duration> {
        self.untitled_program()?;
        let running = self.program_since?.elapsed();
        (running >= SHOWN_FROM).then_some(running)
    }

    /// The head's words, and how long the program there has run.
    pub(super) fn timed_head_title(&self) -> Option<String> {
        let title = self.head_title()?;
        Some(match self.running_for() {
            Some(running) => format!(
                "{title} · {}",
                pane_calls::duration_words(running.as_secs())
            ),
            None => title,
        })
    }
}

/// How long until a clock that has run `running` reads differently: the
/// next second within its first minute, the next minute after.
fn until_next_reading(running: Duration) -> Duration {
    let seconds = running.as_secs();
    let next = if seconds < 60 {
        seconds + 1
    } else {
        (seconds / 60 + 1) * 60
    };
    Duration::from_secs(next).saturating_sub(running)
}

impl MuxApp {
    /// How long until a pane on screen has a clock that reads differently,
    /// counting the ones about to start.
    fn next_clock_change(&self) -> Option<Duration> {
        let tab = self.session.as_ref()?.active_tab()?;
        let mut pane_ids = Vec::new();
        tab.layout.pane_ids(&mut pane_ids);
        pane_ids
            .iter()
            .filter_map(|pane_id| self.panes.get(pane_id))
            .filter(|pane| pane.untitled_program().is_some())
            .filter_map(|pane| pane.program_since)
            .map(|since| {
                let running = since.elapsed();
                SHOWN_FROM
                    .checked_sub(running)
                    .unwrap_or_else(|| until_next_reading(running))
            })
            .min()
    }

    /// The program running in a tab's pane, the first in its layout that
    /// isn't a full-screen one.
    pub(super) fn tab_running_program(&self, tab: &mux_workspace::Tab) -> Option<&str> {
        let mut pane_ids = Vec::new();
        tab.layout.pane_ids(&mut pane_ids);
        running_program(
            pane_ids
                .iter()
                .filter_map(|pane_id| self.panes.get(pane_id))
                .map(|pane| (pane.program.as_deref(), pane.alternate_screen)),
        )
    }

    /// Whether a tab out of sight has a program running, so its dot needs
    /// the pulse.
    pub(super) fn a_tab_out_of_sight_is_running(&self) -> bool {
        self.session.as_ref().is_some_and(|session| {
            session
                .tabs
                .iter()
                .filter(|tab| tab.id != session.active_tab)
                .any(|tab| self.tab_running_program(tab).is_some())
        })
    }

    /// Keep the clocks in the heads on screen going while any has a program
    /// running under it.
    pub(super) fn ensure_run_clock(&mut self, cx: &mut Context<Self>) {
        if self.run_clock.is_some() || self.next_clock_change().is_none() {
            return;
        }
        self.run_clock = Some(Instant::now());
        cx.spawn(async move |entity, cx| {
            let mut wait = Duration::ZERO;
            loop {
                cx.background_executor().timer(wait).await;
                let next = entity
                    .update(cx, |this, cx| {
                        let next = this.next_clock_change();
                        if next.is_none() {
                            this.run_clock = None;
                        } else if !wait.is_zero() {
                            cx.notify();
                        }
                        next
                    })
                    .ok()
                    .flatten();
                let Some(next) = next else {
                    break;
                };
                // A little past the change, so the clock reads the new
                // value when drawn.
                wait = next + Duration::from_millis(20);
            }
        })
        .detach();
    }
}

/// The first of a tab's panes, given as its program and whether it is on
/// the alternate screen, with a program running that isn't full-screen.
fn running_program<'a>(
    panes: impl IntoIterator<Item = (Option<&'a str>, bool)>,
) -> Option<&'a str> {
    panes
        .into_iter()
        .find_map(|(program, alternate)| program.filter(|_| !alternate))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tab_runs_the_first_program_that_isnt_full_screen() {
        assert_eq!(running_program([]), None);
        assert_eq!(running_program([(None, false)]), None);
        assert_eq!(running_program([(Some("sleep"), false)]), Some("sleep"));
        assert_eq!(running_program([(Some("vim"), true)]), None);
        assert_eq!(
            running_program([(Some("vim"), true), (None, false), (Some("cargo"), false)]),
            Some("cargo")
        );
    }

    #[test]
    fn the_clock_changes_by_the_second_then_by_the_minute() {
        let at = |millis| until_next_reading(Duration::from_millis(millis));
        assert_eq!(at(10_000), Duration::from_secs(1));
        assert_eq!(at(10_400), Duration::from_millis(600));
        assert_eq!(at(59_500), Duration::from_millis(500));
        assert_eq!(at(60_000), Duration::from_mins(1));
        assert_eq!(at(150_000), Duration::from_secs(30));
    }
}
