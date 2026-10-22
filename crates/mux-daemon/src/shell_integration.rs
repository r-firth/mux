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
            environment
        }
        Some("bash") => {
            let command = match std::env::var("PROMPT_COMMAND") {
                Ok(own) if !own.trim().is_empty() => format!("{BASH_PROMPT_COMMAND}; {own}"),
                _ => BASH_PROMPT_COMMAND.to_owned(),
            };
            vec![(OsString::from("PROMPT_COMMAND"), OsString::from(command))]
        }
        _ => Vec::new(),
    }
}
