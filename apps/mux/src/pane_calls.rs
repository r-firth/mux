//! Panes asking to be noticed.
//!
//! A bell, or a desktop notification (Claude Code and Codex send one when
//! they want you), from a pane out of sight marks it until it is looked at:
//! its tab's dot turns peach, with what it said in the tooltip, and in the
//! tab on screen the pane's own head dot turns peach and its head says what
//! it said. While mux itself is in the background, the first call bounces its
//! Dock icon.
//!
//! A long program finishing calls the same way: a build or a test run that
//! ends in a pane out of sight, or while mux is in the background, says it
//! is done and how long it took, or that it failed when the shell's marks
//! say it did.
//!
//! A bell in the pane you are looking at has nothing to mark, so the pane
//! flashes instead, washed for a moment in its tab's ink.

use super::*;
use mux_terminal::Attention;

/// A program that runs at least this long is worth saying has finished.
const LONG_COMMAND_SECONDS: u64 = 10;

/// How long a pane on screen flashes when its bell rings.
const BELL_FLASH: Duration = Duration::from_millis(450);

impl MuxApp {
    /// A program has finished in a pane: one that ran long enough is a call,
    /// gathered with the rest.
    pub(super) fn note_finished_command(&mut self, pane_id: PaneId, program: &str, seconds: u64) {
        let exit_status = self
            .panes
            .get_mut(&pane_id)
            .and_then(|pane| pane.title.take_exit_status());
        if seconds >= LONG_COMMAND_SECONDS {
            let words = finished_words(program, seconds, exit_status);
            self.finished_commands.push((pane_id, words));
        }
    }

    /// Gather what panes asked for in the output just applied. Returns
    /// whether a mark changed, which needs drawing even when the pane that
    /// called is out of sight.
    pub(super) fn collect_pane_calls(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let watched = self
            .terminal_input_pane_id()
            .filter(|_| window.is_window_active());
        let mut fresh = false;
        let mut changed = false;
        let mut rung = None;
        for (pane_id, words) in std::mem::take(&mut self.finished_commands) {
            if Some(pane_id) == watched || !self.panes.contains_key(&pane_id) {
                continue;
            }
            let call = Attention::Notification(words);
            fresh |= !self.pane_calls.contains_key(&pane_id);
            changed |= self.pane_calls.get(&pane_id) != Some(&call);
            self.pane_calls.insert(pane_id, call);
        }
        for (pane_id, pane) in &mut self.panes {
            let Some(call) = pane.title.take_attention() else {
                continue;
            };
            if Some(*pane_id) == watched {
                if call == Attention::Bell {
                    rung = Some(*pane_id);
                }
                continue;
            }
            match (self.pane_calls.get(pane_id), call) {
                // A bell rung after a notification adds nothing to it.
                (Some(Attention::Notification(_)), Attention::Bell) => {}
                (previous, call) => {
                    fresh |= previous.is_none();
                    changed |= previous != Some(&call);
                    self.pane_calls.insert(*pane_id, call);
                }
            }
        }
        if fresh {
            window.request_attention();
        }
        if let Some(pane_id) = rung {
            self.flash_bell(pane_id, cx);
            changed = true;
        }
        changed
    }

    /// A bell in the pane you are looking at flashes it, as a visual bell
    /// does, since there is nothing to mark.
    fn flash_bell(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        self.bell_serial += 1;
        let serial = self.bell_serial;
        self.bell_flash = Some((pane_id, serial));
        cx.spawn(async move |entity, cx| {
            cx.background_executor().timer(BELL_FLASH).await;
            let _ = entity.update(cx, |this, cx| {
                if this.bell_flash.is_some_and(|(_, rung)| rung == serial) {
                    this.bell_flash = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The flash over a pane whose bell just rang: a wash of the tab's ink,
    /// fading, or held still while motion is reduced.
    pub(super) fn bell_flash_overlay(&self, pane_id: PaneId) -> Option<gpui::AnyElement> {
        let (flashing, serial) = self.bell_flash?;
        if flashing != pane_id {
            return None;
        }
        let ink = self.active_ink().color();
        let flash = div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .bg(ink.opacity(0.14));
        if self.motion == MotionPreference::Reduced {
            return Some(flash.into_any_element());
        }
        Some(
            flash
                .with_animation(
                    SharedString::from(format!("bell-flash-{serial}")),
                    Animation::new(BELL_FLASH),
                    |flash, delta| flash.opacity(1.0 - delta),
                )
                .into_any_element(),
        )
    }

    /// The pane with the keys, in a window in front, has been seen.
    pub(super) fn clear_seen_pane_call(&mut self, window: &Window) {
        if window.is_window_active()
            && let Some(pane_id) = self.terminal_input_pane_id()
        {
            self.pane_calls.remove(&pane_id);
        }
    }

    pub(super) fn forget_closed_pane_calls(&mut self) {
        let panes = &self.panes;
        self.pane_calls
            .retain(|pane_id, _| panes.contains_key(pane_id));
    }

    pub(super) fn pane_call(&self, pane_id: PaneId) -> Option<&Attention> {
        self.pane_calls.get(&pane_id)
    }

    /// What a tab's panes have asked for: a notification if any said one,
    /// else a bell.
    pub(super) fn tab_call(&self, tab: &mux_workspace::Tab) -> Option<&Attention> {
        let mut panes = Vec::new();
        tab.layout.pane_ids(&mut panes);
        let calls = panes
            .iter()
            .filter_map(|pane_id| self.pane_calls.get(pane_id))
            .collect::<Vec<_>>();
        calls
            .iter()
            .find(|call| matches!(call, Attention::Notification(_)))
            .or_else(|| calls.first())
            .copied()
    }
}

/// What a long program finishing says: done, or failed with the status the
/// shell gave, or stopped by a signal such as Ctrl+C's.
fn finished_words(program: &str, seconds: u64, exit_status: Option<i32>) -> String {
    let took = duration_words(seconds);
    match exit_status {
        None | Some(0) => format!("{program} done in {took}"),
        Some(130 | 143) => format!("{program} stopped after {took}"),
        Some(status) => format!("{program} failed ({status}) after {took}"),
    }
}

/// A length of time as a tab chip has room for: seconds under a minute,
/// then whole minutes, then hours and minutes.
pub(super) fn duration_words(seconds: u64) -> String {
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m", seconds / 60),
        _ if seconds % 3600 < 60 => format!("{}h", seconds / 3600),
        _ => format!("{}h {}m", seconds / 3600, seconds % 3600 / 60),
    }
}

/// How a call reads after a tab's name in its tooltip.
pub(super) fn call_words(call: &Attention) -> &str {
    match call {
        Attention::Bell => "rang",
        Attention::Notification(said) => said,
    }
}

#[cfg(test)]
mod tests {
    use super::{duration_words, finished_words};

    #[test]
    fn a_finished_program_says_how_it_ended() {
        assert_eq!(finished_words("make", 120, Some(0)), "make done in 2m");
        assert_eq!(finished_words("make", 120, None), "make done in 2m");
        assert_eq!(
            finished_words("make", 42, Some(2)),
            "make failed (2) after 42s"
        );
        assert_eq!(
            finished_words("tail", 60, Some(130)),
            "tail stopped after 1m"
        );
    }

    #[test]
    fn durations_read_in_their_largest_units() {
        assert_eq!(duration_words(42), "42s");
        assert_eq!(duration_words(120), "2m");
        assert_eq!(duration_words(134), "2m");
        assert_eq!(duration_words(3600), "1h");
        assert_eq!(duration_words(3725), "1h 2m");
    }
}
