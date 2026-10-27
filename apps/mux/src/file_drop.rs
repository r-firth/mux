//! Files dropped on a pane, as from the Finder: their paths are typed at the
//! pane's cursor, each escaped for the shell and followed by a space, as a
//! Mac terminal does. They go in as a paste, so a shell that takes pastes
//! whole doesn't run anything until Return.

use gpui::ExternalPaths;

use super::*;

impl MuxApp {
    /// A frame over the pane that lights in the active tab's ink while files
    /// are held over it, and takes them when they are let go.
    pub(super) fn file_drop_target(&self, pane_id: PaneId, cx: &mut Context<Self>) -> gpui::Div {
        let ink = self.active_ink().color();
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .border_2()
            .border_color(gpui::transparent_black())
            .drag_over::<ExternalPaths>(move |style, _, _, _| {
                style.border_color(ink).bg(ink.opacity(0.06))
            })
            .on_drop(cx.listener(move |this, paths: &ExternalPaths, window, cx| {
                this.drop_paths(pane_id, paths.paths(), window, cx);
            }))
    }

    fn drop_paths(
        &mut self,
        pane_id: PaneId,
        paths: &[std::path::PathBuf],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A name with control characters in it can't be typed safely in
        // any shell: a Ctrl+u and a Return in it would run a command. Such a
        // file is left out, as Ghostty refuses to open one.
        let (typed, refused): (Vec<_>, Vec<_>) = paths
            .iter()
            .partition(|path| !path.to_string_lossy().chars().any(char::is_control));
        if !refused.is_empty() {
            self.say_in_strip(
                "left out a file whose name holds control characters",
                true,
                cx,
            );
        }
        let text = dropped_paths_text(typed);
        if text.is_empty() {
            return;
        }
        self.focus_handle.focus(window, cx);
        self.request_pane_focus(pane_id);
        if let Some(pane) = self.panes.get(&pane_id)
            && let Ok(bytes) = pane.with_engine(|engine| engine.encode_paste(&text))
            && !bytes.is_empty()
        {
            self.note_pane_key(pane_id);
            self.backend.send(CommandMessage::Write { pane_id, bytes });
            self.return_to_latest(pane_id, cx);
        }
        cx.notify();
    }
}

/// The paths as they would be typed: each escaped for the shell, with a
/// space after each.
fn dropped_paths_text<'a>(paths: impl IntoIterator<Item = &'a std::path::PathBuf>) -> String {
    let mut text = String::new();
    for path in paths {
        text.push_str(&shell_escaped(&path.to_string_lossy()));
        text.push(' ');
    }
    text
}

/// `text` with a backslash before each character a shell would read as
/// more than itself, as the Finder's drops into Terminal are.
fn shell_escaped(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        let plain = character.is_alphanumeric()
            || matches!(
                character,
                '/' | '.' | '_' | '-' | '+' | ',' | ':' | '@' | '%' | '='
            );
        if !plain {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{dropped_paths_text, shell_escaped};

    #[test]
    fn a_dropped_path_is_escaped_for_the_shell() {
        assert_eq!(
            shell_escaped("/Users/ryan/notes.md"),
            "/Users/ryan/notes.md"
        );
        assert_eq!(
            shell_escaped("/Users/ryan/My Drive/it's (final).txt"),
            r"/Users/ryan/My\ Drive/it\'s\ \(final\).txt"
        );
        assert_eq!(shell_escaped("~/a&b;$c*"), r"\~/a\&b\;\$c\*");
        assert_eq!(shell_escaped("/tmp/café"), "/tmp/café");
    }

    #[test]
    fn dropped_paths_are_typed_one_after_another() {
        let paths = [PathBuf::from("/tmp/a b"), PathBuf::from("/tmp/c")];
        assert_eq!(dropped_paths_text(&paths), r"/tmp/a\ b /tmp/c ");
        assert_eq!(dropped_paths_text(&[] as &[PathBuf]), "");
    }
}
