//! Closed tabs and panes, kept so the last few can come back, as a
//! browser's tabs do. A tab comes back with its name, ink, splits and pane
//! names, a pane beside the pane that took its room; each with the
//! directory its shell was in and what it showed. What ran in them has
//! ended, so each pane gets a fresh shell there, under the text it showed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mux_workspace::{Direction, PaneId, Tab, TabId};

use super::restore::{PANE_TEXT_LINES, last_lines, worth_keeping};
use super::{SessionMutationError, SessionRuntime};
use crate::pane::{CLOSED_PANE_RULE, CLOSED_RULE, PaneError, PaneRuntime};

/// How many closed tabs and panes a session remembers.
const KEPT: usize = 10;

/// A tab or a pane closed, newest last.
pub(super) enum Closed {
    Tab(ClosedTab),
    Pane(ClosedPane),
}

pub(super) struct ClosedTab {
    tab: Tab,
    /// Its place in the strip.
    place: usize,
    cwds: HashMap<PaneId, PathBuf>,
    names: Vec<(PaneId, String)>,
    texts: HashMap<PaneId, String>,
}

pub(super) struct ClosedPane {
    /// The tab it was in.
    tab: TabId,
    /// The pane that took its room, and which side of that pane it was on.
    heir: PaneId,
    side: Direction,
    cwd: Option<PathBuf>,
    name: Option<String>,
    text: Option<String>,
}

impl SessionRuntime {
    fn remember(&self, closed: Closed) {
        let mut kept = self.closed.lock();
        kept.push(closed);
        if kept.len() > KEPT {
            kept.remove(0);
        }
    }

    /// What a pane showed, as much of it as comes back with it.
    fn text_to_keep(&self, pane: PaneId) -> Option<String> {
        let text = self.panes.read().get(&pane)?.screen_text()?;
        let text = worth_keeping(last_lines(&text, PANE_TEXT_LINES));
        (!text.is_empty()).then(|| text.to_owned())
    }

    /// Remember a tab that is about to close. The session's last tab never
    /// closes, so it is not remembered.
    pub(super) fn remember_closed_tab(&self, tab_id: TabId) {
        let model = self.model.read();
        if model.tabs.len() < 2 {
            return;
        }
        let Some(place) = model.tabs.iter().position(|tab| tab.id == tab_id) else {
            return;
        };
        let tab = model.tabs[place].clone();
        let mut panes = Vec::new();
        tab.layout.pane_ids(&mut panes);
        let names = panes
            .iter()
            .filter_map(|pane| Some((*pane, model.pane_name(*pane)?.to_owned())))
            .collect();
        drop(model);
        let cwds = panes
            .iter()
            .filter_map(|pane| Some((*pane, self.cwd_for_pane(*pane)?)))
            .collect();
        let texts = panes
            .iter()
            .filter_map(|pane| Some((*pane, self.text_to_keep(*pane)?)))
            .collect();
        self.remember(Closed::Tab(ClosedTab {
            tab,
            place,
            cwds,
            names,
            texts,
        }));
    }

    /// Remember the active tab's focused pane, about to close. A tab's
    /// only pane closes with its tab, so it is not remembered here.
    pub(super) fn remember_closed_pane(&self) {
        let model = self.model.read();
        let Some(tab) = model.active_tab() else {
            return;
        };
        let pane = tab.focused_pane;
        let (Some(heir), Some(side)) = (tab.layout.heir(pane), tab.layout.side_of_heir(pane))
        else {
            return;
        };
        let closed = ClosedPane {
            tab: tab.id,
            heir,
            side,
            cwd: None,
            name: model.pane_name(pane).map(str::to_owned),
            text: None,
        };
        drop(model);
        self.remember(Closed::Pane(ClosedPane {
            cwd: self.cwd_for_pane(pane),
            text: self.text_to_keep(pane),
            ..closed
        }));
    }

    /// Bring back what was closed last, a tab or a pane, and show it.
    pub(super) fn reopen_closed(&self, replay_bytes: usize) -> Result<(), SessionMutationError> {
        let closed = self
            .closed
            .lock()
            .pop()
            .ok_or(SessionMutationError::NothingClosed)?;
        match closed {
            Closed::Tab(closed) => self.reopen_tab(closed, replay_bytes),
            Closed::Pane(closed) => self.reopen_pane(closed, replay_bytes),
        }
    }

