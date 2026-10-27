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
//! say it did, once it has run for ten seconds or more. One that ended as it
//! should is only news, as is a program saying it is done (OSC 7501), so it
//! calls in its tab's own ink rather than peach, its dot, words and rings
//! alike. Peach is kept for what wants you, and a bell rung after news turns
//! it peach.
//!
//! A bell in the pane you are looking at has nothing to mark, so the pane
//! flashes instead, washed for a moment in its tab's ink.
//!
//! "Go to the pane that called", in ⌘P, goes to the pane that called last,
//! in whatever tab, or to an agent waiting on you.

use super::*;
use mux_terminal::Attention;

/// How long a program runs before its finishing is worth saying.
const FINISH_NOTICE_AFTER: Duration = Duration::from_secs(10);

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
        if seconds >= FINISH_NOTICE_AFTER.as_secs() {
            let words = finished_words(program, seconds, exit_status);
            self.finished_commands
                .push((pane_id, words, finished_cleanly(exit_status)));
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
        let mut notices = Vec::new();
        let mut called = Vec::new();
        for (pane_id, words, clean) in std::mem::take(&mut self.finished_commands) {
            if Some(pane_id) == watched || !self.panes.contains_key(&pane_id) {
                continue;
            }
            let call = Attention::Notification(words);
            fresh |= !self.pane_calls.contains_key(&pane_id);
            changed |= self.pane_calls.get(&pane_id) != Some(&call);
            changed |= self.news_calls.contains(&pane_id) != clean;
            self.pane_calls.insert(pane_id, call);
            if clean {
                self.news_calls.insert(pane_id);
            } else {
                self.news_calls.remove(&pane_id);
            }
            self.note_call_order(pane_id);
        }
        for (pane_id, pane) in &mut self.panes {
            let Some((call, news)) = pane.title.take_call() else {
                continue;
            };
            if Some(*pane_id) == watched {
                if call == Attention::Bell {
                    rung = Some(*pane_id);
                }
                continue;
            }
            let previous = self.pane_calls.get(pane_id);
            let was_news = self.news_calls.contains(pane_id);
            // A program's own notification goes to Notification Center; a
            // bell under words already there says nothing new.
            if let Attention::Notification(said) = &call {
                notices.push((*pane_id, said.clone()));
            }
            let (call, news) = next_call(previous, call, news);
            fresh |= previous.is_none();
            changed |= previous != Some(&call) || was_news != news;
            self.pane_calls.insert(*pane_id, call);
            if news {
                self.news_calls.insert(*pane_id);
            } else {
                self.news_calls.remove(pane_id);
            }
            called.push(*pane_id);
        }
        for pane_id in called {
            self.note_call_order(pane_id);
        }
        if fresh {
            window.request_attention();
        }
        if !window.is_window_active() {
            for (pane_id, said) in notices {
                let title = self
                    .pane_name(pane_id)
                    .map(ToOwned::to_owned)
                    .or_else(|| self.panes.get(&pane_id)?.head_title())
                    .unwrap_or_else(|| "Mux".to_owned());
                desktop_notice::post(&title, &said);
            }
        }
        if let Some(pane_id) = rung {
            self.flash_bell(pane_id, cx);
            changed = true;
        }
        changed
    }

    /// The pane has just called: it is the one "go to the pane that called" goes to.
    fn note_call_order(&mut self, pane_id: PaneId) {
        self.call_order.retain(|called| *called != pane_id);
        self.call_order.push(pane_id);
    }

    /// Go to the pane that called last and has not been looked at since,
    /// in whatever tab it is; failing that, to an agent in another tab
    /// waiting for you to allow something or sign in.
    pub(super) fn go_to_call(&mut self, cx: &mut Context<Self>) {
        let calls = &self.pane_calls;
        self.call_order
            .retain(|pane_id| calls.contains_key(pane_id));
        let here = self.terminal_input_pane_id();
        let called = self
            .call_order
            .iter()
            .rev()
            .copied()
            .find(|pane_id| Some(*pane_id) != here)
            .and_then(|pane_id| {
                let tab = self
                    .session
                    .as_ref()?
                    .tabs
                    .iter()
                    .find(|tab| tab.layout.contains(pane_id))?;
                Some((tab.id, Some(pane_id)))
            });
        let active = self.active_tab_id();
        let target = called.or_else(|| {
            let tab_id = self
                .agents
                .iter()
                .filter(|agent| {
                    matches!(
                        agent.status,
                        AgentSessionStatus::WaitingForPermission
                            | AgentSessionStatus::WaitingForAuthentication
                    )
                })
                .find_map(|agent| agent.tab_id.filter(|tab_id| Some(*tab_id) != active))?;
            Some((tab_id, self.agent_panes.get(&tab_id).copied()))
        });
        let Some((tab_id, pane_id)) = target else {
            self.say_in_strip("nothing is calling", false, cx);
            return;
        };
        if Some(tab_id) != active {
            self.send_workspace(WorkspaceCommand::SelectTab(tab_id));
        }
        if let Some(pane_id) = pane_id {
            self.request_pane_focus(pane_id);
        }
        cx.notify();
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
            self.news_calls.remove(&pane_id);
        }
    }

    pub(super) fn forget_closed_pane_calls(&mut self) {
        let panes = &self.panes;
        self.pane_calls
            .retain(|pane_id, _| panes.contains_key(pane_id));
        let calls = &self.pane_calls;
        self.news_calls
            .retain(|pane_id| calls.contains_key(pane_id));
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

    /// Whether a pane's call is only news, a program that finished cleanly.
    pub(super) fn call_is_news(&self, pane_id: PaneId) -> bool {
        self.news_calls.contains(&pane_id)
    }

    /// Whether every call from a tab's panes is only news, so the tab calls
    /// in its own ink.
    pub(super) fn tab_call_is_news(&self, tab: &mux_workspace::Tab) -> bool {
        let mut panes = Vec::new();
        tab.layout.pane_ids(&mut panes);
        panes
            .iter()
            .filter(|pane_id| self.pane_calls.contains_key(pane_id))
            .all(|pane_id| self.call_is_news(*pane_id))
    }

    /// The colour a call is shown in: the tab's ink for news, peach for
    /// anything that wants you.
    pub(super) fn call_color(news: bool, ink: Ink) -> Hsla {
        if news { ink.color() } else { color(SIGNAL) }
    }
}

