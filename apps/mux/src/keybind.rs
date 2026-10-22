//! `keybind` lines from the user's Ghostty config, as Ghostty reads them:
//! `keybind = shift+enter=text:\x1b\r` types those bytes into the pane,
//! `csi:` and `esc:` send an escape sequence, `unbind` gives a key of Mux's
//! back to the program, `ignore` swallows it, and Ghostty's own actions
//! (`new_split:right`, `goto_tab:3`, `jump_to_prompt:-1` and the rest) do
//! what Mux's keys for them do. A bind wins over Mux's key for the same
//! keys. Sequences (`ctrl+a>n`) and `global:` binds are Ghostty's alone.

use gpui::{App, KeyBinding, Keystroke, NoAction};

use super::*;

/// A key bound in the user's Ghostty config, by its keys in GPUI's
/// spelling, so the handler finds what it does since the last reload.
#[derive(Clone, PartialEq, gpui::Action)]
#[action(namespace = mux_agent, no_json)]
pub(super) struct GhosttyKeybind {
    trigger: String,
}

/// What a bound key does.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Bound {
    Write(Vec<u8>),
    Run(Command),
    Unbind,
    Ignore,
}

/// Ghostty's actions that Mux has a key for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    NewTab,
    SplitRight,
    SplitDown,
    SplitAuto,
    ClosePane,
    CloseTab,
    Focus(Direction),
    NextPane,
    PreviousPane,
    Resize(Direction),
    Equalize,
    Zoom,
    NextTab,
    PreviousTab,
    GotoTab(u8),
    LastTab,
    GrowText,
    ShrinkText,
    ResetText,
    Reload,
    FullScreen,
    Clear,
    Quit,
    Palette,
    Settings,
    Find,
    FindSelection,
    Copy,
    Paste,
    PasteSelection,
    SelectAll,
    Scroll(TerminalViewportScroll),
    ScrollPage(bool),
    Prompt(bool),
}

static KEYBINDS: ghostty_config::Reloadable<Vec<(String, Bound)>> =
    ghostty_config::Reloadable::new(read_keybinds);

fn read_keybinds() -> Vec<(String, Bound)> {
    mux_terminal_ghostty::user_keybinds()
        .iter()
        .filter_map(|line| {
            let parsed = parse_keybind(line);
            if parsed.is_none() {
                warn!(keybind = %line, "Ghostty keybind not understood; left to Mux");
            }
            parsed
        })
        .collect()
}

/// What the settings say of the user's keybinds: how many Mux follows, and
/// the triggers of any it leaves to Ghostty. None when there are none.
pub(super) fn keybind_summary() -> Option<String> {
    summarize(&mux_terminal_ghostty::user_keybinds())
}

fn summarize(lines: &[String]) -> Option<String> {
    if lines.is_empty() {
        return None;
    }
    let left: Vec<&str> = lines
        .iter()
        .filter(|line| parse_keybind(line).is_none())
        .map(|line| line.split('=').next().unwrap_or(line).trim())
        .collect();
    let used = lines.len() - left.len();
    let followed = if used == 1 {
        "1 followed".to_owned()
    } else {
        format!("{used} followed")
    };
    let summary = if left.is_empty() {
        followed
    } else {
        format!("{followed}; left to Ghostty: {}", left.join(", "))
    };
    Some(summary)
}

/// Bind the keys the user's Ghostty config binds, over Mux's own. Run
/// after Mux's bindings so these win, and again after a reload.
pub(super) fn bind_ghostty_keys(cx: &mut App) {
    let bindings: Vec<KeyBinding> = KEYBINDS
        .get()
        .into_iter()
        .map(|(trigger, bound)| {
            if bound == Bound::Unbind {
                KeyBinding::new(&trigger, NoAction, None)
            } else {
                let action = GhosttyKeybind {
                    trigger: trigger.clone(),
                };
                KeyBinding::new(&trigger, action, None)
            }
        })
        .collect();
    cx.bind_keys(bindings);
}