    /// A fresh shell for a pane coming back, in its directory (or where
    /// the session starts, if that has gone) under the text it showed and
    /// a rule saying where that ends.
    fn respawn(
        &self,
        pane: PaneId,
        cwd: Option<&Path>,
        text: Option<(&str, &str)>,
        replay_bytes: usize,
    ) -> Result<Arc<PaneRuntime>, PaneError> {
        let session_id = self.model.read().id;
        let cwd = cwd
            .filter(|cwd| cwd.is_dir())
            .unwrap_or(self.spawn.cwd.as_path());
        PaneRuntime::spawn_with_history(
            session_id,
            pane,
            cwd,
            &self.spawn.command,
            self.spawn.size,
            replay_bytes,
            self.events.clone(),
            text,
        )
    }

    /// Bring a closed tab back where it was in the strip.
    fn reopen_tab(
        &self,
        closed: ClosedTab,
        replay_bytes: usize,
    ) -> Result<(), SessionMutationError> {
        let mut old = Vec::new();
        closed.tab.layout.pane_ids(&mut old);
        let fresh = old
            .iter()
            .map(|pane| (*pane, PaneId::new()))
            .collect::<HashMap<_, _>>();
        let mut spawned = Vec::new();
        for pane in &old {
            let cwd = closed.cwds.get(pane).map(PathBuf::as_path);
            let text = closed
                .texts
                .get(pane)
                .map(|text| (text.as_str(), CLOSED_RULE));
            match self.respawn(fresh[pane], cwd, text, replay_bytes) {
                Ok(runtime) => spawned.push((fresh[pane], runtime)),
                Err(error) => {
                    for (_, runtime) in spawned {
                        let _ = runtime.kill();
                    }
                    return Err(error.into());
                }
            }
        }
        let tab = Tab {
            id: TabId::new(),
            layout: closed.tab.layout.renamed(&|pane| fresh[&pane]),
            focused_pane: fresh[&closed.tab.focused_pane],
            zoomed_pane: None,
            ..closed.tab
        };
        let mut model = self.model.write();
        for (pane, name) in closed.names {
            model.pane_names.insert(fresh[&pane], name);
        }
        model.insert_tab(tab, closed.place);
        drop(model);
        self.panes.write().extend(spawned);
        Ok(())
    }

