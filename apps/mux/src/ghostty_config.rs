//! Settings read from the user's Ghostty config, kept until the config is
//! reloaded (⌘⇧, as in Ghostty), and read again the next time they are
//! wanted after that.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

static GENERATION: AtomicU64 = AtomicU64::new(0);

/// One setting, read from the config when first wanted.
pub(crate) struct Reloadable<T> {
    read: fn() -> T,
    kept: Mutex<Option<(u64, T)>>,
}

impl<T: Clone> Reloadable<T> {
    pub(crate) const fn new(read: fn() -> T) -> Self {
        Self {
            read,
            kept: Mutex::new(None),
        }
    }

    pub(crate) fn get(&self) -> T {
        let generation = generation();
        let mut kept = self
            .kept
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match kept.as_ref() {
            Some((read_at, value)) if *read_at == generation => value.clone(),
            _ => {
                let value = (self.read)();
                *kept = Some((generation, value.clone()));
                value
            }
        }
    }
}

/// How many times the config has been reloaded: what was read before a
/// reload is stale after it.
pub(crate) fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

/// Forget every setting read so far, so each is read again when next
/// wanted.
pub(crate) fn forget_settings() {
    GENERATION.fetch_add(1, Ordering::AcqRel);
}

impl super::MuxApp {
    /// Read the user's Ghostty config again, as ⌘⇧, does in Ghostty:
    /// colours, font, cursor and the rest take effect at once, in every
    /// pane. The text size goes back to the config's.
    pub(super) fn reload_ghostty_config(&mut self, cx: &mut gpui::Context<Self>) {
        forget_settings();
        self.ghostty_theme = mux_terminal_ghostty::GhosttyTheme::load_user().unwrap_or_default();
        let (font, metrics) = Self::resolve_terminal_font(cx);
        self.terminal_font = font;
        self.configured_font_size = metrics.font_size;
        self.metrics = metrics;
        let theme = &self.ghostty_theme;
        let blink = super::cursor_blink::cursor_blink_setting() == Some(true);
        for pane in self.panes.values_mut() {
            let _ = pane.with_engine(|engine| {
                if !theme.is_empty() {
                    engine.apply_theme(theme)?;
                }
                engine.set_default_cursor_style(super::pane_cursor_style())?;
                engine.set_default_cursor_blink(blink)?;
                engine.set_history_limit(super::pane_history_bytes())
            });
            let _ = pane.publish_frame();
        }
        self.terminal_views.clear();
        super::configure_pointer_hiding(cx);
        super::keybind::bind_ghostty_keys(cx);
        self.say_in_strip("reloaded the Ghostty config", false, cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    static READS: AtomicU32 = AtomicU32::new(0);

    fn read() -> u32 {
        READS.fetch_add(1, Ordering::SeqCst) + 1
    }

    #[test]
    fn a_setting_is_read_once_until_the_config_is_reloaded() {
        static SETTING: Reloadable<u32> = Reloadable::new(read);
        let first = SETTING.get();
        assert_eq!(SETTING.get(), first);
        forget_settings();
        assert!(SETTING.get() > first);
    }
}
