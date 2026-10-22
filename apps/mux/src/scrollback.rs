//! A pane's history.
//!
//! The wheel moves through it. ⌘home and ⌘end go to either end, and ⌘ or ⇧
//! with page up and page down go a page at a time. Typing anything returns a
//! pane to its latest output, as in Ghostty, so what is typed is never sent
//! somewhere out of sight. While a pane is scrolled back, a thin thumb on its
//! right edge says where in its history it is; drag it to move through the
//! history, or click above or below it to jump there. A pill at the bottom
//! says how much is below the view, and a click on it goes back down. Above
//! output whose prompt is out of sight, that prompt's line stays pinned to
//! the pane's top.

use super::*;
use mux_terminal::{CommandEnding, TerminalScrollState};

/// The keys that move a pane through its history rather than reaching its
/// program. ⇧ with page up or down reaches the program when there is no
/// history to move through, as in a full-screen program's own screen.
fn scrollback_key(
    keystroke: &gpui::Keystroke,
    rows: u16,
    has_history: bool,
) -> Option<TerminalViewportScroll> {
    let modifiers = keystroke.modifiers;
    if modifiers.control || modifiers.alt {
        return None;
    }
    let command = modifiers.platform && !modifiers.shift;
    let shift = modifiers.shift && !modifiers.platform;
    let page = i64::from(rows.saturating_sub(1).max(1));
    match keystroke.key.as_str() {
        "home" if command => Some(TerminalViewportScroll::Top),
        "end" if command => Some(TerminalViewportScroll::Bottom),
        "pageup" if command || (shift && has_history) => Some(TerminalViewportScroll::Delta(-page)),
        "pagedown" if command || (shift && has_history) => {
            Some(TerminalViewportScroll::Delta(page))
        }
        _ => None,
    }
}

/// ⌘↑ and ⌘↓ (with ⇧ too, as iTerm has them) step through the shell's
/// prompts: whether the key goes to an older one.
fn prompt_key(keystroke: &gpui::Keystroke) -> Option<bool> {
    let modifiers = keystroke.modifiers;
    if !modifiers.platform || modifiers.control || modifiers.alt {
        return None;
    }
    match keystroke.key.as_str() {
        "up" => Some(true),
        "down" => Some(false),
        _ => None,
    }
}

/// The scroll that brings the next prompt above the view's top, or below
/// it, to the top. Past the newest prompt is the latest output.
fn prompt_jump(
    prompts: &[usize],
    scroll: TerminalScrollState,
    older: bool,
) -> Option<(TerminalViewportScroll, Option<usize>)> {
    let top = usize::try_from(scroll.offset).unwrap_or(usize::MAX);
    let target = if older {
        prompts.iter().rev().find(|&&row| row < top)
    } else {
        prompts.iter().find(|&&row| row > top)
    };
    let bottom = scroll.offset.saturating_add(scroll.len) >= scroll.total;
    match target {
        Some(&row) => {
            let delta =
                i64::try_from(row).unwrap_or(i64::MAX) - i64::try_from(top).unwrap_or(i64::MAX);
            Some((TerminalViewportScroll::Delta(delta), Some(row)))
        }
        None if !older && !bottom => Some((TerminalViewportScroll::Bottom, None)),
        None => None,
    }
}

/// The prompt of the command whose output starts the view: the last one
/// above the view's top. None when the view starts on a prompt, or before
/// any.
fn command_above(prompts: &[usize], scroll: TerminalScrollState) -> Option<usize> {
    let top = usize::try_from(scroll.offset).unwrap_or(usize::MAX);
    prompts
        .iter()
        .rev()
        .find(|&&row| row <= top)
        .copied()
        .filter(|&row| row < top)
}

/// How the command run from the prompt at `index` of `prompt_count` ended:
/// the shell's endings lined up with the prompts from the newest.
fn ending_at(
    endings: &std::collections::VecDeque<CommandEnding>,
    prompt_count: usize,
    index: usize,
) -> CommandEnding {
    prompt_count
        .checked_sub(index + 1)
        .and_then(|from_newest| endings.len().checked_sub(from_newest + 1))
        .map(|at| endings[at])
        .unwrap_or_default()
}

/// What is worth saying of how a command ended: its status if it failed,
/// and how long it ran if that was a second or more.
pub(super) fn ending_words(ending: CommandEnding) -> Option<String> {
    let status = ending.failed().map(|status| format!("exit {status}"));
    let took = ending
        .took
        .filter(|took| took.as_secs() >= 1)
        .map(|took| pane_calls::duration_words(took.as_secs()));
    match (status, took) {
        (Some(status), Some(took)) => Some(format!("{status} · {took}")),
        (status, took) => status.or(took),
    }
}

/// The command on a prompt's line, after the prompt: what follows the
/// first `$ `, `# `, `% `, `> ` or `❯ `, or the whole line when it has none.
/// A line that ends in one of them, its space trimmed, is a prompt nothing
/// was typed at.
fn command_of(line: &str) -> &str {
    let line = line.trim_end();
    let marks = ["$ ", "# ", "% ", "> ", "❯ "];
    if marks.iter().any(|mark| line.ends_with(mark.trim_end())) {
        let typed = marks.iter().filter_map(|mark| line.find(mark)).min();
        if typed.is_none() {
            return "";
        }
    }
    marks
        .iter()
        .filter_map(|mark| line.find(mark).map(|at| at + mark.len()))
        .min()
        .map_or(line, |start| &line[start..])
        .trim()
}

