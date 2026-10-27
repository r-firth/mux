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
/// a `PROMPT_COMMAND` of the user's that runs after it. The prompt's own
/// mark goes into `PS1` too, as Ghostty's integration puts it, so a prompt
/// bash draws again, as `Ctrl+l` does, is still marked.
const BASH_PROMPT_COMMAND: &str = concat!(
    "__mux_status=$?; ",
    r"builtin printf '\033]133;D;%s\007\033]133;A\007' $__mux_status; ",
    r#"[[ $PS1 == *'133;A'* ]] || PS1='\[\e]133;A\a\]'"$PS1"; "#,
    "__mux_keep_status() { return $1; }; __mux_keep_status $__mux_status",
);

/// Where a command starts, printed from bash's PS0.
const BASH_COMMAND_START: &str = r"\e]133;C\a";
/// The prompt's blinking bar cursor, and the program's own cursor back as a
/// command starts, as Ghostty's shell integration does.
const BASH_PROMPT_CURSOR: &str = r"builtin printf '\033[5 q'; ";
const BASH_COMMAND_CURSOR: &str = r"\e[0 q";

/// The pane's title as bash's prompt comes back: where the shell is, as
/// Ghostty's shell integration sets it. Control characters are left out, so
/// a directory named with escape sequences cannot send them to the terminal.
const BASH_TITLE: &str = concat!(
    r#"__mux_title=${PWD/#$HOME/\~}; "#,
    r#"builtin printf '\033]2;%s\007' "${__mux_title//[[:cntrl:]]/}"; "#,
);

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
            environment.push((OsString::from("MUX_PROMPT_CURSOR"), OsString::from("5")));
            environment.push((OsString::from("MUX_SHELL_TITLE"), OsString::from("1")));
            environment
        }
        Some("bash") => {
            let extras = format!("{BASH_PROMPT_CURSOR}{BASH_TITLE}");
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
                "{BASH_COMMAND_START}{BASH_COMMAND_CURSOR}{}",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_title_leaves_out_control_characters_in_the_directory() {
        let root = tempfile::tempdir().expect("temporary directory");
        // Canonical, as bash finds its directory, so HOME matches it.
        let home = root.path().canonicalize().expect("canonical directory");
        let directory = home.join("a\x1b]0;forged\x07b");
        std::fs::create_dir(&directory).expect("directory");
        let output = std::process::Command::new("bash")
            .args(["--norc", "--noprofile", "-c", BASH_TITLE])
            .current_dir(&directory)
            .env("HOME", &home)
            .env_remove("PWD")
            .output()
            .expect("run bash");
        assert_eq!(output.stdout, b"\x1b]2;~/a]0;forgedb\x07");
    }
}
