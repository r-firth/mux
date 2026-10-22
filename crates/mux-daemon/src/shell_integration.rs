//! Marks for the shell's prompts. Mux's panes find their prompts by the
//! OSC 133 marks a shell prints; zsh and bash print none of their own, so a
//! pane's shell is started with a little that does. zsh is pointed at a
//! `.zshenv` of Mux's that hands over to the user's own startup files, and
//! bash is given a `PROMPT_COMMAND`, which also says how each command ended.
//! Other shells start as they are.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use tracing::warn;

const ZSHENV: &str = include_str!("shell_integration/zshenv");
/// How the last command ended and where the prompt begins, keeping `$?` for
/// a `PROMPT_COMMAND` of the user's that runs after it.
const BASH_PROMPT_COMMAND: &str = concat!(
    "__mux_status=$?; ",
    r"builtin printf '\033]133;D;%s\007\033]133;A\007' $__mux_status; ",
    "__mux_keep_status() { return $1; }; __mux_keep_status $__mux_status",
);

/// Where a command starts, printed from bash's PS0.
const BASH_COMMAND_START: &str = r"\e]133;C\a";
/// The prompt's blinking bar cursor, and the program's own cursor back as
/// a command starts, as Ghostty's shell integration does.
const BASH_PROMPT_CURSOR: &str = r"builtin printf '\033[5 q'; ";
const BASH_COMMAND_CURSOR: &str = r"\e[0 q";

/// Whether the prompt shows a bar cursor: Ghostty's `cursor` shell
/// integration feature, on unless the user's Ghostty config turns it off.
fn prompt_cursor() -> bool {
    #[cfg(feature = "ghostty")]
    {
        mux_terminal_ghostty::user_prompt_cursor()
    }
    #[cfg(not(feature = "ghostty"))]
    {
        true
    }
}

/// Whether the shell titles its pane with where it is and what it runs:
/// Ghostty's `title` shell integration feature, on unless turned off.
fn shell_title() -> bool {
    #[cfg(feature = "ghostty")]
    {
        mux_terminal_ghostty::user_shell_title()
    }
    #[cfg(not(feature = "ghostty"))]
    {
        true
    }
}

/// The pane's title as bash's prompt comes back: where the shell is, as
/// Ghostty's shell integration sets it.
const BASH_TITLE: &str = r#"builtin printf '\033]2;%s\007' "${PWD/#$HOME/\~}"; "#;

static ROOT: OnceLock<PathBuf> = OnceLock::new();

/// Where the integration lives, under the daemon's state directory. Panes
/// started before this is called start without it.
pub fn install(state_dir: &Path) {
    let root = state_dir.join("shell-integration");
    if write_zshenv(&root) {
        let _ = ROOT.set(root);
    }
}

fn write_zshenv(root: &Path) -> bool {
    let directory = root.join("zsh");
    let path = directory.join(".zshenv");
    if std::fs::read_to_string(&path).is_ok_and(|contents| contents == ZSHENV) {
        return true;
    }
    match std::fs::create_dir_all(&directory).and_then(|()| std::fs::write(&path, ZSHENV)) {
        Ok(()) => true,
        Err(error) => {
            warn!(path = %path.display(), %error, "could not write zsh integration");
            false
        }
    }
}

/// The environment that starts `program` with prompt marks, if it is a
/// shell Mux knows how to mark.
pub fn environment(program: &Path) -> Vec<(OsString, OsString)> {
    let Some(root) = ROOT.get() else {
        return Vec::new();
    };
    match program.file_name().and_then(|name| name.to_str()) {
        Some("zsh") => {
            // Written again should something have cleared it: zsh pointed at
            // an empty ZDOTDIR would skip the user's own startup files.
            if !write_zshenv(root) {
                return Vec::new();
            }
            let mut environment =
                vec![(OsString::from("ZDOTDIR"), root.join("zsh").into_os_string())];
            if let Some(user) = std::env::var_os("ZDOTDIR") {
                environment.push((OsString::from("MUX_USER_ZDOTDIR"), user));
            }
            if prompt_cursor() {
                environment.push((OsString::from("MUX_PROMPT_CURSOR"), OsString::from("1")));
            }
            if shell_title() {
                environment.push((OsString::from("MUX_SHELL_TITLE"), OsString::from("1")));
            }
            environment
        }
        Some("bash") => {
            let cursor = prompt_cursor();
            let extras = format!(
                "{}{}",
                if cursor { BASH_PROMPT_CURSOR } else { "" },
                if shell_title() { BASH_TITLE } else { "" }
            );
            let ours = BASH_PROMPT_COMMAND.replacen(
                "__mux_status=$?; ",
                &format!("__mux_status=$?; {extras}"),
                1,
            );
            let command = match std::env::var("PROMPT_COMMAND") {
                Ok(own) if !own.trim().is_empty() => format!("{ours}; {own}"),
                _ => ours,
            };
            // PS0 prints as a command starts (bash 4.4 and later), so an
            // empty line is told from a command and a command can be timed.
            let start = format!(
                "{BASH_COMMAND_START}{}{}",
                if cursor { BASH_COMMAND_CURSOR } else { "" },
                std::env::var("PS0").unwrap_or_default()
            );
            vec![
                (OsString::from("PROMPT_COMMAND"), OsString::from(command)),
                (OsString::from("PS0"), OsString::from(start)),
            ]
        }
        _ => Vec::new(),
    }
}