/// A command run in a pane, for the palette to go back to.
#[derive(Clone, Debug)]
pub(super) struct CommandRan {
    pub(super) pane_id: PaneId,
    /// Its prompt's row, from the top of the history.
    pub(super) row: usize,
    pub(super) command: String,
    pub(super) ending: CommandEnding,
    /// Where it ran, when not in the pane the palette opened over.
    pub(super) place: Option<String>,
}

/// How many of a pane's latest commands the palette offers.
const COMMANDS_OFFERED: usize = 200;

/// The prompt a history row's command was run from: the row's own, or the
/// last above it.
fn prompt_owning(prompts: &[usize], row: usize) -> Option<usize> {
    prompts.iter().rev().find(|&&prompt| prompt <= row).copied()
}

/// The rows a command printed, between its prompt and the next (or the end
/// of the history), its trailing blank lines left out. None when it printed
/// nothing.
pub(super) fn output_rows(
    prompts: &[usize],
    prompt: usize,
    lines: &[&str],
) -> Option<(usize, usize)> {
    let next = prompts
        .iter()
        .find(|&&row| row > prompt)
        .copied()
        .unwrap_or(lines.len());
    let first = prompt + 1;
    let last = (first..next)
        .rev()
        .find(|&row| lines.get(row).is_some_and(|line| !line.trim().is_empty()))?;
    Some((first, last))
}

/// The command run from the prompt at `prompt` after a `$ `, then what it
/// printed. None when no command was run there.
fn transcript(prompts: &[usize], prompt: usize, lines: &[&str]) -> Option<String> {
    let command = command_of(lines.get(prompt)?);
    if command.is_empty() {
        return None;
    }
    let mut transcript = format!("$ {command}");
    if let Some((first, last)) = output_rows(prompts, prompt, lines) {
        for line in &lines[first..=last] {
            transcript.push('\n');
            transcript.push_str(line.trim_end());
        }
    }
    Some(transcript)
}

/// A pane's text as a file holds it: each line's trailing blanks and the
/// blank lines at the end left off, ending in a newline.
fn history_text(screen: &str) -> String {
    let mut text = screen
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n");
    text.truncate(text.trim_end().len());
    text.push('\n');
    text
}

/// Each pane's prompt rows, with the frame they were read for.
pub(super) type PromptRows = std::cell::RefCell<HashMap<PaneId, (u64, Rc<Vec<usize>>)>>;

/// How far in from the pane's right edge the thumb can be grabbed: wider
/// than the thumb, which is slim to stay out of the way.
const THUMB_GRIP: f32 = 14.0;
const THUMB_INSET: f32 = 6.0;

/// A held thumb: where its track is in the window, and where on the thumb
/// the pointer took it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ThumbDrag {
    pane_id: PaneId,
    track_top: f32,
    track: f32,
    grab: f32,
}

/// How many rows of a scrolled-back pane are below its view, if any.
fn rows_below(scroll: TerminalScrollState) -> Option<u64> {
    let below = scroll
        .total
        .saturating_sub(scroll.offset.saturating_add(scroll.len));
    (below > 0).then_some(below)
}

/// The history row the view starts at with the thumb's top `top` down a
/// track `track` tall.
fn offset_for_thumb(scroll: TerminalScrollState, track: f32, top: f32) -> u64 {
    let (height, _) = thumb_geometry(scroll, track);
    let travel = scroll.total.saturating_sub(scroll.len);
    let room = (track - height).max(1.0);
    let fraction = (top / room).clamp(0.0, 1.0);
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let offset = (fraction * travel as f32).round() as u64;
    offset.min(travel)
}

/// The thumb's height and its offset from the top of a track `track` tall.
fn thumb_geometry(scroll: TerminalScrollState, track: f32) -> (f32, f32) {
    let total = scroll.total.max(1) as f32;
    let shown = scroll.len as f32;
    let height = (track * shown / total).clamp(track.min(24.0), track);
    let travel = (total - shown).max(1.0);
    let top = (track - height) * (scroll.offset as f32 / travel).clamp(0.0, 1.0);
    (height, top)
}

impl MuxApp {
    /// A press on a pane's right edge while its thumb shows: take the thumb,
    /// or, off it, jump so the thumb sits under the pointer and take it
    /// there. Returns whether the press was the thumb's.
    pub(super) fn grab_scroll_thumb(
        &mut self,
        pane_id: PaneId,
        body: layout::Rect,
        event: &gpui::MouseDownEvent,
    ) -> bool {
        let (x, y) = (f32::from(event.position.x), f32::from(event.position.y));
        if event.button != gpui::MouseButton::Left
            || x < body.x + body.width - THUMB_GRIP
            || !self.scroll_thumb_shown(pane_id)
        {
            return false;
        }
        let Some(scroll) = self.panes.get(&pane_id).map(|pane| pane.frame.scroll) else {
            return false;
        };
        let track_top = body.y + THUMB_INSET;
        let track = body.height - THUMB_INSET * 2.0;
        let (height, top) = thumb_geometry(scroll, track);
        let at = y - track_top;
        let grab = if (top..=top + height).contains(&at) {
            at - top
        } else {
            height / 2.0
        };
        self.request_pane_focus(pane_id);
        self.thumb_drag = Some(ThumbDrag {
            pane_id,
            track_top,
            track,
            grab,
        });
        self.drag_scroll_thumb(event.position);
        true
    }

