//! A prompt gone to, by a `jump_to_prompt` keybind, the palette or the pinned
//! header, washes for a moment in the tab's ink, so the eye lands on it in
//! a screen of output.

use super::*;

/// How long the wash takes to fade.
const PROMPT_FLASH: Duration = Duration::from_millis(600);

/// The prompt row washed, in which pane, and which flash it is.
#[derive(Clone, Copy)]
pub(super) struct PromptFlash {
    pane_id: PaneId,
    row: usize,
    serial: u64,
}

impl MuxApp {
    /// Wash the prompt at this history row of the pane.
    pub(super) fn flash_prompt(&mut self, pane_id: PaneId, row: usize, cx: &mut Context<Self>) {
        let serial = self.prompt_flash.map_or(0, |flash| flash.serial + 1);
        self.prompt_flash = Some(PromptFlash {
            pane_id,
            row,
            serial,
        });
        cx.spawn(async move |entity, cx| {
            cx.background_executor().timer(PROMPT_FLASH).await;
            let _ = entity.update(cx, |this, cx| {
                if this
                    .prompt_flash
                    .is_some_and(|flash| flash.serial == serial)
                {
                    this.prompt_flash = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The wash over the prompt row gone to, while it fades.
    pub(super) fn prompt_flash_overlay(
        &self,
        pane_id: PaneId,
        rect: layout::Rect,
    ) -> Option<gpui::AnyElement> {
        let flash = self.prompt_flash.filter(|flash| flash.pane_id == pane_id)?;
        let frame = &self.panes.get(&pane_id)?.frame;
        let row = flash
            .row
            .checked_sub(usize::try_from(frame.scroll.offset).ok()?)?;
        let row = u16::try_from(row).ok().filter(|row| *row < frame.rows)?;
        let padding =
            self.metrics
                .balanced_padding(rect.width, rect.height, frame.cols, frame.rows);
        let height = self.metrics.cell_height;
        let wash = div()
            .absolute()
            .left_0()
            .top(px(padding.top + height * f32::from(row)))
            .w(px(rect.width))
            .h(px(height))
            .bg(self.active_ink().color().opacity(0.22));
        if self.motion == MotionPreference::Reduced {
            return Some(wash.into_any_element());
        }
        Some(
            wash.with_animation(
                SharedString::from(format!("prompt-flash-{}", flash.serial)),
                Animation::new(PROMPT_FLASH).with_easing(gpui::ease_out_quint()),
                |wash, delta| wash.opacity(1.0 - delta),
            )
            .into_any_element(),
        )
    }
}
