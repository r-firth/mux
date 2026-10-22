//! Closed tabs, kept so the last few can come back, as a browser's do: the
//! tab's name, ink, splits and pane names, and the directory each pane's
//! shell was in. What ran in them has ended, so each pane gets a fresh shell
//! there.

use std::collections::HashMap;
use std::path::PathBuf;

use mux_workspace::{PaneId, Tab, TabId};

use super::{SessionMutationError, SessionRuntime};
use crate::pane::PaneRuntime;

/// How many closed tabs a session remembers.
const KEPT: usize = 10;

pub(super) struct ClosedTab {
    tab: Tab,
    /// Its place in the strip.
    place: usize,
    cwds: HashMap<PaneId, PathBuf>,
    names: Vec<(PaneId, String)>,
}

impl SessionRuntime {
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
        let mut closed = self.closed_tabs.lock();
        closed.push(ClosedTab {
            tab,
            place,
            cwds,
            names,
        });
        if closed.len() > KEPT {
            closed.remove(0);
        }
    }

    /// Bring back the tab closed last, where it was in the strip, with a
    /// fresh shell in each pane's directory, and show it.
    pub(super) fn reopen_closed_tab(
        &self,
        replay_bytes: usize,
    ) -> Result<(), SessionMutationError> {
        let closed = self
            .closed_tabs
            .lock()
            .pop()
            .ok_or(SessionMutationError::NothingClosed)?;
        let session_id = self.model.read().id;
        let mut old = Vec::new();
        closed.tab.layout.pane_ids(&mut old);
        let fresh = old
            .iter()
            .map(|pane| (*pane, PaneId::new()))
            .collect::<HashMap<_, _>>();
        let mut spawned = Vec::new();
        for pane in &old {
            // A directory removed since starts the shell where the session
            // does.
            let cwd = closed
                .cwds
                .get(pane)
                .filter(|cwd| cwd.is_dir())
                .cloned()
                .unwrap_or_else(|| self.spawn.cwd.clone());
            match PaneRuntime::spawn(
                session_id,
                fresh[pane],
                &cwd,
                &self.spawn.command,
                self.spawn.size,
                replay_bytes,
                self.events.clone(),
            ) {
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
}