    /// The held thumb follows the pointer. Returns whether one is held.
    pub(super) fn drag_scroll_thumb(&mut self, position: gpui::Point<gpui::Pixels>) -> bool {
        let Some(drag) = self.thumb_drag else {
            return false;
        };
        let Some(scroll) = self.panes.get(&drag.pane_id).map(|pane| pane.frame.scroll) else {
            self.thumb_drag = None;
            return false;
        };
        let top = f32::from(position.y) - drag.track_top - drag.grab;
        let target = offset_for_thumb(scroll, drag.track, top);
        if target != scroll.offset {
            #[allow(clippy::cast_possible_wrap)]
            let delta = target as i64 - scroll.offset as i64;
            self.scroll_viewport(drag.pane_id, TerminalViewportScroll::Delta(delta));
        }
        true
    }

    /// Let go of the thumb. Returns whether one was held.
    pub(super) fn release_scroll_thumb(&mut self) -> bool {
        self.thumb_drag.take().is_some()
    }

    /// Where the thumb can be taken, in the pane's body, with the arrow
    /// over it rather than the text's I-beam.
    pub(super) fn render_thumb_grip(
        &self,
        pane_id: PaneId,
        body: layout::Rect,
    ) -> Option<gpui::AnyElement> {
        self.scroll_thumb_shown(pane_id).then(|| {
            div()
                .absolute()
                .top_0()
                .left(px(body.width - THUMB_GRIP))
                .w(px(THUMB_GRIP))
                .h(px(body.height))
                .cursor_default()
                .into_any_element()
        })
    }

    fn scroll_thumb_shown(&self, pane_id: PaneId) -> bool {
        let Some(scroll) = self.panes.get(&pane_id).map(|pane| pane.frame.scroll) else {
            return false;
        };
        let finding = self
            .find
            .as_ref()
            .is_some_and(|find| find.pane_id == pane_id)
            && scroll.total > scroll.len;
        let held = self.thumb_drag.is_some_and(|drag| drag.pane_id == pane_id);
        scroll.is_scrolled() || finding || held
    }

    /// A history key typed into the pane with the keys. Returns whether the
    /// key was one.
    pub(super) fn scrollback_key_down(
        &mut self,
        keystroke: &gpui::Keystroke,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(pane_id) = self.terminal_input_pane_id() else {
            return false;
        };
        let Some(pane) = self.panes.get(&pane_id) else {
            return false;
        };
        let scroll = pane.frame.scroll;
        let movement = match prompt_key(keystroke) {
            Some(older) => {
                let prompts = pane
                    .with_engine(|engine| engine.prompt_rows())
                    .unwrap_or_default();
                if prompts.is_empty() {
                    return false;
                }
                prompt_jump(&prompts, scroll, older)
            }
            None => scrollback_key(keystroke, pane.frame.rows, scroll.total > scroll.len)
                .map(|movement| (movement, None)),
        };
        let Some((movement, prompt)) = movement else {
            return false;
        };
        if self.scroll_viewport(pane_id, movement) {
            cx.notify();
        }
        if let Some(prompt) = prompt {
            self.flash_prompt(pane_id, prompt, cx);
        }
        true
    }

    /// Step the pane with the keys to the prompt before, or after, the top
    /// of its view.
    pub(super) fn jump_to_prompt(&mut self, older: bool, cx: &mut Context<Self>) {
        let Some(pane_id) = self.terminal_input_pane_id() else {
            return;
        };
        let Some(pane) = self.panes.get(&pane_id) else {
            return;
        };
        let prompts = pane
            .with_engine(|engine| engine.prompt_rows())
            .unwrap_or_default();
        if let Some((movement, row)) = prompt_jump(&prompts, pane.frame.scroll, older) {
            self.scroll_input_pane(movement, cx);
            if let Some(row) = row {
                self.flash_prompt(pane_id, row, cx);
            }
        }
    }

    /// Move the pane with the keys through its history.
    pub(super) fn scroll_input_pane(
        &mut self,
        movement: TerminalViewportScroll,
        cx: &mut Context<Self>,
    ) {
        if let Some(pane_id) = self.terminal_input_pane_id()
            && self.scroll_viewport(pane_id, movement)
        {
            cx.notify();
        }
    }

    /// Typing into a pane scrolled back through its history brings it back to
    /// the latest output.
    pub(super) fn return_to_latest(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        if self
            .panes
            .get(&pane_id)
            .is_some_and(|pane| pane.frame.scroll.is_scrolled())
            && self.scroll_viewport(pane_id, TerminalViewportScroll::Bottom)
        {
            cx.notify();
        }
    }

    pub(super) fn scroll_viewport(
        &mut self,
        pane_id: PaneId,
        movement: TerminalViewportScroll,
    ) -> bool {
        let Some(pane) = self.panes.get_mut(&pane_id) else {
            return false;
        };
        if let Err(error) = pane.with_engine(|engine| engine.scroll_viewport(movement)) {
            error!(%pane_id, %error, "could not scroll terminal viewport");
            return false;
        }
        if let Err(error) = pane.publish_frame() {
            error!(%pane_id, %error, "could not render scrolled terminal viewport");
            return false;
        }
        true
    }

