//! A quiet pane's history compressed, as the daemon compresses its own copy:
//! once a pane has printed nothing for a few seconds, the window's copy of
//! its scrollback is compressed off the window's thread, a step at a time.
//! History scrolled back to later is uncompressed again as it is read.

use super::*;

/// How long a pane prints nothing before its history is compressed.
const QUIET_FOR: Duration = Duration::from_secs(5);

impl MuxApp {
    /// Keep the timer going while some pane's history is owed compressing.
    pub(super) fn ensure_history_compression(&mut self, cx: &mut Context<Self>) {
        if self.history_compression_since.is_some()
            || !self.panes.values().any(|pane| pane.compression_owed)
        {
            return;
        }
        self.history_compression_since = Some(Instant::now());
        cx.spawn(async move |entity, cx| {
            loop {
                cx.background_executor().timer(QUIET_FOR).await;
                let Ok(Some(engines)) = entity.update(cx, |this, _| this.take_quiet_engines())
                else {
                    break;
                };
                for engine in engines {
                    cx.background_executor()
                        .spawn(async move { compress_history(&engine) })
                        .await;
                }
            }
        })
        .detach();
    }

    /// The engines of panes quiet long enough to compress, taken off the
    /// owed list, or `None`, which stops the timer, when no pane is owed.
    fn take_quiet_engines(&mut self) -> Option<Vec<pane_parser::SharedEngine>> {
        let mut still_owed = false;
        let mut quiet = Vec::new();
        for pane in self.panes.values_mut().filter(|pane| pane.compression_owed) {
            if pane.output_at.elapsed() >= QUIET_FOR {
                pane.compression_owed = false;
                quiet.push(pane.engine.clone());
            } else {
                still_owed = true;
            }
        }
        if quiet.is_empty() && !still_owed {
            self.history_compression_since = None;
            return None;
        }
        Some(quiet)
    }
}

/// The lock is let go between steps, so output that arrives meanwhile
/// waits a step at most.
fn compress_history(engine: &pane_parser::SharedEngine) {
    while engine.lock().compress_history_step().unwrap_or(false) {}
}
