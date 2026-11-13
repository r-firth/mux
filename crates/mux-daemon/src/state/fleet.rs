//! The coding agents in the daemon's panes. The daemon hears of them from
//! their hooks, which report through `muxctl hook`; from what each pane's
//! program says in its output, with OSC 7501 or a notification; and from
//! the process in front of each pane's shell. It keeps them in one
//! [`Fleet`], tells every client of each change, and writes them down so
//! a daemon that starts again can offer to pick their sessions up.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use mux_fleet::{AgentKind, AgentReport, FleetAgent, FleetEvent, Need, StatusReport, StatusState};
use mux_protocol::{ErrorCode, RemoteError};
use mux_terminal::{Attention, ProgramNeed, ProgramState, ProgramStatus};
use mux_workspace::{PaneId, SessionId};
use tokio::sync::broadcast;
use tracing::warn;

use super::DaemonState;
use crate::pane::{ForegroundChange, PaneRuntime};

const FLEET_FILE: &str = "fleet.json";
/// The most lines of a pane's text a client can ask for.
const MAX_TEXT_LINES: u32 = 2_000;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

impl DaemonState {
    #[must_use]
    pub fn subscribe_fleet_events(&self) -> broadcast::Receiver<FleetEvent> {
        self.fleet_events.subscribe()
    }

    #[must_use]
    pub fn list_fleet(&self) -> Vec<FleetAgent> {
        self.fleet.lock().list()
    }

    /// Take in what an agent's hook said the agent in a pane has done.
    pub fn report_agent(&self, pane_id: PaneId, report: AgentReport) -> Result<(), RemoteError> {
        let (session_id, pane) = self.session_and_pane(pane_id)?;
        // Asked of the system only when the hook did not say.
        let cwd = report
            .cwd
            .clone()
            .or_else(|| pane.current_working_directory())
            .unwrap_or_else(|| PathBuf::from("/"));
        let event = self
            .fleet
            .lock()
            .report(pane_id, session_id, report, &cwd, now_ms());
        self.publish_fleet(event);
        Ok(())
    }

    /// The user has looked at a pane.
    pub fn fleet_seen(&self, pane_id: PaneId) {
        let event = self.fleet.lock().seen(pane_id, now_ms());
        self.publish_fleet(event);
    }

    /// Let go of an ended session kept for a pane. A running agent is not
    /// forgotten: it would only be found again.
    pub fn forget_fleet_agent(&self, pane_id: PaneId) {
        let event = {
            let mut fleet = self.fleet.lock();
            if fleet
                .get(pane_id)
                .is_some_and(|agent| agent.state.is_live())
            {
                None
            } else {
                fleet.forget(pane_id)
            }
        };
        self.publish_fleet(event);
    }

    /// The last `lines` lines a pane shows.
    pub fn pane_text(&self, pane_id: PaneId, lines: u32) -> Result<String, RemoteError> {
        let (_, pane) = self.session_and_pane(pane_id)?;
        let text = pane.screen_text().unwrap_or_default();
        let lines = usize::try_from(lines.clamp(1, MAX_TEXT_LINES)).unwrap_or(usize::MAX);
        Ok(super::restore::last_lines(text.trim_end(), lines).to_owned())
    }

    /// A program has come to the front of a pane, or given the terminal
    /// back: an agent's coming and going.
    pub(super) fn note_foreground(
        &self,
        session_id: SessionId,
        pane: &PaneRuntime,
        change: &ForegroundChange,
    ) {
        let event = match change {
            ForegroundChange::Started(name) => {
                let kind = pane.foreground_identity().map_or_else(
                    || AgentKind::detect(name, None, &[]),
                    |process| {
                        AgentKind::detect(
                            &process.name,
                            process.executable.as_deref(),
                            &process.arguments,
                        )
                    },
                );
                let Some(kind) = kind else {
                    return;
                };
                let cwd = pane
                    .current_working_directory()
                    .unwrap_or_else(|| PathBuf::from("/"));
                self.fleet
                    .lock()
                    .process_started(pane.id, session_id, kind, &cwd, now_ms())
            }
            ForegroundChange::Finished(..) => self.fleet.lock().process_finished(pane.id, now_ms()),
        };
        self.publish_fleet(event);
    }

    /// The user sent a line into a pane.
    pub(super) fn note_submitted(&self, pane_id: PaneId) {
        let event = self.fleet.lock().submitted(pane_id, now_ms());
        self.publish_fleet(event);
    }

