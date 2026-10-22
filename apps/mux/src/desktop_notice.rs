//! A program's desktop notification (OSC 9, OSC 777) reaches the system's
//! notifications while Mux is behind other windows, as Ghostty's do; with
//! Mux in front, the pane's mark says it.

use std::process::{Command, Stdio};

/// The most of a program's words a notification carries.
const MOST_SAID: usize = 240;

/// Post a notification, without waiting for the system to show it.
pub(super) fn post(title: &str, body: &str) {
    let (title, body) = (plain(title), plain(body));
    let mut command = notifier(&title, &body);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match command.spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(error) => tracing::debug!(%error, "desktop notification not posted"),
    }
}

/// What a program said, one line without control characters, cut short.
fn plain(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .take(MOST_SAID)
        .collect()
}

#[cfg(target_os = "macos")]
fn notifier(title: &str, body: &str) -> Command {
    let mut command = Command::new("osascript");
    command.arg("-e").arg(format!(
        "display notification {} with title {}",
        applescript_string(body),
        applescript_string(title)
    ));
    command
}

#[cfg(not(target_os = "macos"))]
fn notifier(title: &str, body: &str) -> Command {
    let mut command = Command::new("notify-send");
    command.arg("--app-name=Mux").arg(title).arg(body);
    command
}

/// An `AppleScript` string literal, so a program's words stay words.
#[cfg(any(target_os = "macos", test))]
fn applescript_string(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for character in text.chars() {
        if matches!(character, '"' | '\\') {
            quoted.push('\\');
        }
        quoted.push(character);
    }
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_programs_words_stay_words() {
        assert_eq!(
            applescript_string(r#"say "hi" \ bye"#),
            r#""say \"hi\" \\ bye""#
        );
        assert_eq!(plain("two\nlines\x07"), "two lines ");
        assert_eq!(plain(&"x".repeat(500)).len(), MOST_SAID);
    }
}