    /// On a scrolled-back pane, how much is below the view, at the bottom
    /// of its body; a click goes back to the latest output. It counts up as
    /// the pane carries on printing out of sight.
    pub(super) fn render_latest_pill(
        &self,
        geometry: layout::PaneGeometry,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let scroll = self.panes.get(&geometry.pane_id)?.frame.scroll;
        let below = rows_below(scroll)?;
        let pane_id = geometry.pane_id;
        let rect = geometry.surface();
        let words = if below == 1 {
            "1 line below".to_owned()
        } else {
            format!("{below} lines below")
        };
        // Laid out in a box over the body that takes no pointer itself, so
        // the pill can sit against its bottom right whatever its width.
        let pill = h_flex()
            .id(SharedString::from(format!("latest-{pane_id}")))
            .h(px(24.0))
            .px(px(10.0))
            .gap(px(6.0))
            .items_center()
            .rounded_full()
            .border_1()
            .border_color(hairline(0.14))
            .bg(color(SURFACE))
            .shadow_md()
            .block_mouse_except_scroll()
            .cursor_pointer()
            .font_family(EMBEDDED_TERMINAL_FONT)
            .text_size(px(11.5))
            .text_color(color(MUTED_TEXT))
            .hover(|style| {
                style
                    .text_color(color(TEXT))
                    .bg(color(SURFACE).blend(wash(0.06)))
            })
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.scroll_viewport(pane_id, TerminalViewportScroll::Bottom) {
                    cx.notify();
                }
            }))
            .child("↓")
            .child(words);
        Some(
            h_flex()
                .absolute()
                .left(px(rect.x))
                .top(px(rect.y))
                .w(px(rect.width))
                .h(px(rect.height))
                .justify_end()
                .items_end()
                .pr(px(THUMB_GRIP + 4.0))
                .pb(px(10.0))
                .child(pill)
                .into_any_element(),
        )
    }

    /// The thumb on a scrolled-back pane's right edge. While find is open in
    /// a pane with history, it shows there too, beside the ticks for what was
    /// found.
    pub(super) fn render_scroll_thumb(
        &self,
        geometry: layout::PaneGeometry,
    ) -> Option<gpui::AnyElement> {
        let scroll = self.panes.get(&geometry.pane_id)?.frame.scroll;
        if !self.scroll_thumb_shown(geometry.pane_id) {
            return None;
        }
        let held = self
            .thumb_drag
            .is_some_and(|drag| drag.pane_id == geometry.pane_id);
        let rect = geometry.surface();
        let (height, top) = thumb_geometry(scroll, rect.height - THUMB_INSET * 2.0);
        // Held, it fills out a little and brightens, so it reads as taken.
        let width = if held { 5.0 } else { 3.0 };
        Some(
            div()
                .absolute()
                .left(px(rect.x + rect.width - 4.0 - width))
                .top(px(rect.y + THUMB_INSET + top))
                .w(px(width))
                .h(px(height))
                .rounded_full()
                .bg(color(MUTED_TEXT).opacity(if held { 0.7 } else { 0.38 }))
                .into_any_element(),
        )
    }

    /// What a scrolled-back pane shows over its edges: the command whose
    /// output is in view, prompt ticks, the thumb, and the pill saying how
    /// much is below.
    pub(super) fn render_scrollback_marks(
        &self,
        geometry: layout::PaneGeometry,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        [
            self.render_command_header(geometry, cx),
            self.render_prompt_ticks(geometry),
            self.render_failure_marks(geometry, cx),
            self.render_scroll_thumb(geometry),
            self.render_latest_pill(geometry, cx),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    /// The commands run in the session: the focused pane's first, newest
    /// first, then the other panes'.
    pub(super) fn commands_ran(&self) -> Vec<CommandRan> {
        let focused = self.focused_pane_id();
        let mut ran: Vec<CommandRan> = focused
            .map(|pane_id| self.commands_ran_in(pane_id, None))
            .unwrap_or_default();
        // Then the session's other panes, tab by tab, each saying where.
        let Some(session) = self.session.as_ref() else {
            return ran;
        };
        let active = session.active_tab;
        for (index, tab) in session.tabs.iter().enumerate() {
            let mut pane_ids = Vec::new();
            tab.layout.pane_ids(&mut pane_ids);
            for pane_id in pane_ids
                .into_iter()
                .filter(|&pane_id| Some(pane_id) != focused)
            {
                let place = if tab.id == active {
                    "another pane".to_owned()
                } else {
                    format!("tab {}", index + 1)
                };
                ran.extend(self.commands_ran_in(pane_id, Some(&place)));
            }
        }
        ran
    }

    /// The commands run in one pane, newest first, each once, leaving out
    /// the prompt still waiting and empty lines.
    fn commands_ran_in(&self, pane_id: PaneId, place: Option<&str>) -> Vec<CommandRan> {
        let (Some(pane), Some(prompts)) =
            (self.panes.get(&pane_id), self.cached_prompt_rows(pane_id))
        else {
            return Vec::new();
        };
        let endings = pane.title.command_endings();
        let count = prompts.len();
        let mut seen = std::collections::HashSet::new();
        pane.with_engine(|engine| {
            prompts
                .iter()
                .enumerate()
                .rev()
                .skip(1)
                .take(COMMANDS_OFFERED)
                .filter_map(|(index, &row)| {
                    let line = engine.row_text(row).ok()?;
                    let command = command_of(&line);
                    // The same command again is offered once, at its newest.
                    (!command.is_empty() && seen.insert(command.to_owned())).then(|| CommandRan {
                        pane_id,
                        row,
                        command: command.to_owned(),
                        ending: ending_at(endings, count, index),
                        place: place.map(str::to_owned),
                    })
                })
                .collect()
        })
    }

    /// The prompt of the command whose line or output is under the pointer.
    pub(super) fn prompt_under(
        &self,
        pane_id: PaneId,
        rect: layout::Rect,
        position: gpui::Point<gpui::Pixels>,
    ) -> Option<usize> {
        let pane = self.panes.get(&pane_id)?;
        let point = self.selection_pointer(rect, &pane.frame, position).point?;
        let row = usize::try_from(pane.frame.scroll.offset).ok()? + usize::from(point.row);
        let prompt = prompt_owning(&self.cached_prompt_rows(pane_id)?, row)?;
        // A prompt nothing was typed at has no command to copy.
        let line = pane.with_engine(|engine| engine.row_text(prompt)).ok()?;
        (!command_of(&line).is_empty()).then_some(prompt)
    }

    /// Put on the clipboard the command run from a prompt, without the
    /// prompt.
    pub(super) fn copy_command(&mut self, pane_id: PaneId, prompt: usize, cx: &mut Context<Self>) {
        let Some(line) = self
            .panes
            .get(&pane_id)
            .and_then(|pane| pane.with_engine(|engine| engine.row_text(prompt)).ok())
        else {
            return;
        };
        let command = command_of(&line).to_owned();
        if command.is_empty() {
            self.say_in_strip("no command was run there", false, cx);
        } else if let Some(clipboard) = &mut self.clipboard
            && clipboard.set_text(command).is_ok()
        {
            self.say_in_strip("copied the command", false, cx);
        }
    }

    /// Put on the clipboard the command run from a prompt and what it
    /// printed, as a transcript to paste into an issue or a chat.
    pub(super) fn copy_transcript(
        &mut self,
        pane_id: PaneId,
        prompt: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(pane) = self.panes.get(&pane_id) else {
            return;
        };
        let prompts = pane
            .with_engine(|engine| engine.prompt_rows())
            .unwrap_or_default();
        let text = pane
            .with_engine(|engine| engine.screen_text())
            .unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        let Some(transcript) = transcript(&prompts, prompt, &lines) else {
            self.say_in_strip("no command was run there", false, cx);
            return;
        };
        if let Some(clipboard) = &mut self.clipboard
            && clipboard.set_text(transcript).is_ok()
        {
            self.say_in_strip("copied the command and its output", false, cx);
        }
    }

    /// Put on the clipboard what the command run from a prompt printed.
    pub(super) fn copy_output(&mut self, pane_id: PaneId, prompt: usize, cx: &mut Context<Self>) {
        let Some(pane) = self.panes.get(&pane_id) else {
            return;
        };
        let prompts = pane
            .with_engine(|engine| engine.prompt_rows())
            .unwrap_or_default();
        let text = pane
            .with_engine(|engine| engine.screen_text())
            .unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        let Some((first, last)) = output_rows(&prompts, prompt, &lines) else {
            self.say_in_strip("that command printed nothing", false, cx);
            return;
        };
        let output = lines[first..=last]
            .iter()
            .map(|line| line.trim_end())
            .collect::<Vec<_>>()
            .join("\n");
        let count = last - first + 1;
        if let Some(clipboard) = &mut self.clipboard
            && clipboard.set_text(output).is_ok()
        {
            let copied = if count == 1 {
                "copied 1 line of output".to_owned()
            } else {
                format!("copied {count} lines of output")
            };
            self.say_in_strip(copied, false, cx);
        }
    }

    /// Write all a pane holds, its history and screen, to a text file and
    /// open it in the system's text editor, to read, search or keep.
    pub(super) fn open_history_in_editor(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        let Some(text) = self
            .panes
            .get(&pane_id)
            .and_then(|pane| pane.with_engine(|engine| engine.screen_text()).ok())
        else {
            return;
        };
        let text = history_text(&text);
        let lines = text.lines().count();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        let path = std::env::temp_dir().join(format!("mux-history-{stamp}.txt"));
        let opened = std::fs::write(&path, text).and_then(|()| {
            let mut command = if cfg!(target_os = "macos") {
                let mut open = std::process::Command::new("/usr/bin/open");
                open.arg("-t");
                open
            } else {
                std::process::Command::new("xdg-open")
            };
            command.arg(&path).spawn().map(|_| ())
        });
        match opened {
            Ok(()) => {
                let words = if lines == 1 {
                    "opened 1 line in your editor".to_owned()
                } else {
                    format!("opened {lines} lines in your editor")
                };
                self.say_in_strip(words, false, cx);
            }
            Err(error) => {
                error!(%pane_id, %error, "could not open the pane's history");
                self.say_in_strip("could not open the history in an editor", true, cx);
            }
        }
    }

    /// Bring a command's prompt to the top of its pane, and the keys there.
    pub(super) fn go_to_command(&mut self, pane_id: PaneId, row: usize, cx: &mut Context<Self>) {
        let Some(offset) = self
            .panes
            .get(&pane_id)
            .map(|pane| pane.frame.scroll.offset)
        else {
            return;
        };
        if let Some(session) = self.session.as_ref()
            && let Some(tab) = session.tabs.iter().find(|tab| tab.layout.contains(pane_id))
            && tab.id != session.active_tab
        {
            self.send_workspace(WorkspaceCommand::SelectTab(tab.id));
        }
        self.request_pane_focus(pane_id);
        let delta =
            i64::try_from(row).unwrap_or(i64::MAX) - i64::try_from(offset).unwrap_or(i64::MAX);
        self.scroll_viewport(pane_id, TerminalViewportScroll::Delta(delta));
        self.flash_prompt(pane_id, row, cx);
    }

    /// The pane's prompt rows, read again only when its screen has changed.
    pub(super) fn cached_prompt_rows(&self, pane_id: PaneId) -> Option<Rc<Vec<usize>>> {
        let pane = self.panes.get(&pane_id)?;
        let mut cache = self.prompt_rows.borrow_mut();
        if let Some((serial, rows)) = cache.get(&pane_id)
            && *serial == pane.frame_serial
        {
            return Some(Rc::clone(rows));
        }
        let rows = Rc::new(
            pane.with_engine(|engine| engine.prompt_rows())
                .unwrap_or_default(),
        );
        cache.insert(pane_id, (pane.frame_serial, Rc::clone(&rows)));
        Some(rows)
    }

    /// While a pane is scrolled back into a command's output with its
    /// prompt above the view, that prompt line pinned to the pane's top, so
    /// the output in view says what made it. A click goes to the prompt.
    pub(super) fn render_command_header(
        &self,
        geometry: layout::PaneGeometry,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let pane_id = geometry.pane_id;
        let pane = self.panes.get(&pane_id)?;
        let scroll = pane.frame.scroll;
        let finding = self
            .find
            .as_ref()
            .is_some_and(|find| find.pane_id == pane_id);
        if finding || !scroll.is_scrolled() {
            return None;
        }
        let prompts = self.cached_prompt_rows(pane_id)?;
        let row = command_above(&prompts, scroll)?;
        let ending = prompts
            .iter()
            .position(|&prompt| prompt == row)
            .map(|index| ending_at(pane.title.command_endings(), prompts.len(), index))
            .unwrap_or_default();
        let text = pane
            .with_engine(|engine| engine.row_text(row))
            .ok()
            .filter(|text| !text.trim().is_empty())?;
        let metrics = self.metrics;
        let rect = geometry.rect;
        let padding =
            metrics.balanced_padding(rect.width, rect.height, pane.frame.cols, pane.frame.rows);
        let delta = i64::try_from(row).unwrap_or(i64::MAX)
            - i64::try_from(scroll.offset).unwrap_or(i64::MAX);
        Some(
            h_flex()
                .id(SharedString::from(format!("command-header-{pane_id}")))
                .absolute()
                .left(px(rect.x))
                .top(px(rect.y))
                .w(px(rect.width - THUMB_GRIP))
                .h(px(padding.top + metrics.cell_height))
                .pt(px(padding.top))
                .pl(px(padding.left))
                .pr(px(8.0))
                .items_center()
                .overflow_hidden()
                .bg(color(SURFACE))
                .border_b_1()
                .border_color(hairline(0.12))
                .shadow_sm()
                .block_mouse_except_scroll()
                .cursor_pointer()
                .font_family(self.terminal_font.clone())
                .text_size(px(metrics.font_size))
                .text_color(color(MUTED_TEXT))
                .whitespace_nowrap()
                .hover(|style| style.text_color(color(TEXT)))
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.scroll_viewport(pane_id, TerminalViewportScroll::Delta(delta)) {
                        this.flash_prompt(pane_id, row, cx);
                        cx.notify();
                    }
                }))
                .child(div().flex_1().overflow_hidden().child(text))
                .children(ending_words(ending).map(|words| {
                    div()
                        .flex_none()
                        .pl(px(12.0))
                        .text_color(color(if ending.failed().is_some() {
                            SIGNAL
                        } else {
                            FAINT_TEXT
                        }))
                        .child(words)
                }))
                .into_any_element(),
        )
    }

    /// A slim peach bar in the left margin beside the prompt of each
    /// command on screen that failed, as iTerm marks them, so a failure
    /// among many commands is seen at a glance. Hovering one says how the
    /// command ended, and a click selects what it printed.
    fn render_failure_marks(
        &self,
        geometry: layout::PaneGeometry,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let pane_id = geometry.pane_id;
        let pane = self.panes.get(&pane_id)?;
        let prompts = self.cached_prompt_rows(pane_id)?;
        let endings = pane.title.command_endings();
        let frame = &pane.frame;
        let top = usize::try_from(frame.scroll.offset).ok()?;
        let rect = geometry.rect;
        let padding =
            self.metrics
                .balanced_padding(rect.width, rect.height, frame.cols, frame.rows);
        let height = self.metrics.cell_height;
        let marks = prompts
            .iter()
            .enumerate()
            .map(|(index, &row)| (row, ending_at(endings, prompts.len(), index)))
            .filter(|(_, ending)| ending.failed().is_some())
            .filter_map(|(prompt, ending)| {
                let row = prompt.checked_sub(top)?;
                (row < usize::from(frame.rows)).then_some((prompt, row, ending))
            })
            .map(|(prompt, row, ending)| {
                let words = ending_words(ending).unwrap_or_default();
                // The bar is slim; the area that takes the pointer is wider.
                div()
                    .id(SharedString::from(format!(
                        "failure-mark-{pane_id}-{prompt}"
                    )))
                    .absolute()
                    .left(px(rect.x + (padding.left - 7.0).max(0.0)))
                    .top(px(rect.y + padding.top + height * row as f32))
                    .w(px(8.0))
                    .h(px(height))
                    .pl(px(3.0))
                    .py(px(2.0))
                    .cursor_pointer()
                    .tooltip(move |_, cx| {
                        super::strip_tooltip(
                            format!("{words} · click to select its output"),
                            None,
                            cx,
                        )
                    })
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_output_of(pane_id, prompt, cx);
                        cx.notify();
                    }))
                    .child(
                        div()
                            .w(px(2.0))
                            .h_full()
                            .rounded_full()
                            .bg(color(SIGNAL).opacity(0.85)),
                    )
            })
            .collect::<Vec<_>>();
        (!marks.is_empty()).then(|| {
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .children(marks)
                .into_any_element()
        })
    }

    pub(super) fn render_prompt_ticks(
        &self,
        geometry: layout::PaneGeometry,
    ) -> Option<gpui::AnyElement> {
        let pane_id = geometry.pane_id;
        let finding = self
            .find
            .as_ref()
            .is_some_and(|find| find.pane_id == pane_id);
        if finding || !self.scroll_thumb_shown(pane_id) {
            return None;
        }
        let pane = self.panes.get(&pane_id)?;
        let prompts = self.cached_prompt_rows(pane_id)?;
        if prompts.is_empty() {
            return None;
        }
        let total = pane.frame.scroll.total.max(1) as f32;
        let rect = geometry.surface();
        let track = rect.height - THUMB_INSET * 2.0;
        let mut ticks = div()
            .absolute()
            .left(px(rect.x + rect.width - 13.0))
            .top(px(rect.y + THUMB_INSET))
            .w(px(4.0))
            .h(px(track));
        // One tick to a pixel row or two is all the edge can show.
        let endings = pane.title.command_endings();
        let mut last = f32::NEG_INFINITY;
        for (index, &row) in prompts.iter().enumerate() {
            let y = (row as f32 + 0.5) / total * track;
            // A failure always shows, in the colour progress fails in.
            let failed = ending_at(endings, prompts.len(), index).failed().is_some();
            if y - last < 2.0 && !failed {
                continue;
            }
            last = y;
            let tint = if failed {
                color(SIGNAL).opacity(0.9)
            } else {
                color(MUTED_TEXT).opacity(0.45)
            };
            ticks = ticks.child(
                div()
                    .absolute()
                    .top(px((y - 0.75).clamp(0.0, track - 1.5)))
                    .left_0()
                    .w(px(4.0))
                    .h(px(1.5))
                    .rounded_full()
                    .bg(tint),
            );
        }
        Some(ticks.into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keystroke(key: &str, platform: bool, shift: bool) -> gpui::Keystroke {
        gpui::Keystroke {
            modifiers: gpui::Modifiers {
                platform,
                shift,
                ..gpui::Modifiers::default()
            },
            key: key.to_owned(),
            key_char: None,
        }
    }

    #[test]
    fn history_keys_move_the_view_and_leave_programs_their_own_keys() {
        assert_eq!(
            scrollback_key(&keystroke("end", true, false), 40, true),
            Some(TerminalViewportScroll::Bottom)
        );
        assert_eq!(
            scrollback_key(&keystroke("pageup", false, true), 40, true),
            Some(TerminalViewportScroll::Delta(-39))
        );
        // A full-screen program has no history; its ⇧page keys are its own.
        assert_eq!(
            scrollback_key(&keystroke("pageup", false, true), 40, false),
            None
        );
        // Plain page keys always reach the program.
        assert_eq!(
            scrollback_key(&keystroke("pagedown", false, false), 40, true),
            None
        );
    }

    #[test]
    fn command_arrows_step_through_the_prompts() {
        let prompts = [10, 120, 300, 470];
        let scroll = |offset| TerminalScrollState {
            total: 500,
            offset,
            len: 40,
        };
        let delta = |rows, to| (TerminalViewportScroll::Delta(rows), Some(to));
        assert_eq!(
            prompt_jump(&prompts, scroll(460), true),
            Some(delta(-160, 300))
        );
        assert_eq!(
            prompt_jump(&prompts, scroll(300), true),
            Some(delta(-180, 120))
        );
        assert_eq!(prompt_jump(&prompts, scroll(10), true), None);
        assert_eq!(
            prompt_jump(&prompts, scroll(120), false),
            Some(delta(180, 300))
        );
        assert_eq!(
            prompt_jump(&[10, 120], scroll(120), false),
            Some((TerminalViewportScroll::Bottom, None)),
            "past the newest prompt is the latest output"
        );
        assert_eq!(prompt_jump(&[10, 120], scroll(460), false), None);
        assert_eq!(prompt_key(&keystroke("up", true, false)), Some(true));
        assert_eq!(prompt_key(&keystroke("down", true, true)), Some(false));
        assert_eq!(prompt_key(&keystroke("up", false, false)), None);
    }

    #[test]
    fn the_header_names_the_command_whose_output_is_in_view() {
        let prompts = [10, 120, 300];
        let scroll = |offset| TerminalScrollState {
            total: 500,
            offset,
            len: 40,
        };
        assert_eq!(
            command_above(&prompts, scroll(5)),
            None,
            "before any prompt"
        );
        assert_eq!(
            command_above(&prompts, scroll(10)),
            None,
            "on the prompt itself"
        );
        assert_eq!(command_above(&prompts, scroll(11)), Some(10));
        assert_eq!(command_above(&prompts, scroll(299)), Some(120));
        assert_eq!(command_above(&prompts, scroll(460)), Some(300));
    }

    #[test]
    fn endings_line_up_with_the_prompts_from_the_newest() {
        let ended = |status, seconds| CommandEnding {
            status: Some(status),
            took: Some(std::time::Duration::from_secs(seconds)),
        };
        let endings: std::collections::VecDeque<_> = [
            ended(1, 0),
            ended(0, 200),
            ended(2, 75),
            CommandEnding::default(),
        ]
        .into();
        let words = |index| ending_words(ending_at(&endings, 5, index));
        // Five prompts on screen, the oldest from before the shell's marks
        // were followed.
        assert_eq!(words(0), None);
        assert_eq!(words(1).as_deref(), Some("exit 1"), "too quick to time");
        assert_eq!(words(2).as_deref(), Some("3m"), "it succeeded");
        assert_eq!(words(3).as_deref(), Some("exit 2 · 1m"));
        assert_eq!(words(4), None, "still at its prompt");
    }

    #[test]
    fn the_command_is_what_follows_the_prompt() {
        assert_eq!(command_of("root@vm:~# echo a > b"), "echo a > b");
        assert_eq!(command_of("~/src $ cargo test"), "cargo test");
        assert_eq!(command_of("❯ ls"), "ls");
        assert_eq!(command_of("ryan@mac src % "), "");
        assert_eq!(command_of("plain"), "plain");
        assert_eq!(command_of("root@vm:~#"), "", "its space trimmed");
        assert_eq!(command_of("~/src $ make >"), "make >");
    }

    #[test]
    fn a_row_belongs_to_the_command_above_it() {
        let prompts = [2, 6];
        assert_eq!(prompt_owning(&prompts, 1), None);
        assert_eq!(prompt_owning(&prompts, 2), Some(2));
        assert_eq!(prompt_owning(&prompts, 5), Some(2));
        assert_eq!(prompt_owning(&prompts, 9), Some(6));
        let lines = ["", "", "$ ls", "a", "b", "", "$ true", "$ "];
        assert_eq!(output_rows(&prompts, 2, &lines), Some((3, 4)));
        assert_eq!(output_rows(&prompts, 6, &lines), Some((7, 7)));
        assert_eq!(output_rows(&[2, 3], 2, &lines), None, "printed nothing");
        assert_eq!(
            transcript(&prompts, 2, &lines).as_deref(),
            Some("$ ls\na\nb")
        );
        assert_eq!(transcript(&[2, 3], 2, &lines).as_deref(), Some("$ ls"));
        assert_eq!(transcript(&prompts, 7, &lines), None, "nothing typed");
    }

    #[test]
    fn history_is_written_without_trailing_blanks() {
        assert_eq!(history_text("$ ls   \na b\n\n\n   \n"), "$ ls\na b\n");
    }

    #[test]
    fn the_pill_counts_the_rows_below_the_view() {
        let scroll = |offset| TerminalScrollState {
            total: 500,
            offset,
            len: 40,
        };
        assert_eq!(rows_below(scroll(460)), None, "at the latest output");
        assert_eq!(rows_below(scroll(406)), Some(54));
        assert_eq!(rows_below(scroll(0)), Some(460));
    }

    #[test]
    fn the_thumb_sits_where_the_view_is() {
        let at = |offset| TerminalScrollState {
            total: 400,
            offset,
            len: 40,
        };
        let (height, top) = thumb_geometry(at(0), 400.0);
        assert!((height - 40.0).abs() < 0.01);
        assert!(top.abs() < 0.01);
        let (_, top) = thumb_geometry(at(360), 400.0);
        assert!((top - 360.0).abs() < 0.01);
        // A long history keeps the thumb big enough to see.
        let (height, _) = thumb_geometry(
            TerminalScrollState {
                total: 100_000,
                offset: 0,
                len: 40,
            },
            400.0,
        );
        assert!((height - 24.0).abs() < 0.01);
    }

    #[test]
    fn a_dragged_thumb_puts_the_view_where_it_is() {
        let scroll = TerminalScrollState {
            total: 400,
            offset: 0,
            len: 40,
        };
        // A 40-row view of 400 on a 400-point track: a 40-point thumb with
        // 360 points to travel, a row for each point.
        assert_eq!(offset_for_thumb(scroll, 400.0, 0.0), 0);
        assert_eq!(offset_for_thumb(scroll, 400.0, 180.0), 180);
        assert_eq!(offset_for_thumb(scroll, 400.0, 360.0), 360);
        // Past either end it stops there.
        assert_eq!(offset_for_thumb(scroll, 400.0, -50.0), 0);
        assert_eq!(offset_for_thumb(scroll, 400.0, 900.0), 360);
    }
}
