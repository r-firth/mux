//! Holding Cmd on its own a moment numbers the tabs, as Safari does, with
//! the number Cmd+1 through Cmd+9 picks each by: the first eight by place,
//! and the last by 9. Pressing a key with Cmd before then (Cmd+C, Cmd+T)
//! shows nothing, so the numbers never flicker through quick shortcuts.

use super::{Context, Duration, MuxApp};

/// Whether the tabs show their numbers, and which press of Cmd a wait to
/// show them belongs to.
#[derive(Default)]
pub(super) struct TabNumbers {
    pub(super) showing: bool,
    serial: u64,
}

/// How long Cmd is held alone before the tabs show their numbers.
const NUMBERS_AFTER: Duration = Duration::from_millis(450);

/// The number Cmd picks a tab by, from its place among `count` tabs.
pub(super) fn command_number(position: usize, count: usize) -> Option<usize> {
    if position + 1 == count {
        Some(count.min(9))
    } else {
        (position < 8).then_some(position + 1)
    }
}

impl MuxApp {
    /// Cmd went down or up, or another modifier joined it.
    pub(super) fn tab_numbers_modifiers_changed(
        &mut self,
        modifiers: gpui::Modifiers,
        cx: &mut Context<Self>,
    ) {
        self.tab_numbers.serial += 1;
        let alone = modifiers.platform
            && !(modifiers.control || modifiers.alt || modifiers.shift || modifiers.function);
        if !alone {
            if self.tab_numbers.showing {
                self.tab_numbers.showing = false;
                cx.notify();
            }
            return;
        }
        let serial = self.tab_numbers.serial;
        cx.spawn(async move |entity, cx| {
            cx.background_executor().timer(NUMBERS_AFTER).await;
            let _ = entity.update(cx, |this, cx| {
                if this.tab_numbers.serial == serial {
                    this.tab_numbers.showing = true;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// A key was pressed: Cmd held for a shortcut shows no numbers. Ones
    /// already showing stay, for picking another tab.
    pub(super) fn tab_numbers_key_pressed(&mut self) {
        self.tab_numbers.serial += 1;
    }

    /// The window went to the back, where Cmd's release goes unseen.
    pub(super) fn hide_tab_numbers(&mut self) {
        self.tab_numbers.serial += 1;
        self.tab_numbers.showing = false;
    }
}

#[cfg(test)]
mod tests {
    use super::command_number;

    #[test]
    fn the_last_tab_goes_by_nine() {
        let numbers = |count| {
            (0..count)
                .map(|at| command_number(at, count))
                .collect::<Vec<_>>()
        };
        assert_eq!(numbers(3), [Some(1), Some(2), Some(3)]);
        assert_eq!(numbers(9)[8], Some(9));
        let eleven = numbers(11);
        assert_eq!(eleven[7], Some(8));
        assert_eq!(eleven[8], None);
        assert_eq!(eleven[9], None);
        assert_eq!(eleven[10], Some(9));
    }
}