    /// Take in what each pane's program has said of itself since the last
    /// look, and forget the agents of panes that have closed.
    pub fn announce_fleet(&self) {
        let panes: Vec<_> = self
            .sessions
            .read()
            .values()
            .flat_map(|session| session.panes.read().values().cloned().collect::<Vec<_>>())
            .collect();
        let live: HashSet<PaneId> = panes.iter().map(|pane| pane.id).collect();
        let mut events = Vec::new();
        {
            let mut fleet = self.fleet.lock();
            for pane in &panes {
                // Taken from every pane, so nothing said before an agent
                // started is laid at its door when it does.
                let (status, attention) = pane.take_said();
                if fleet.get(pane.id).is_none() {
                    continue;
                }
                let now = now_ms();
                if let Some(status) = status.as_ref().and_then(status_report) {
                    events.extend(fleet.status(pane.id, &status, now));
                }
                match attention {
                    Some(Attention::Notification(said)) => {
                        events.extend(fleet.notice(pane.id, &said, now));
                    }
                    Some(Attention::Bell) => events.extend(fleet.notice(pane.id, "", now)),
                    None => {}
                }
            }
            events.extend(fleet.retain_panes(|pane_id| live.contains(&pane_id)));
        }
        for event in events {
            self.publish_fleet(Some(event));
        }
    }

    /// Write the fleet down if it has changed since it was last written.
    pub fn save_fleet(&self) {
        let agents = self.list_fleet();
        let Ok(text) = serde_json::to_string_pretty(&agents) else {
            return;
        };
        let mut saved = self.saved_fleet.lock();
        if *saved == text {
            return;
        }
        match super::restore::write_private(&self.state_dir, FLEET_FILE, &text) {
            Ok(()) => *saved = text,
            Err(error) => warn!(%error, "could not save the fleet"),
        }
    }

    /// Bring back, as sessions to pick up, the agents the daemon before
    /// this one had in panes that have come back.
    pub fn restore_fleet(&self) {
        let Ok(text) = std::fs::read_to_string(self.state_dir.join(FLEET_FILE)) else {
            return;
        };
        let Ok(agents) = serde_json::from_str::<Vec<FleetAgent>>(&text) else {
            warn!("the saved fleet could not be read");
            return;
        };
        let live: HashSet<PaneId> = self
            .sessions
            .read()
            .values()
            .flat_map(|session| session.panes.read().keys().copied().collect::<Vec<_>>())
            .collect();
        self.fleet
            .lock()
            .restore(agents, |pane_id| live.contains(&pane_id));
    }

    fn session_and_pane(
        &self,
        pane_id: PaneId,
    ) -> Result<(SessionId, std::sync::Arc<PaneRuntime>), RemoteError> {
        self.sessions
            .read()
            .values()
            .find_map(|session| {
                let pane = session.panes.read().get(&pane_id).cloned()?;
                Some((session.model.read().id, pane))
            })
            .ok_or_else(|| {
                RemoteError::new(ErrorCode::NotFound, format!("pane not found: {pane_id}"))
            })
    }

    fn publish_fleet(&self, event: Option<FleetEvent>) {
        if let Some(event) = event {
            let _ = self.fleet_events.send(event);
        }
    }
}

