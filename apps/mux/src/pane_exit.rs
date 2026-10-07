//! Panes whose program has exited.
//!
//! A shell told to exit in the pane being typed in takes its pane with it, as
//! in tmux; a clean exit asks for nothing more. Any other exit (a failure, or
//! a pane that ended while something else had the keys) stays on screen, and
//! a rule drawn across the row where the next prompt would have been closes
//! its transcript: what happened on the left and, with the focus, the keys
//! that act on it on the right. The session's last pane is never closed;
//! enter starts a new shell there instead.

use super::*;
use mux_workspace::SplitAxis;

/// A dead pane being replaced in place. Closing a pane focuses the tab's
/// first one, so the shell that took its region is given the focus back once
/// it appears.
pub(super) struct PaneRestart {
    tab_id: TabId,
    exited: PaneId,
    before: HashSet<PaneId>,
}

/// What closing the focused pane takes with it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PaneEnd {
    Pane,
    Tab,
    /// The session's last pane of all, which stays.
    Last,
}

fn pane_end(panes_in_tab: usize, tabs: usize) -> PaneEnd {
    if panes_in_tab > 1 {
        PaneEnd::Pane
    } else if tabs > 1 {
        PaneEnd::Tab
    } else {
        PaneEnd::Last
    }
}

/// How an exit reads: the code only when it says something went wrong.
pub(super) fn exit_words(status: ProcessExit) -> String {
    match status.code {
        Some(code) if !status.success => format!("exited {code}"),
        _ => "exited".to_owned(),
    }
}

/// The grid cell the closing rule starts at: the start of the row the next
/// prompt would have taken, or the bottom row when that is out of view.
fn rule_row(frame: &RenderFrame) -> u16 {
    let last = frame.rows.saturating_sub(1);
    match frame.cursor.filter(|_| !frame.scroll.is_scrolled()) {
        Some(cursor) if cursor.y <= last => {
            if cursor.x == 0 || cursor.y == last {
                cursor.y
            } else {
                cursor.y + 1
            }
        }
        _ => last,
    }
}

impl MuxApp {
    /// Note exits from a fresh view of the workspace, and finish a restart
    /// the view shows has happened.
    pub(super) fn note_pane_exits(&mut self, exits: HashMap<PaneId, ProcessExit>) {
        self.pane_exits = exits;
        self.finish_pane_restart();
    }

    pub(super) fn note_pane_exit(
        &mut self,
        session_id: SessionId,
        pane_id: PaneId,
        status: ProcessExit,
    ) {
        if self
            .session
            .as_ref()
            .is_none_or(|session| session.id != session_id)
            || !self.panes.contains_key(&pane_id)
        {
            return;
        }
        let typed_in = self.focused_pane_id() == Some(pane_id)
            && self
                .pending_focused_pane
                .is_none_or(|pending| pending == pane_id);
        if status.success && typed_in && self.active_agent_pane() != Some(pane_id) {
            match self.focused_pane_end() {
                PaneEnd::Pane => {
                    self.send_workspace(WorkspaceCommand::ClosePane);
                    return;
                }
                PaneEnd::Tab => {
                    self.send_workspace(WorkspaceCommand::CloseTab);
                    return;
                }
                PaneEnd::Last => {}
            }
        }
        self.pane_exits.insert(pane_id, status);
    }

    fn focused_pane_end(&self) -> PaneEnd {
        let Some(session) = self.session.as_ref() else {
            return PaneEnd::Last;
        };
        let mut panes = Vec::new();
        if let Some(tab) = session.active_tab() {
            tab.layout.pane_ids(&mut panes);
        }
        pane_end(panes.len(), session.tabs.len())
    }

    /// A key typed into a pane whose program has exited. Enter closes it (or
    /// starts a new shell in the session's last pane), n starts a new shell
    /// in its place, and anything else has nowhere to go. Returns whether the
    /// key was the dead pane's.
    pub(super) fn exited_pane_key(&mut self, keystroke: &gpui::Keystroke, held: bool) -> bool {
        let Some(pane_id) = self
            .terminal_input_pane_id()
            .filter(|pane_id| self.pane_exits.contains_key(pane_id))
        else {
            return false;
        };
        if held || keystroke.modifiers.modified() {
            return true;
        }
        match keystroke.key.as_str() {
            "enter" => match self.focused_pane_end() {
                PaneEnd::Pane => self.send_workspace(WorkspaceCommand::ClosePane),
                PaneEnd::Tab => self.send_workspace(WorkspaceCommand::CloseTab),
                PaneEnd::Last => self.restart_exited_pane(pane_id),
            },
            "n" => self.restart_exited_pane(pane_id),
            _ => {}
        }
        true
    }

    /// A new shell where the dead one was: split it, then close it, so the
    /// tab keeps its name, its ink and the region the pane had.
    fn restart_exited_pane(&mut self, exited: PaneId) {
        let Some(tab) = self.session.as_ref().and_then(Session::active_tab) else {
            return;
        };
        let mut before = Vec::new();
        tab.layout.pane_ids(&mut before);
        let tab_id = tab.id;
        self.send_workspace(WorkspaceCommand::SplitPane(SplitAxis::Horizontal));
        self.send_workspace(WorkspaceCommand::SetFocusedPane(exited));
        self.send_workspace(WorkspaceCommand::ClosePane);
        // Alone in its tab, the new shell is the first pane and has the focus
        // already.
        self.pane_restart = (before.len() > 1).then(|| PaneRestart {
            tab_id,
            exited,
            before: before.into_iter().collect(),
        });
    }