    /// Bring a closed pane back beside the pane that took its room, on the
    /// side it was, with the keys. If that pane has gone too it comes back
    /// beside its tab's focused pane, and if its tab has gone, beside the
    /// focused pane of the tab on screen.
    fn reopen_pane(
        &self,
        closed: ClosedPane,
        replay_bytes: usize,
    ) -> Result<(), SessionMutationError> {
        let pane = PaneId::new();
        let runtime = self.respawn(
            pane,
            closed.cwd.as_deref(),
            closed.text.as_deref().map(|text| (text, CLOSED_PANE_RULE)),
            replay_bytes,
        )?;
        let mut model = self.model.write();
        let active = model.active_tab;
        let place = model
            .tabs
            .iter()
            .position(|tab| tab.id == closed.tab)
            .or_else(|| model.tabs.iter().position(|tab| tab.id == active));
        let Some(tab) = place.map(|place| &mut model.tabs[place]) else {
            drop(model);
            let _ = runtime.kill();
            return Err(SessionMutationError::NoActiveTab);
        };
        let (beside, side) = if tab.layout.contains(closed.heir) {
            (closed.heir, closed.side)
        } else {
            (tab.focused_pane, Direction::Right)
        };
        if let Err(error) = tab.layout.split_beside(beside, pane, side) {
            drop(model);
            let _ = runtime.kill();
            return Err(error.into());
        }
        tab.focused_pane = pane;
        tab.zoomed_pane = None;
        let tab_id = tab.id;
        model.select_tab(tab_id)?;
        if let Some(name) = closed.name {
            model.pane_names.insert(pane, name);
        }
        drop(model);
        self.panes.write().insert(pane, runtime);
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use mux_protocol::{CreateSession, SessionSelector, SpawnCommand};
    use mux_terminal::TerminalSize;
    use mux_workspace::{SplitAxis, WorkspaceCommand};

    use super::super::DaemonState;
    use super::PathBuf;

    #[test]
    fn a_closed_tab_comes_back_with_its_splits_and_names() {
        let state_directory = tempfile::tempdir().expect("state directory");
        let start = tempfile::tempdir().expect("start directory");
        let state = DaemonState::new(state_directory.path().to_path_buf(), 64 * 1024);
        let summary = state
            .create_session(CreateSession {
                name: "work".to_owned(),
                cwd: start.path().to_path_buf(),
                command: SpawnCommand {
                    program: PathBuf::from("/bin/sh"),
                    args: Vec::new(),
                    environment: Vec::new(),
                },
                initial_panes: 1,
                initial_size: TerminalSize::default(),
            })
            .expect("create session");
        let run = |command| {
            state
                .workspace_command(summary.id, command)
                .expect("workspace command");
        };
        run(WorkspaceCommand::NewTab);
        run(WorkspaceCommand::SplitPane(SplitAxis::Horizontal));
        run(WorkspaceCommand::RenameTab("build".to_owned()));
        let runtime = state
            .resolve(&SessionSelector::Id(summary.id))
            .expect("session");
        let focused = runtime.model.read().active_tab().expect("tab").focused_pane;
        run(WorkspaceCommand::RenamePane {
            pane: focused,
            name: "tests".to_owned(),
        });
        run(WorkspaceCommand::CloseTab);
        assert_eq!(runtime.model.read().tabs.len(), 1);
        assert_eq!(runtime.panes.read().len(), 1);

        run(WorkspaceCommand::ReopenClosedTab);
        let model = runtime.model.read().clone();
        let back = model.active_tab().expect("reopened tab");
        assert_eq!((model.tabs.len(), back.title.as_str()), (2, "build"));
        let mut panes = Vec::new();
        back.layout.pane_ids(&mut panes);
        assert_eq!(panes.len(), 2);
        assert!(!panes.contains(&focused), "fresh panes");
        assert_eq!(model.pane_name(back.focused_pane), Some("tests"));
        assert_eq!(runtime.panes.read().len(), 3, "a shell in every pane");
        assert!(
            state
                .workspace_command(summary.id, WorkspaceCommand::ReopenClosedTab)
                .is_err(),
            "nothing left to reopen"
        );
        runtime.kill();
    }

    #[test]
    fn a_closed_pane_comes_back_where_it_was() {
        let state_directory = tempfile::tempdir().expect("state directory");
        let start = tempfile::tempdir().expect("start directory");
        let state = DaemonState::new(state_directory.path().to_path_buf(), 64 * 1024);
        let summary = state
            .create_session(CreateSession {
                name: "work".to_owned(),
                cwd: start.path().to_path_buf(),
                command: SpawnCommand {
                    program: PathBuf::from("/bin/sh"),
                    args: Vec::new(),
                    environment: Vec::new(),
                },
                initial_panes: 1,
                initial_size: TerminalSize::default(),
            })
            .expect("create session");
        let run = |command| {
            state
                .workspace_command(summary.id, command)
                .expect("workspace command");
        };
        let runtime = state
            .resolve(&SessionSelector::Id(summary.id))
            .expect("session");
        let first = runtime.model.read().active_tab().expect("tab").focused_pane;
        run(WorkspaceCommand::SplitPane(SplitAxis::Horizontal));
        let second = runtime.model.read().active_tab().expect("tab").focused_pane;
        run(WorkspaceCommand::RenamePane {
            pane: second,
            name: "logs".to_owned(),
        });
        run(WorkspaceCommand::SetFocusedPane(first));
        run(WorkspaceCommand::ClosePane);
        assert_eq!(runtime.panes.read().len(), 1);

        run(WorkspaceCommand::ReopenClosedTab);
        let model = runtime.model.read().clone();
        let tab = model.active_tab().expect("tab");
        let mut panes = Vec::new();
        tab.layout.pane_ids(&mut panes);
        assert_eq!(panes.len(), 2);
        assert_eq!(
            panes[1], second,
            "back on the left of the pane that took its room"
        );
        assert_eq!(tab.focused_pane, panes[0], "with the keys");
        assert_eq!(model.pane_name(second), Some("logs"));
        assert_eq!(runtime.panes.read().len(), 2);
        runtime.kill();
    }
}