/// Whether the user's Ghostty config unbinds `keystroke`, giving the key
/// back to the program over a default of Ghostty's that Mux keeps too.
pub(super) fn unbinds(keystroke: &Keystroke) -> bool {
    // A Mac's arrows carry the function modifier, which triggers leave out.
    let held = |modifiers: gpui::Modifiers| gpui::Modifiers {
        function: false,
        ..modifiers
    };
    KEYBINDS.get().into_iter().any(|(trigger, bound)| {
        bound == Bound::Unbind
            && Keystroke::parse(&trigger).is_ok_and(|unbound| {
                unbound.key == keystroke.key && held(unbound.modifiers) == held(keystroke.modifiers)
            })
    })
}

impl MuxApp {
    pub(super) fn run_ghostty_keybind(
        &mut self,
        bind: &GhosttyKeybind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let bound = KEYBINDS
            .get()
            .into_iter()
            .rev()
            .find(|(trigger, _)| *trigger == bind.trigger)
            .map(|(_, bound)| bound);
        match bound {
            Some(Bound::Write(bytes)) => self.write_bound_bytes(&bytes, cx),
            Some(Bound::Run(command)) => self.run_bound_command(command, window, cx),
            Some(Bound::Ignore | Bound::Unbind) => {}
            // Taken out of the config since the last reload: the key is
            // the pane's again.
            None => {
                if let Ok(keystroke) = Keystroke::parse(&bind.trigger) {
                    self.send_terminal_key_down(&keystroke, false, false, cx);
                }
            }
        }
    }

    fn write_bound_bytes(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        let Some(pane_id) = self.terminal_input_pane_id() else {
            return;
        };
        self.return_to_latest(pane_id, cx);
        self.clear_selection_on_typing(pane_id);
        for target in self.key_targets(pane_id) {
            self.note_pane_key(target);
            self.backend.send(CommandMessage::Write {
                pane_id: target,
                bytes: bytes.to_vec(),
            });
        }
    }

    fn run_bound_command(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        let action: Box<dyn gpui::Action> = match command {
            Command::Copy => return self.copy_selection(cx),
            Command::Paste => return self.paste_clipboard(cx),
            Command::PasteSelection => return self.paste_selection_clipboard(cx),
            Command::SelectAll => {
                self.select_all();
                return cx.notify();
            }
            Command::Scroll(movement) => return self.scroll_input_pane(movement, cx),
            Command::ScrollPage(up) => {
                let rows = self
                    .terminal_input_pane_id()
                    .and_then(|pane_id| self.panes.get(&pane_id))
                    .map_or(1, |pane| pane.frame.rows);
                let page = i64::from(rows.saturating_sub(1).max(1));
                let movement = TerminalViewportScroll::Delta(if up { -page } else { page });
                return self.scroll_input_pane(movement, cx);
            }
            Command::Prompt(older) => return self.jump_to_prompt(older, cx),
            Command::GotoTab(index) => {
                return self.run_shortcut(Action::SelectTab(index), window, cx);
            }
            Command::Focus(direction) => {
                return self.run_shortcut(Action::FocusPane(direction), window, cx);
            }
            Command::Resize(direction) => {
                return self.run_shortcut(Action::ResizePane(direction), window, cx);
            }
            Command::NewTab => Box::new(NewMuxTab),
            Command::SplitRight => Box::new(SplitMuxRight),
            Command::SplitDown => Box::new(SplitMuxDown),
            Command::SplitAuto => Box::new(SplitMuxAuto),
            Command::ClosePane => Box::new(CloseMuxPane),
            Command::CloseTab => Box::new(CloseMuxTab),
            Command::NextPane => Box::new(NextMuxPane),
            Command::PreviousPane => Box::new(PreviousMuxPane),
            Command::Equalize => Box::new(EqualizeMuxPanes),
            Command::Zoom => Box::new(ZoomMuxPane),
            Command::NextTab => Box::new(NextMuxTab),
            Command::PreviousTab => Box::new(PreviousMuxTab),
            Command::LastTab => Box::new(SelectLastMuxTab),
            Command::GrowText => Box::new(GrowMuxText),
            Command::ShrinkText => Box::new(ShrinkMuxText),
            Command::ResetText => Box::new(ResetMuxText),
            Command::Reload => Box::new(ReloadMuxConfig),
            Command::FullScreen => Box::new(ToggleMuxFullScreen),
            Command::Clear => Box::new(ClearMuxPane),
            Command::Quit => Box::new(QuitMux),
            Command::Palette => Box::new(OpenMuxPalette),
            Command::Settings => Box::new(OpenMuxSettings),
            Command::Find => Box::new(OpenFind),
            Command::FindSelection => Box::new(FindSelection),
        };
        window.dispatch_action(action, cx);
    }
}

