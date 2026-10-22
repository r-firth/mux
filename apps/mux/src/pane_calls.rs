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
//! is done and how long it took.

use super::*;
use mux_terminal::Attention;

/// A program that runs at least this long is worth saying has finished.
const LONG_COMMAND_SECONDS: u64 = 10;

impl MuxApp {
    /// A program has finished in a pane: one that ran long enough is a call,
    /// gathered with the rest.
    pub(super) fn note_finished_command(&mut self, pane_id: PaneId, program: &str, seconds: u64) {
        if seconds >= LONG_COMMAND_SECONDS {
            let words = format!("{program} done in {}", duration_words(seconds));
            self.finished_commands.push((pane_id, words));
        }
    }

    /// Gather what panes asked for in the output just applied. Returns
    /// whether a mark changed, which needs drawing even when the pane that
    /// called is out of sight.
    pub(super) fn collect_pane_calls(&mut self, window: &mut Window) -> bool {
        let watched = self
            .terminal_input_pane_id()
            .filter(|_| window.is_window_active());
        let mut fresh = false;
        let mut changed = false;
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
        changed
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

/// A length of time as a tab chip has room for: seconds under a minute,
/// then whole minutes, then hours and minutes.
fn duration_words(seconds: u64) -> String {
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
    use super::duration_words;

    #[test]
    fn durations_read_in_their_largest_units() {
        assert_eq!(duration_words(42), "42s");
        assert_eq!(duration_words(120), "2m");
        assert_eq!(duration_words(134), "2m");
        assert_eq!(duration_words(3600), "1h");
        assert_eq!(duration_words(3725), "1h 2m");
    }
}