/// What a pane calls with once `call` comes after the call it has, and
/// whether that is only news. A bell rung after a notification adds nothing
/// to its words, but it wants you, so the call is no longer news.
fn next_call(previous: Option<&Attention>, call: Attention, news: bool) -> (Attention, bool) {
    match (previous, call) {
        (Some(said @ Attention::Notification(_)), Attention::Bell) => (said.clone(), false),
        (_, call) => (call, news),
    }
}

/// Whether a program ended as it should, so its finishing is only news: with
/// no failure, or stopped on purpose by Ctrl+C or a terminate signal.
fn finished_cleanly(exit_status: Option<i32>) -> bool {
    matches!(exit_status, None | Some(0 | 130 | 143))
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
    use super::{duration_words, finished_cleanly, finished_words, next_call};
    use mux_terminal::Attention;

    #[test]
    fn a_bell_after_news_keeps_its_words_but_wants_you() {
        let done = Attention::Notification("cargo is done".to_owned());
        assert_eq!(
            next_call(Some(&done), Attention::Bell, false),
            (done.clone(), false)
        );
        assert_eq!(next_call(None, done.clone(), true), (done.clone(), true));
        let failed = Attention::Notification("cargo failed".to_owned());
        assert_eq!(
            next_call(Some(&done), failed.clone(), false),
            (failed, false)
        );
        assert_eq!(
            next_call(Some(&Attention::Bell), done.clone(), true),
            (done, true)
        );
    }

    #[test]
    fn a_program_that_ended_as_it_should_is_only_news() {
        assert!(finished_cleanly(None));
        assert!(finished_cleanly(Some(0)));
        assert!(finished_cleanly(Some(130)));
        assert!(finished_cleanly(Some(143)));
        assert!(!finished_cleanly(Some(1)));
        assert!(!finished_cleanly(Some(2)));
    }

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
