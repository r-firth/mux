//! Letters typed straight after a field opens. The palette, find and the
//! rename fields take the keys a frame after they open; anything typed in
//! between would be lost, or for find typed into the shell. It goes into
//! the field instead, in place of what the field held, as typing over a
//! selected name would.

use super::*;

/// The field the keys are meant for, if it is open but does not have them
/// yet.
enum Early {
    Palette,
    Find,
    Rename,
}

impl MuxApp {
    fn early_field(&self, window: &Window, cx: &App) -> Option<(Entity<InputState>, Early)> {
        let unfocused = |input: &Entity<InputState>| {
            !gpui::Focusable::focus_handle(input.read(cx), cx).is_focused(window)
        };
        if let Some(palette) = &self.palette {
            return unfocused(&palette.input).then(|| (palette.input.clone(), Early::Palette));
        }
        if let Some(rename) = &self.tab_rename {
            return unfocused(&rename.input).then(|| (rename.input.clone(), Early::Rename));
        }
        if let Some(rename) = &self.pane_rename {
            return unfocused(&rename.input).then(|| (rename.input.clone(), Early::Rename));
        }
        // Find stays open while the pane has the keys back, so only just
        // after it opens are its keys its own.
        self.find
            .as_ref()
            .filter(|find| find.opened.elapsed() <= find::FIND_CATCHES_TYPING)
            .filter(|find| unfocused(&find.input))
            .map(|find| (find.input.clone(), Early::Find))
    }

    /// Put a key typed before its field has the keys into the field.
    /// Returns whether the key was one.
    pub(super) fn catch_early_typing(
        &mut self,
        keystroke: &gpui::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((input, early)) = self.early_field(window, cx) else {
            return false;
        };
        let Some(text) = typed_text(keystroke) else {
            return false;
        };
        let id = input.entity_id();
        let mut typed = match self.early_typed.take() {
            Some((field, typed)) if field == id => typed,
            _ => String::new(),
        };
        typed.push_str(text);
        input.update(cx, |input, cx| input.set_value(typed.clone(), window, cx));
        match early {
            Early::Palette => {
                if let Some(palette) = self.palette.as_mut() {
                    palette.typed(&typed);
                }
            }
            Early::Find => self.find_typed(typed.clone(), cx),
            Early::Rename => {}
        }
        self.early_typed = Some((id, typed));
        cx.notify();
        true
    }
}

/// What a key types into a text field, if it types anything: not a key
/// with Ctrl or Cmd, nor one that types a control character.
fn typed_text(keystroke: &gpui::Keystroke) -> Option<&str> {
    let modifiers = keystroke.modifiers;
    if modifiers.control || modifiers.platform || modifiers.function {
        return None;
    }
    keystroke
        .key_char
        .as_deref()
        .filter(|text| !text.is_empty() && !text.chars().any(char::is_control))
}

#[cfg(test)]
mod tests {
    use super::typed_text;

    fn key(key: &str, key_char: Option<&str>, control: bool) -> gpui::Keystroke {
        gpui::Keystroke {
            modifiers: gpui::Modifiers {
                control,
                ..gpui::Modifiers::default()
            },
            key: key.to_owned(),
            key_char: key_char.map(str::to_owned),
        }
    }

    #[test]
    fn only_keys_that_type_are_caught() {
        assert_eq!(typed_text(&key("a", Some("a"), false)), Some("a"));
        assert_eq!(typed_text(&key("a", Some("a"), true)), None);
        assert_eq!(typed_text(&key("enter", Some("\n"), false)), None);
        assert_eq!(typed_text(&key("escape", None, false)), None);
    }
}
