//! Typing into every pane of a tab at once, as Zellij's Tab mode `s` does:
//! for the same command on several machines, or a change made in each of a
//! few checkouts. The keys go to each shell as its own modes would have them.

use super::{MuxApp, PaneId, TabId};

impl MuxApp {
    /// Turn typing into every pane of the tab on screen on or off.
    pub(super) fn toggle_sync_tab(&mut self) {
        let Some(tab) = self.active_tab_id() else {
            return;
        };
        if !self.synced_tabs.remove(&tab) {
            self.synced_tabs.insert(tab);
        }
    }

    /// Whether the tab on screen types into all its panes.
    pub(super) fn active_tab_synced(&self) -> bool {
        self.active_tab_id()
            .is_some_and(|tab| self.synced_tabs.contains(&tab))
    }

    /// The panes a key typed into this one goes to: every live terminal of
    /// its tab while the tab is synced, else just this one.
    pub(super) fn key_targets(&self, pane_id: PaneId) -> Vec<PaneId> {
        let Some(tab) = self
            .session
            .as_ref()
            .and_then(|session| session.tabs.iter().find(|tab| tab.layout.contains(pane_id)))
        else {
            return vec![pane_id];
        };
        if !self.synced_tabs.contains(&tab.id) {
            return vec![pane_id];
        }
        let mut panes = Vec::new();
        tab.layout.pane_ids(&mut panes);
        let agent = self.agent_panes.get(&tab.id).copied();
        sync_targets(pane_id, &panes, |pane| {
            Some(pane) == agent || self.pane_has_exited(pane) || !self.panes.contains_key(&pane)
        })
    }

    /// Whether this pane takes keys typed into another, so its head says so.
    pub(super) fn pane_synced(&self, tab: Option<TabId>, pane_id: PaneId) -> bool {
        tab.is_some_and(|tab| self.synced_tabs.contains(&tab))
            && self.active_agent_pane() != Some(pane_id)
            && !self.pane_has_exited(pane_id)
    }
}

/// The typed pane first, then the tab's others that can take keys.
fn sync_targets(typed: PaneId, panes: &[PaneId], skip: impl Fn(PaneId) -> bool) -> Vec<PaneId> {
    std::iter::once(typed)
        .chain(
            panes
                .iter()
                .copied()
                .filter(|&pane| pane != typed && !skip(pane)),
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{PaneId, sync_targets};

    #[test]
    fn keys_reach_the_typed_pane_first_and_skip_the_rest_that_cannot_take_them() {
        let [left, middle, right] = [PaneId::new(), PaneId::new(), PaneId::new()];
        let panes = [left, middle, right];
        assert_eq!(
            sync_targets(middle, &panes, |_| false),
            vec![middle, left, right]
        );
        assert_eq!(
            sync_targets(left, &panes, |pane| pane == right),
            vec![left, middle]
        );
        assert_eq!(sync_targets(left, &panes, |_| true), vec![left]);
    }
}