/// One `keybind` value, `trigger=action`, as the keys in GPUI's spelling
/// and what they do. None for what Mux cannot bind.
fn parse_keybind(line: &str) -> Option<(String, Bound)> {
    // The trigger ends at the first `=` that is not itself the key, as in
    // `ctrl+==ignore`.
    let split = line
        .char_indices()
        .find(|&(index, character)| character == '=' && index > 0 && !line[..index].ends_with('+'))
        .map(|(index, _)| index)?;
    let trigger = gpui_trigger(line[..split].trim())?;
    let bound = parse_action(line[split + 1..].trim())?;
    Some((trigger, bound))
}

/// Ghostty's spelling of a key and its modifiers, `super+shift+arrow_up`,
/// in GPUI's, `cmd-shift-up`.
fn gpui_trigger(trigger: &str) -> Option<String> {
    let mut trigger = trigger;
    while let Some(rest) = ["all:", "unconsumed:", "performable:"]
        .iter()
        .find_map(|prefix| trigger.strip_prefix(prefix))
    {
        trigger = rest;
    }
    if trigger.starts_with("global:") {
        return None;
    }
    let (modifiers, key) = match trigger.strip_suffix("++") {
        Some(modifiers) => (modifiers, "+"),
        None if trigger == "+" => ("", "+"),
        None => trigger.rsplit_once('+').unwrap_or(("", trigger)),
    };
    if key.is_empty() || (key.len() > 1 && key.contains('>')) {
        return None;
    }
    let mut parts = Vec::new();
    for modifier in modifiers.split('+').filter(|part| !part.is_empty()) {
        parts.push(match modifier.to_ascii_lowercase().as_str() {
            "shift" => "shift",
            "ctrl" | "control" => "ctrl",
            "alt" | "opt" | "option" => "alt",
            "super" | "cmd" | "command" => "cmd",
            _ => return None,
        });
    }
    let key = gpui_key(key)?;
    let mut spelled = parts.join("-");
    if !spelled.is_empty() {
        spelled.push('-');
    }
    spelled.push_str(&key);
    Keystroke::parse(&spelled).ok()?;
    Some(spelled)
}

fn gpui_key(key: &str) -> Option<String> {
    let lower = key.to_ascii_lowercase();
    let lower = lower.strip_prefix("key_").unwrap_or(&lower);
    let lower = lower.strip_prefix("digit_").unwrap_or(lower);
    let lower = lower.strip_prefix("arrow_").unwrap_or(lower);
    let named = match lower {
        "return" | "enter" => "enter",
        "tab" => "tab",
        "escape" | "esc" => "escape",
        "backspace" => "backspace",
        "delete" => "delete",
        "insert" => "insert",
        "space" => "space",
        "home" => "home",
        "end" => "end",
        "page_up" | "pageup" => "pageup",
        "page_down" | "pagedown" => "pagedown",
        "up" | "down" | "left" | "right" => lower,
        "minus" => "-",
        "equal" => "=",
        "plus" => "+",
        "bracket_left" | "left_bracket" => "[",
        "bracket_right" | "right_bracket" => "]",
        "comma" => ",",
        "period" => ".",
        "slash" => "/",
        "semicolon" => ";",
        "quote" | "apostrophe" => "'",
        "backslash" => "\\",
        "backquote" | "grave_accent" => "`",
        "zero" => "0",
        "one" => "1",
        "two" => "2",
        "three" => "3",
        "four" => "4",
        "five" => "5",
        "six" => "6",
        "seven" => "7",
        "eight" => "8",
        "nine" => "9",
        _ => "",
    };
    if !named.is_empty() {
        return Some(named.to_owned());
    }
    let function_key = lower
        .strip_prefix('f')
        .and_then(|number| number.parse::<u8>().ok())
        .is_some_and(|number| (1..=24).contains(&number));
    if function_key {
        return Some(lower.to_owned());
    }
    let mut characters = lower.chars();
    match (characters.next(), characters.next()) {
        (Some(character), None) if !character.is_whitespace() => {
            Some(character.to_lowercase().collect())
        }
        _ => None,
    }
}