/// A program's status as the fleet takes it. Taking back what it said is
/// no state of its own.
fn status_report(status: &ProgramStatus) -> Option<StatusReport> {
    Some(StatusReport {
        state: match status.state {
            ProgramState::Idle => StatusState::Idle,
            ProgramState::Working => StatusState::Working,
            ProgramState::Done => StatusState::Done,
            ProgramState::Blocked => StatusState::Blocked,
            ProgramState::Error => StatusState::Error,
            ProgramState::Clear => return None,
        },
        need: status.need.map(|need| match need {
            ProgramNeed::Permission => Need::Permission,
            ProgramNeed::Question => Need::Question,
            ProgramNeed::Auth => Need::Auth,
        }),
        message: status.message.clone(),
    })
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use mux_fleet::{FleetState, ReportEvent, Signal};
    use mux_protocol::{CreateSession, SessionSelector, SpawnCommand};
    use mux_terminal::TerminalSize;

    use super::*;

    fn daemon_with_pane(
        state_directory: &std::path::Path,
        cwd: &std::path::Path,
    ) -> (DaemonState, PaneId) {
        let daemon = DaemonState::new(state_directory.to_path_buf(), 64 * 1024);
        let summary = daemon
            .create_session(CreateSession {
                name: "work".to_owned(),
                cwd: cwd.to_path_buf(),
                command: SpawnCommand {
                    program: PathBuf::from("/bin/sh"),
                    args: Vec::new(),
                    environment: Vec::new(),
                },
                initial_panes: 1,
                initial_size: TerminalSize::default(),
            })
            .expect("create session");
        let session = daemon
            .resolve(&SessionSelector::Id(summary.id))
            .expect("session");
        let pane_id = session.model.read().tabs[0].focused_pane;
        (daemon, pane_id)
    }

    fn wait_for(daemon: &DaemonState, what: &str, done: impl Fn(&[FleetAgent]) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            daemon.announce_finished_commands();
            daemon.announce_fleet();
            if done(&daemon.list_fleet()) {
                return;
            }
            assert!(Instant::now() < deadline, "never saw {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn report(event: ReportEvent) -> AgentReport {
        AgentReport {
            kind: AgentKind::Claude,
            event,
            agent_session: Some("session-1".to_owned()),
            transcript: None,
            cwd: None,
            at_ms: now_ms(),
        }
    }

    #[test]
    fn a_pane_knows_itself_and_its_agent_is_followed_by_what_it_says() {
        let state_directory = tempfile::tempdir().expect("state directory");
        let start = tempfile::tempdir().expect("start directory");
        let (daemon, pane_id) = daemon_with_pane(state_directory.path(), start.path());
        let mut events = daemon.subscribe_fleet_events();

        // The pane's shell is told which pane it is.
        let named = start.path().join("named");
        daemon
            .write_input(
                pane_id,
                format!("echo $MUX_PANE_ID > '{}'\n", named.display()).as_bytes(),
            )
            .expect("echo");
        let deadline = Instant::now() + Duration::from_secs(5);
        while std::fs::read_to_string(&named)
            .map(|text| text.trim().to_owned())
            .ok()
            != Some(pane_id.to_string())
        {
            assert!(Instant::now() < deadline, "the pane never named itself");
            std::thread::sleep(Duration::from_millis(20));
        }

        daemon
            .report_agent(
                pane_id,
                report(ReportEvent::Prompted {
                    prompt: "fix the flaky test".to_owned(),
                }),
            )
            .expect("report");
        let agents = daemon.list_fleet();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].state, FleetState::Working);
        assert_eq!(agents[0].signal, Signal::Hooks);
        assert_eq!(
            agents[0].cwd.canonicalize().ok(),
            start.path().canonicalize().ok()
        );
        assert!(matches!(
            events.try_recv(),
            Ok(FleetEvent::Changed(agent)) if agent.pane_id == pane_id
        ));

        // What the program says with OSC 7501 is read from its output.
        daemon
            .write_input(
                pane_id,
                b"printf '\\033]7501;state=blocked:kind=permission:msg=QXBwbHk/\\033\\\\'\n",
            )
            .expect("status");
        wait_for(&daemon, "the agent blocked", |agents| {
            agents[0].state == FleetState::Blocked
        });
        let blocked = &daemon.list_fleet()[0];
        assert_eq!(blocked.need, Some(Need::Permission));
        assert_eq!(blocked.message.as_deref(), Some("Apply?"));

        daemon
            .report_agent(pane_id, report(ReportEvent::Finished { message: None }))
            .expect("report");
        assert_eq!(daemon.list_fleet()[0].state, FleetState::Done);
        daemon.fleet_seen(pane_id);
        assert_eq!(daemon.list_fleet()[0].state, FleetState::Idle);

        assert!(
            daemon
                .report_agent(PaneId::new(), report(ReportEvent::SessionEnded))
                .is_err()
        );
        for session in daemon.sessions.write().drain() {
            session.1.kill();
        }
    }

    #[test]
    fn an_agent_is_found_by_its_process_and_kept_for_the_next_daemon_to_resume() {
        let state_directory = tempfile::tempdir().expect("state directory");
        let start = tempfile::tempdir().expect("start directory");
        // A program by an agent's name stands in for the agent.
        let agent = start.path().join("claude");
        std::fs::write(&agent, "#!/bin/sh\nread line\n").expect("agent");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o755))
                .expect("permissions");
        }
        let (daemon, pane_id) = daemon_with_pane(state_directory.path(), start.path());
        daemon.write_input(pane_id, b"./claude\n").expect("run");
        wait_for(&daemon, "the agent", |agents| agents.len() == 1);
        let found = &daemon.list_fleet()[0];
        assert_eq!(found.kind, AgentKind::Claude);
        assert_eq!(found.state, FleetState::Unknown);
        assert_eq!(found.signal, Signal::Process);

        daemon
            .report_agent(pane_id, report(ReportEvent::SessionStarted { title: None }))
            .expect("report");
        daemon.save_fleet();

        // The agent exits: its session is kept to pick up.
        daemon.write_input(pane_id, b"\n").expect("exit");
        wait_for(&daemon, "the agent ended", |agents| {
            agents[0].state == FleetState::Ended
        });
        // A running agent is not forgotten on request; an ended one is.
        assert_eq!(
            daemon.list_fleet()[0].resume_command().as_deref(),
            Some("claude --resume session-1")
        );

        // A daemon that starts again, with the workspace it had, offers it.
        daemon.save_workspace();
        for session in daemon.sessions.write().drain() {
            session.1.kill();
        }
        let after = DaemonState::new(state_directory.path().to_path_buf(), 64 * 1024);
        after.restore_workspace();
        after.restore_fleet();
        let restored = after.list_fleet();
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].pane_id, pane_id);
        assert_eq!(restored[0].state, FleetState::Ended);
        after.forget_fleet_agent(pane_id);
        assert!(after.list_fleet().is_empty());
        for session in after.sessions.write().drain() {
            session.1.kill();
        }
    }
}
