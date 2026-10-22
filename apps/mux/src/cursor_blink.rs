//! A blinking cursor, when its program asks for one or the user's Ghostty
//! has `cursor-style-blink = true`; `false` keeps every cursor steady. Only
//! the pane with the keys blinks, and typing shows the cursor again at once.

use super::*;

/// How long the cursor is shown, and then hidden, as in Ghostty.
const BLINK: Duration = Duration::from_millis(600);

/// The user's `cursor-style-blink`.
pub(super) fn cursor_blink_setting() -> Option<bool> {
    static BLINK_SETTING: ghostty_config::Reloadable<Option<bool>> =
        ghostty_config::Reloadable::new(mux_terminal_ghostty::user_cursor_blink);
    BLINK_SETTING.get()
}

impl MuxApp {
    fn cursor_blinks(&self, pane_id: PaneId) -> bool {
        cursor_blink_setting() != Some(false)
            && self.terminal_input_pane_id() == Some(pane_id)
            && self
                .panes
                .get(&pane_id)
                .and_then(|pane| pane.frame.cursor)
                .is_some_and(|cursor| cursor.visible && cursor.blinking)
    }

    /// Whether the pane's cursor is in the shown half of its blink, or does
    /// not blink at all.
    pub(super) fn cursor_lit(&self, pane_id: PaneId) -> bool {
        !self.cursor_blinks(pane_id) || blink_lit(self.blink_since.elapsed())
    }

    /// Typing shows the cursor and starts its blink over.
    pub(super) fn restart_cursor_blink(&mut self) {
        self.blink_since = Instant::now();
    }

    /// Keep drawing at each turn of the blink while the pane with the keys
    /// has a blinking cursor.
    pub(super) fn ensure_cursor_blink(&mut self, cx: &mut Context<Self>) {
        let wanted = |this: &Self| {
            this.terminal_input_pane_id()
                .is_some_and(|pane_id| this.cursor_blinks(pane_id))
        };
        if self.blink_timer.is_some() || !wanted(self) {
            return;
        }
        self.blink_timer = Some(Instant::now());
        cx.spawn(async move |entity, cx| {
            loop {
                let wait = entity
                    .read_with(cx, |this, _| until_next_turn(this.blink_since.elapsed()))
                    .unwrap_or(BLINK);
                cx.background_executor().timer(wait).await;
                let live = entity
                    .update(cx, |this, cx| {
                        if wanted(this) {
                            cx.notify();
                            true
                        } else {
                            this.blink_timer = None;
                            false
                        }
                    })
                    .unwrap_or(false);
                if !live {
                    break;
                }
            }
        })
        .detach();
    }
}

fn blink_lit(elapsed: Duration) -> bool {
    (elapsed.as_millis() / BLINK.as_millis()).is_multiple_of(2)
}

fn until_next_turn(elapsed: Duration) -> Duration {
    let into = elapsed.as_millis() % BLINK.as_millis();
    BLINK.saturating_sub(Duration::from_millis(u64::try_from(into).unwrap_or(0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cursor_shows_then_hides_in_turn() {
        assert!(blink_lit(Duration::ZERO));
        assert!(blink_lit(Duration::from_millis(599)));
        assert!(!blink_lit(Duration::from_millis(600)));
        assert!(blink_lit(Duration::from_millis(1250)));
        assert_eq!(
            until_next_turn(Duration::from_millis(1250)),
            Duration::from_millis(550)
        );
    }
}