fn parse_action(action: &str) -> Option<Bound> {
    let (name, parameter) = action.split_once(':').unwrap_or((action, ""));
    let direction = |parameter: &str| match parameter.split(',').next()? {
        "left" => Some(Direction::Left),
        "right" => Some(Direction::Right),
        "up" | "top" => Some(Direction::Up),
        "down" | "bottom" => Some(Direction::Down),
        _ => None,
    };
    let command = match name {
        "text" => return Some(Bound::Write(unescape(parameter)?)),
        "csi" => return Some(Bound::Write([b"\x1b[", parameter.as_bytes()].concat())),
        "esc" => return Some(Bound::Write([b"\x1b", parameter.as_bytes()].concat())),
        "unbind" => return Some(Bound::Unbind),
        "ignore" => return Some(Bound::Ignore),
        "new_tab" => Command::NewTab,
        "new_split" => match parameter {
            "down" | "up" => Command::SplitDown,
            "auto" => Command::SplitAuto,
            _ => Command::SplitRight,
        },
        "close_surface" => Command::ClosePane,
        "close_tab" if matches!(parameter, "" | "this") => Command::CloseTab,
        "goto_split" => match parameter {
            "next" => Command::NextPane,
            "previous" => Command::PreviousPane,
            _ => Command::Focus(direction(parameter)?),
        },
        "resize_split" => Command::Resize(direction(parameter)?),
        "equalize_splits" => Command::Equalize,
        "toggle_split_zoom" => Command::Zoom,
        "next_tab" => Command::NextTab,
        "previous_tab" => Command::PreviousTab,
        "last_tab" => Command::LastTab,
        "goto_tab" => Command::GotoTab(parameter.parse().ok().filter(|&tab| tab > 0)?),
        "increase_font_size" => Command::GrowText,
        "decrease_font_size" => Command::ShrinkText,
        "reset_font_size" => Command::ResetText,
        "reload_config" => Command::Reload,
        "open_config" => Command::Settings,
        "toggle_fullscreen" | "toggle_native_fullscreen" => Command::FullScreen,
        "clear_screen" => Command::Clear,
        "quit" => Command::Quit,
        "toggle_command_palette" => Command::Palette,
        "start_search" => Command::Find,
        "search_selection" => Command::FindSelection,
        "copy_to_clipboard" => Command::Copy,
        "paste_from_clipboard" => Command::Paste,
        "paste_from_selection" => Command::PasteSelection,
        "select_all" => Command::SelectAll,
        "scroll_to_top" => Command::Scroll(TerminalViewportScroll::Top),
        "scroll_to_bottom" => Command::Scroll(TerminalViewportScroll::Bottom),
        "scroll_page_up" => Command::ScrollPage(true),
        "scroll_page_down" => Command::ScrollPage(false),
        "jump_to_prompt" => Command::Prompt(parameter.trim().starts_with('-')),
        _ => return None,
    };
    Some(Bound::Run(command))
}