    fn finish_pane_restart(&mut self) {
        let Some(restart) = self.pane_restart.as_ref() else {
            return;
        };
        let Some(session) = self.session.as_ref() else {
            self.pane_restart = None;
            return;
        };
        let Some(tab) = session.tabs.iter().find(|tab| tab.id == restart.tab_id) else {
            self.pane_restart = None;
            return;
        };
        if tab.layout.contains(restart.exited) {
            return;
        }
        let mut panes = Vec::new();
        tab.layout.pane_ids(&mut panes);
        let fresh = panes
            .into_iter()
            .find(|pane_id| !restart.before.contains(pane_id))
            .filter(|pane_id| session.active_tab == tab.id && tab.focused_pane != *pane_id);
        self.pane_restart = None;
        if let Some(fresh) = fresh {
            self.request_pane_focus(fresh);
        }
    }

    pub(super) fn pane_has_exited(&self, pane_id: PaneId) -> bool {
        self.pane_exits.contains_key(&pane_id)
    }

    /// The rule that closes a dead pane's transcript.
    pub(super) fn render_pane_exit(
        &self,
        geometry: layout::PaneGeometry,
    ) -> Option<gpui::AnyElement> {
        let status = *self.pane_exits.get(&geometry.pane_id)?;
        let frame = &self.panes.get(&geometry.pane_id)?.frame;
        let metrics = self.metrics;
        let rect = geometry.rect;
        let padding = metrics.balanced_padding(rect.width, rect.height, frame.cols, frame.rows);
        let cell = metrics.cell_width;
        let top = rect.y + padding.top + f32::from(rule_row(frame)) * metrics.cell_height;
        let width = (f32::from(frame.cols) * cell).min(rect.width - padding.left);
        let tone = if status.success { MUTED_TEXT } else { SIGNAL };
        let rule = |grow: bool| {
            div().h(px(1.0)).bg(hairline(0.14)).map(|rule| {
                if grow {
                    rule.flex_1().min_w(px(cell))
                } else {
                    rule.w(px(cell * 2.0)).flex_none()
                }
            })
        };
        let mut line = h_flex()
            .absolute()
            .left(px(rect.x + padding.left))
            .top(px(top))
            .w(px(width))
            .h(px(metrics.cell_height))
            .items_center()
            .gap(px(cell))
            .overflow_hidden()
            .bg(color(SURFACE))
            .font_family(self.terminal_font.clone())
            .text_size(px(metrics.font_size))
            .line_height(px(metrics.cell_height))
            .whitespace_nowrap()
            .text_color(color(FAINT_TEXT))
            .child(rule(false))
            .child(
                div()
                    .flex_none()
                    .text_color(color(tone))
                    .child(exit_words(status)),
            )
            .child(rule(true));
        if geometry.focused {
            let keys: &[(&str, &str)] = if self.focused_pane_end() == PaneEnd::Last {
                &[("enter", "new shell")]
            } else {
                &[("enter", "close"), ("n", "new shell")]
            };
            let mut hints = h_flex().flex_none().gap(px(cell * 2.0));
            for (key, label) in keys {
                hints = hints.child(
                    h_flex()
                        .gap(px(cell))
                        .child(kbd(*key, color(TEXT)))
                        .child(*label),
                );
            }
            line = line.child(hints);
        }
        Some(line.into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mux_terminal::{CursorStyle, RenderCursor, RenderDirty, Rgb, TerminalScrollState};

    fn frame(cursor: Option<(u16, u16)>, scrolled: bool) -> RenderFrame {
        RenderFrame {
            cols: 80,
            rows: 24,
            dirty: RenderDirty::Clean,
            background: Rgb::default(),
            foreground: Rgb::default(),
            cursor: cursor.map(|(x, y)| RenderCursor {
                visible: true,
                blinking: false,
                x,
                y,
                style: CursorStyle::Block,
                color: Rgb::default(),
            }),
            scroll: TerminalScrollState {
                total: if scrolled { 200 } else { 24 },
                offset: 0,
                len: 24,
            },
            row_metadata: Vec::new(),
            cells: Vec::new(),
        }
    }

    #[test]
    fn a_clean_exit_says_so_and_a_failure_gives_its_code() {
        let clean = ProcessExit {
            code: Some(0),
            success: true,
        };
        let failed = ProcessExit {
            code: Some(1),
            success: false,
        };
        assert_eq!(exit_words(clean), "exited");
        assert_eq!(exit_words(failed), "exited 1");
    }

    #[test]
    fn closing_takes_the_pane_then_the_tab_but_never_the_last_of_all() {
        assert_eq!(pane_end(2, 1), PaneEnd::Pane);
        assert_eq!(pane_end(1, 3), PaneEnd::Tab);
        assert_eq!(pane_end(1, 1), PaneEnd::Last);
    }

    #[test]
    fn the_rule_takes_the_row_the_next_prompt_would_have() {
        // After `exit`, the shell leaves the cursor at the start of a row.
        assert_eq!(rule_row(&frame(Some((0, 5)), false)), 5);
        // A program that ends mid-row: the rule goes under what it wrote.
        assert_eq!(rule_row(&frame(Some((12, 5)), false)), 6);
        assert_eq!(rule_row(&frame(Some((12, 23)), false)), 23);
        // Scrolled back through history, or with no cursor: the bottom row.
        assert_eq!(rule_row(&frame(Some((0, 5)), true)), 23);
        assert_eq!(rule_row(&frame(None, false)), 23);
    }
}
