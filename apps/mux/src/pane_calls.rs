//! Panes asking to be noticed.
//!
//! A bell, or a desktop notification (Claude Code and Codex send one when
//! they want you), from a pane out of sight marks it until it is looked at:
//! its tab's dot turns peach, with what it said in the tooltip, and in the
//! tab on screen the pane's own head dot turns peach and its head says what
//! it said. While mux itself is in the background, the first call bounces its
//! Dock icon.

use super::*;
use mux_terminal::Attention;

impl MuxApp {
    /// Gather what panes asked for in the output just applied. Returns
    /// whether a mark changed, which needs drawing even when the pane that
    /// called is out of sight.
    pub(super) fn collect_pane_calls(&mut self, window: &mut Window) -> bool {
        let watched = self
            .terminal_input_pane_id()
            .filter(|_| window.is_window_active());
        let mut fresh = false;
        let mut changed = false;
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

/// How a call reads after a tab's name in its tooltip.
pub(super) fn call_words(call: &Attention) -> &str {
    match call {
        Attention::Bell => "rang",
        Attention::Notification(said) => said,
    }
}