/// A `text:` binding's text, with Zig's escapes as Ghostty reads them:
/// `\n`, `\r`, `\t`, `\\`, `\"`, `\'`, `\xNN` and `\u{N}`.
fn unescape(text: &str) -> Option<Vec<u8>> {
    let mut bytes = Vec::with_capacity(text.len());
    let mut characters = text.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            let mut buffer = [0; 4];
            bytes.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
            continue;
        }
        match characters.next()? {
            'n' => bytes.push(b'\n'),
            'r' => bytes.push(b'\r'),
            't' => bytes.push(b'\t'),
            '\\' => bytes.push(b'\\'),
            '"' => bytes.push(b'"'),
            '\'' => bytes.push(b'\''),
            'x' => {
                let hex: String = characters.by_ref().take(2).collect();
                bytes.push(u8::from_str_radix(&hex, 16).ok()?);
            }
            'u' => {
                if characters.next()? != '{' {
                    return None;
                }
                let hex: String = characters.by_ref().take_while(|&c| c != '}').collect();
                let character = char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?;
                let mut buffer = [0; 4];
                bytes.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
            }
            _ => return None,
        }
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triggers_are_spelled_as_gpui_spells_them() {
        assert_eq!(gpui_trigger("shift+enter").as_deref(), Some("shift-enter"));
        assert_eq!(
            gpui_trigger("super+shift+arrow_up").as_deref(),
            Some("cmd-shift-up")
        );
        assert_eq!(gpui_trigger("ctrl+key_a").as_deref(), Some("ctrl-a"));
        assert_eq!(gpui_trigger("cmd+digit_1").as_deref(), Some("cmd-1"));
        assert_eq!(gpui_trigger("opt+left_bracket").as_deref(), Some("alt-["));
        assert_eq!(gpui_trigger("ctrl+=").as_deref(), Some("ctrl-="));
        assert_eq!(gpui_trigger("cmd+minus").as_deref(), Some("cmd--"));
        assert_eq!(gpui_trigger("all:cmd+k").as_deref(), Some("cmd-k"));
        assert_eq!(gpui_trigger("f5").as_deref(), Some("f5"));
        assert_eq!(gpui_trigger("global:cmd+grave_accent"), None);
        assert_eq!(gpui_trigger("ctrl+a>n"), None, "sequences are Ghostty's");
        assert_eq!(gpui_trigger("hyper+a"), None);
    }

    #[test]
    fn keybinds_type_text_or_run_what_mux_has() {
        assert_eq!(
            parse_keybind("shift+enter=text:\\x1b\\r"),
            Some(("shift-enter".to_owned(), Bound::Write(b"\x1b\r".to_vec())))
        );
        assert_eq!(
            parse_keybind("ctrl+==ignore"),
            Some(("ctrl-=".to_owned(), Bound::Ignore))
        );
        assert_eq!(
            parse_keybind("cmd+d=new_split:down"),
            Some(("cmd-d".to_owned(), Bound::Run(Command::SplitDown)))
        );
        assert_eq!(
            parse_keybind("super+alt+w=close_tab:this"),
            Some(("cmd-alt-w".to_owned(), Bound::Run(Command::CloseTab)))
        );
        assert_eq!(parse_keybind("super+alt+w=close_tab:other"), None);
        assert_eq!(
            parse_keybind("cmd+k=unbind"),
            Some(("cmd-k".to_owned(), Bound::Unbind))
        );
        assert_eq!(
            parse_keybind("alt+left=esc:b"),
            Some(("alt-left".to_owned(), Bound::Write(b"\x1bb".to_vec())))
        );
        assert_eq!(
            parse_keybind("cmd+up=jump_to_prompt:-1"),
            Some(("cmd-up".to_owned(), Bound::Run(Command::Prompt(true))))
        );
        assert_eq!(
            parse_keybind("cmd+3=goto_tab:3"),
            Some(("cmd-3".to_owned(), Bound::Run(Command::GotoTab(3))))
        );
        assert_eq!(parse_keybind("cmd+j=toggle_quick_terminal"), None);
        assert_eq!(parse_keybind("no action"), None);
    }

    #[test]
    fn the_settings_name_the_binds_left_to_ghostty() {
        assert_eq!(summarize(&[]), None);
        let lines = [
            "shift+enter=text:\\r".to_owned(),
            "ctrl+a>n=new_tab".to_owned(),
            "cmd+j=toggle_quick_terminal".to_owned(),
        ];
        assert_eq!(
            summarize(&lines).as_deref(),
            Some("1 followed; left to Ghostty: ctrl+a>n, cmd+j")
        );
    }

    #[test]
    fn text_escapes_are_read_as_zig_writes_them() {
        assert_eq!(unescape("a\\nb").as_deref(), Some(b"a\nb".as_slice()));
        assert_eq!(unescape("\\x1b[A").as_deref(), Some(b"\x1b[A".as_slice()));
        assert_eq!(unescape("\\u{e9}").as_deref(), Some("é".as_bytes()));
        assert_eq!(unescape("\\\\").as_deref(), Some(b"\\".as_slice()));
        assert_eq!(unescape("\\q"), None);
        assert_eq!(unescape("trailing\\"), None);
    }
}
