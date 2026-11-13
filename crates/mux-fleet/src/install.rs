//! Putting Mux's hooks into the agents, and taking them out again.
//!
//! Each agent runs commands of the user's at the turns of a session. Mux
//! adds one of its own for each turn it follows:
//!
//! ```sh
//! [ -z "$MUX_PANE_ID" ] || "$MUX_CTL" hook claude Stop 2>/dev/null || true
//! ```
//!
//! Outside a Mux pane `MUX_PANE_ID` is unset and the command is over
//! before it starts anything, so an agent run in another terminal, an
//! editor or CI is as it was. Inside one, `MUX_CTL` is the `muxctl` of the
//! Mux that owns the pane, so nothing here names a path that an update or
//! a move could leave stale. The command prints nothing and always
//! succeeds: it can never hold an agent up, change what it does, or put
//! words in its context.
//!
//! Only Mux's own entries are ever added, replaced or removed, found by
//! the `"$MUX_CTL" hook` in their command; everything else in a file is
//! left as it is, in its order. A file is copied beside itself the first
//! time Mux changes it.
//!
//! - Claude Code: `hooks` in `~/.claude/settings.json`.
//! - Codex: `~/.codex/hooks.json`. Codex asks the user to trust a new
//!   hook, with `/hooks`, before it runs it, and trusts it by its text, so
//!   the command is kept the same from one Mux to the next.
//! - Copilot: a file of Mux's own, `~/.copilot/hooks/mux.json`.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use thiserror::Error;

use crate::AgentKind;

/// What marks a hook command as Mux's.
const MARK: &str = "\"$MUX_CTL\" hook ";
/// Seconds an agent waits on a hook before going on without it.
const TIMEOUT_SECONDS: u64 = 5;

/// Where each agent keeps its settings.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Homes {
    pub claude: PathBuf,
    pub codex: PathBuf,
    pub copilot: PathBuf,
}

impl Homes {
    /// The agents' own defaults under `home`, unless the variable each
    /// reads to move its settings says otherwise.
    #[must_use]
    pub fn new(home: &Path, variable: impl Fn(&str) -> Option<PathBuf>) -> Self {
        let place = |name: &str, default: &str| {
            variable(name)
                .filter(|path| path.is_absolute())
                .unwrap_or_else(|| home.join(default))
        };
        Self {
            claude: place("CLAUDE_CONFIG_DIR", ".claude"),
            codex: place("CODEX_HOME", ".codex"),
            copilot: place("COPILOT_HOME", ".copilot"),
        }
    }

    /// From this process's environment.
    #[must_use]
    pub fn from_environment() -> Option<Self> {
        let home = std::env::var_os("HOME").map(PathBuf::from)?;
        Some(Self::new(&home, |name| {
            std::env::var_os(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        }))
    }

    /// The agent's settings folder.
    #[must_use]
    pub fn home(&self, kind: AgentKind) -> Option<&Path> {
        match kind {
            AgentKind::Claude => Some(&self.claude),
            AgentKind::Codex => Some(&self.codex),
            AgentKind::Copilot => Some(&self.copilot),
            _ => None,
        }
    }

    /// The file Mux's hooks for the agent go in.
    #[must_use]
    pub fn hooks_file(&self, kind: AgentKind) -> Option<PathBuf> {
        match kind {
            AgentKind::Claude => Some(self.claude.join("settings.json")),
            AgentKind::Codex => Some(self.codex.join("hooks.json")),
            AgentKind::Copilot => Some(self.copilot.join("hooks").join("mux.json")),
            _ => None,
        }
    }
}

/// Whether Mux's hooks are in an agent.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HookStatus {
    /// The agent has never been set up on this machine.
    Absent,
    /// The agent is here, without Mux's hooks.
    Missing,
    /// Hooks from another version of Mux, which installing brings up to
    /// date.
    Outdated,
    Installed,
}

impl HookStatus {
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Absent => "not found",
            Self::Missing => "not hooked",
            Self::Outdated => "hooks out of date",
            Self::Installed => "hooked",
        }
    }
}

#[derive(Debug, Error)]
pub enum InstallError {
    #[error("Mux has no hooks for {0}")]
    Unsupported(&'static str),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// The file is not what the agent's hooks are kept in: it is left
    /// alone rather than mended.
    #[error("{path} is not as expected ({reason}); Mux left it as it is")]
    Unexpected { path: PathBuf, reason: String },
}

/// The command a hook runs for `event`.
#[must_use]
pub fn hook_command(kind: AgentKind, event: &str) -> String {
    format!(
        "[ -z \"$MUX_PANE_ID\" ] || {MARK}{} {event} 2>/dev/null || true",
        kind.command()
    )
}

/// The events Mux follows in each agent, and whether the agent may run
/// the hook alongside its work rather than wait for it.
const fn events(kind: AgentKind) -> &'static [(&'static str, bool)] {
    match kind {
        AgentKind::Claude => &[
            ("SessionStart", false),
            ("UserPromptSubmit", true),
            ("PreToolUse", true),
            ("PostToolUse", true),
            ("PermissionRequest", true),
            ("Notification", true),
            ("Stop", true),
            ("StopFailure", true),
            ("SessionEnd", false),
        ],
        AgentKind::Codex => &[
            ("SessionStart", false),
            ("UserPromptSubmit", false),
            ("PreToolUse", false),
            ("PostToolUse", false),
            ("PermissionRequest", false),
            ("Stop", false),
            ("SessionEnd", false),
        ],
        AgentKind::Copilot => &[
            ("sessionStart", false),
            ("userPromptSubmitted", false),
            ("preToolUse", false),
            ("postToolUse", false),
            ("permissionRequest", false),
            ("agentStop", false),
            ("errorOccurred", false),
            ("sessionEnd", false),
        ],
        _ => &[],
    }
}

/// Whether Mux's hooks are in `kind`.
#[must_use]
pub fn status(kind: AgentKind, homes: &Homes) -> HookStatus {
    let (Some(home), Some(file)) = (homes.home(kind), homes.hooks_file(kind)) else {
        return HookStatus::Absent;
    };
    if !home.is_dir() {
        return HookStatus::Absent;
    }
    let Ok(Some(current)) = read(&file) else {
        return HookStatus::Missing;
    };
    if !mentions_mux(&current) {
        return HookStatus::Missing;
    }
    match with_hooks(kind, &file, &current) {
        Ok(wanted) if wanted == current => HookStatus::Installed,
        _ => HookStatus::Outdated,
    }
}

/// Put Mux's hooks into `kind`, replacing any it had from before. Says
/// whether anything changed.
pub fn install(kind: AgentKind, homes: &Homes) -> Result<bool, InstallError> {
    let file = homes
        .hooks_file(kind)
        .ok_or(InstallError::Unsupported(kind.name()))?;
    let current = read(&file)?;
    let wanted = with_hooks(kind, &file, current.as_ref().unwrap_or(&json!({})))?;
    if current.as_ref() == Some(&wanted) {
        return Ok(false);
    }
    write(&file, &wanted, current.is_some())?;
    Ok(true)
}

/// Take Mux's hooks out of `kind`, leaving the rest. Says whether
/// anything changed.
pub fn uninstall(kind: AgentKind, homes: &Homes) -> Result<bool, InstallError> {
    let file = homes
        .hooks_file(kind)
        .ok_or(InstallError::Unsupported(kind.name()))?;
    let Some(current) = read(&file)? else {
        return Ok(false);
    };
    if kind == AgentKind::Copilot {
        // The file is Mux's own.
        std::fs::remove_file(&file).map_err(|source| InstallError::Io {
            path: file.clone(),
            source,
        })?;
        return Ok(true);
    }
    let mut wanted = current.clone();
    strip(&file, &mut wanted)?;
    if wanted == current {
        return Ok(false);
    }
    let emptied = wanted.as_object().is_some_and(Map::is_empty);
    if emptied && kind == AgentKind::Codex {
        // A hooks file with only Mux's hooks in it was Mux's making.
        std::fs::remove_file(&file).map_err(|source| InstallError::Io {
            path: file.clone(),
            source,
        })?;
    } else {
        write(&file, &wanted, true)?;
    }
    Ok(true)
}

/// `current` with Mux's hooks for `kind` in it, and none of its old ones.
fn with_hooks(kind: AgentKind, file: &Path, current: &Value) -> Result<Value, InstallError> {
    let mut wanted = current.clone();
    if kind == AgentKind::Copilot {
        let mut hooks = Map::new();
        for (event, _) in events(kind) {
            hooks.insert(
                (*event).to_owned(),
                json!([{
                    "type": "command",
                    "bash": hook_command(kind, event),
                    "timeoutSec": TIMEOUT_SECONDS,
                }]),
            );
        }
        return Ok(json!({ "version": 1, "hooks": hooks }));
    }
    strip(file, &mut wanted)?;
    let unexpected = |reason: &str| InstallError::Unexpected {
        path: file.to_path_buf(),
        reason: reason.to_owned(),
    };
    let root = wanted
        .as_object_mut()
        .ok_or_else(|| unexpected("it does not hold an object"))?;
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| unexpected("`hooks` is not an object"))?;
    for (event, alongside) in events(kind) {
        let mut handler = Map::new();
        handler.insert("type".to_owned(), json!("command"));
        handler.insert("command".to_owned(), json!(hook_command(kind, event)));
        handler.insert("timeout".to_owned(), json!(TIMEOUT_SECONDS));
        if *alongside {
            handler.insert("async".to_owned(), json!(true));
        }
        hooks
            .entry(*event)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or_else(|| unexpected("an event's hooks are not a list"))?
            .push(json!({ "hooks": [handler] }));
    }
    Ok(wanted)
}

/// Take Mux's handlers out of a settings file's `hooks`, and with them
/// whatever that leaves empty.
fn strip(file: &Path, settings: &mut Value) -> Result<(), InstallError> {
    let unexpected = |reason: &str| InstallError::Unexpected {
        path: file.to_path_buf(),
        reason: reason.to_owned(),
    };
    let root = settings
        .as_object_mut()
        .ok_or_else(|| unexpected("it does not hold an object"))?;
    let Some(hooks) = root.get_mut("hooks") else {
        return Ok(());
    };
    let hooks = hooks
        .as_object_mut()
        .ok_or_else(|| unexpected("`hooks` is not an object"))?;
    let mut touched = false;
    for groups in hooks.values_mut() {
        let Some(groups) = groups.as_array_mut() else {
            continue;
        };
        for group in groups.iter_mut() {
            let Some(handlers) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
                continue;
            };
            let before = handlers.len();
            handlers.retain(|handler| !is_mux_handler(handler));
            touched |= handlers.len() != before;
        }
        // Only a group Mux emptied goes: one the user left empty stays.
        groups.retain(|group| {
            !(touched
                && group
                    .get("hooks")
                    .and_then(Value::as_array)
                    .is_some_and(Vec::is_empty))
        });
    }
    if touched {
        hooks.retain(|_, groups| !groups.as_array().is_some_and(Vec::is_empty));
        if hooks.is_empty() {
            root.remove("hooks");
        }
    }
    Ok(())
}

fn is_mux_handler(handler: &Value) -> bool {
    ["command", "bash"].iter().any(|key| {
        handler
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|command| command.contains(MARK))
    })
}

fn mentions_mux(value: &Value) -> bool {
    match value {
        Value::String(text) => text.contains(MARK),
        Value::Array(items) => items.iter().any(mentions_mux),
        Value::Object(fields) => fields.values().any(mentions_mux),
        _ => false,
    }
}

/// The JSON in `file`, or nothing if it is not there or is empty.
fn read(file: &Path) -> Result<Option<Value>, InstallError> {
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(InstallError::Io {
                path: file.to_path_buf(),
                source,
            });
        }
    };
    if text.trim().is_empty() {
        return Ok(None);
    }
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|error| InstallError::Unexpected {
            path: file.to_path_buf(),
            reason: format!("not JSON: {error}"),
        })
}

/// Write `value` to `file` in one step, so an agent reading its settings
/// meanwhile sees the old file or the new and never half of one. A file
/// that was there is first copied beside itself, once.
fn write(file: &Path, value: &Value, existed: bool) -> Result<(), InstallError> {
    let failed = |path: &Path| {
        let path = path.to_path_buf();
        move |source| InstallError::Io { path, source }
    };
    // Written where a link points, so settings kept in a dotfiles
    // repository stay linked.
    let target = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let directory = target.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(directory).map_err(failed(directory))?;
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    if existed {
        let backup = directory.join(format!("{name}.before-mux"));
        if !backup.exists() {
            std::fs::copy(&target, &backup).map_err(failed(&backup))?;
        }
    }
    let mut text =
        serde_json::to_string_pretty(value).map_err(|error| InstallError::Unexpected {
            path: file.to_path_buf(),
            reason: error.to_string(),
        })?;
    text.push('\n');
    let temporary = directory.join(format!(".{name}.mux-{}", std::process::id()));
    std::fs::write(&temporary, text).map_err(failed(&temporary))?;
    if let Ok(metadata) = std::fs::metadata(&target) {
        let _ = std::fs::set_permissions(&temporary, metadata.permissions());
    }
    std::fs::rename(&temporary, &target).map_err(|source| {
        let _ = std::fs::remove_file(&temporary);
        InstallError::Io {
            path: target.clone(),
            source,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn homes(root: &Path) -> Homes {
        Homes::new(root, |_| None)
    }

    fn read_json(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).expect("file")).expect("JSON")
    }

    #[test]
    fn the_command_does_nothing_outside_a_mux_pane() {
        let command = hook_command(AgentKind::Claude, "Stop");
        assert_eq!(
            command,
            "[ -z \"$MUX_PANE_ID\" ] || \"$MUX_CTL\" hook claude Stop 2>/dev/null || true"
        );
        let root = tempfile::tempdir().expect("temporary directory");
        let ran = root.path().join("ran");
        let script = root.path().join("muxctl");
        std::fs::write(
            &script,
            format!("#!/bin/sh\necho \"$@\" > {}\n", ran.display()),
        )
        .expect("script");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
                .expect("permissions");
        }
        let run = |pane: Option<&str>, ctl: Option<&Path>| {
            let mut shell = std::process::Command::new("sh");
            shell.args(["-c", &command]);
            shell.env_remove("MUX_PANE_ID").env_remove("MUX_CTL");
            if let Some(pane) = pane {
                shell.env("MUX_PANE_ID", pane);
            }
            if let Some(ctl) = ctl {
                shell.env("MUX_CTL", ctl);
            }
            shell.output().expect("run sh")
        };
        let outside = run(None, Some(&script));
        assert!(outside.status.success());
        assert!(!ran.exists());
        // A pane whose muxctl has gone still lets the agent go on, silently.
        let broken = run(Some("pane"), None);
        assert!(broken.status.success());
        assert!(broken.stdout.is_empty() && broken.stderr.is_empty());
        let inside = run(Some("pane"), Some(&script));
        assert!(inside.status.success());
        assert_eq!(
            std::fs::read_to_string(&ran).expect("ran").trim(),
            "hook claude Stop"
        );
    }

    #[test]
    fn claude_hooks_go_in_beside_the_users_own_and_come_out_again() {
        let root = tempfile::tempdir().expect("temporary directory");
        let homes = homes(root.path());
        assert_eq!(status(AgentKind::Claude, &homes), HookStatus::Absent);
        std::fs::create_dir_all(&homes.claude).expect("claude home");
        let file = homes.claude.join("settings.json");
        let own = r#"{
  "model": "opus",
  "hooks": {
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "afplay /System/Library/Sounds/Glass.aiff"
          }
        ]
      }
    ]
  },
  "permissions": {
    "allow": [
      "Bash(cargo test:*)"
    ]
  }
}
"#;
        std::fs::write(&file, own).expect("settings");
        assert_eq!(status(AgentKind::Claude, &homes), HookStatus::Missing);

        assert!(install(AgentKind::Claude, &homes).expect("install"));
        assert_eq!(status(AgentKind::Claude, &homes), HookStatus::Installed);
        let settings = read_json(&file);
        // The user's keys keep their order, and their hook its place.
        let keys: Vec<_> = settings.as_object().expect("object").keys().collect();
        assert_eq!(keys, ["model", "hooks", "permissions"]);
        let stop = settings["hooks"]["Stop"].as_array().expect("Stop hooks");
        assert_eq!(stop.len(), 2);
        assert_eq!(
            stop[0]["hooks"][0]["command"],
            "afplay /System/Library/Sounds/Glass.aiff"
        );
        assert_eq!(
            stop[1]["hooks"][0]["command"],
            hook_command(AgentKind::Claude, "Stop")
        );
        assert_eq!(stop[1]["hooks"][0]["async"], true);
        assert!(
            settings["hooks"]["SessionStart"][0]["hooks"][0]
                .get("async")
                .is_none()
        );
        // The file as it was is kept beside it.
        assert_eq!(
            std::fs::read_to_string(homes.claude.join("settings.json.before-mux")).expect("copy"),
            own
        );

        // Again changes nothing.
        assert!(!install(AgentKind::Claude, &homes).expect("install again"));

        assert!(uninstall(AgentKind::Claude, &homes).expect("uninstall"));
        assert_eq!(std::fs::read_to_string(&file).expect("settings"), own);
        assert_eq!(status(AgentKind::Claude, &homes), HookStatus::Missing);
        assert!(!uninstall(AgentKind::Claude, &homes).expect("uninstall again"));
    }

    #[test]
    fn hooks_from_an_older_mux_are_brought_up_to_date() {
        let root = tempfile::tempdir().expect("temporary directory");
        let homes = homes(root.path());
        std::fs::create_dir_all(&homes.claude).expect("claude home");
        let file = homes.claude.join("settings.json");
        std::fs::write(
            &file,
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"\"$MUX_CTL\" hook claude Stop"}]}],
                "SubagentStop":[{"hooks":[{"type":"command","command":"\"$MUX_CTL\" hook claude SubagentStop"}]}]}}"#,
        )
        .expect("settings");
        assert_eq!(status(AgentKind::Claude, &homes), HookStatus::Outdated);
        assert!(install(AgentKind::Claude, &homes).expect("install"));
        assert_eq!(status(AgentKind::Claude, &homes), HookStatus::Installed);
        let settings = read_json(&file);
        assert!(settings["hooks"].get("SubagentStop").is_none());
        assert_eq!(settings["hooks"]["Stop"].as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn a_file_that_is_not_settings_is_left_alone() {
        let root = tempfile::tempdir().expect("temporary directory");
        let homes = homes(root.path());
        std::fs::create_dir_all(&homes.claude).expect("claude home");
        let file = homes.claude.join("settings.json");
        for broken in ["{ not json", "[1, 2]", r#"{"hooks": "none"}"#] {
            std::fs::write(&file, broken).expect("settings");
            assert!(matches!(
                install(AgentKind::Claude, &homes),
                Err(InstallError::Unexpected { .. })
            ));
            assert_eq!(std::fs::read_to_string(&file).expect("settings"), broken);
        }
    }

    #[test]
    fn codex_gets_a_hooks_file_and_loses_it_when_only_mux_was_in_it() {
        let root = tempfile::tempdir().expect("temporary directory");
        let homes = homes(root.path());
        std::fs::create_dir_all(&homes.codex).expect("codex home");
        let file = homes.codex.join("hooks.json");
        assert_eq!(status(AgentKind::Codex, &homes), HookStatus::Missing);
        assert!(install(AgentKind::Codex, &homes).expect("install"));
        assert_eq!(status(AgentKind::Codex, &homes), HookStatus::Installed);
        let hooks = read_json(&file);
        assert_eq!(
            hooks["hooks"]["PermissionRequest"][0]["hooks"][0]["command"],
            hook_command(AgentKind::Codex, "PermissionRequest")
        );
        // Codex is not asked to run a hook alongside its work.
        assert!(hooks["hooks"]["Stop"][0]["hooks"][0].get("async").is_none());
        assert!(uninstall(AgentKind::Codex, &homes).expect("uninstall"));
        assert!(!file.exists());
    }

    #[test]
    fn copilot_gets_a_file_of_its_own() {
        let root = tempfile::tempdir().expect("temporary directory");
        let homes = Homes::new(root.path(), |name| {
            (name == "COPILOT_HOME").then(|| root.path().join("elsewhere"))
        });
        assert_eq!(homes.copilot, root.path().join("elsewhere"));
        std::fs::create_dir_all(&homes.copilot).expect("copilot home");
        assert!(install(AgentKind::Copilot, &homes).expect("install"));
        let file = homes.copilot.join("hooks/mux.json");
        let hooks = read_json(&file);
        assert_eq!(hooks["version"], 1);
        assert_eq!(
            hooks["hooks"]["agentStop"][0]["bash"],
            hook_command(AgentKind::Copilot, "agentStop")
        );
        assert_eq!(status(AgentKind::Copilot, &homes), HookStatus::Installed);
        assert!(uninstall(AgentKind::Copilot, &homes).expect("uninstall"));
        assert!(!file.exists());
        assert!(matches!(
            install(AgentKind::Gemini, &homes),
            Err(InstallError::Unsupported(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn linked_settings_stay_linked() {
        let root = tempfile::tempdir().expect("temporary directory");
        let homes = homes(root.path());
        std::fs::create_dir_all(&homes.claude).expect("claude home");
        let dotfiles = root.path().join("dotfiles");
        std::fs::create_dir_all(&dotfiles).expect("dotfiles");
        std::fs::write(dotfiles.join("claude.json"), "{}\n").expect("settings");
        let link = homes.claude.join("settings.json");
        std::os::unix::fs::symlink(dotfiles.join("claude.json"), &link).expect("link");
        assert!(install(AgentKind::Claude, &homes).expect("install"));
        assert!(std::fs::symlink_metadata(&link).expect("link").is_symlink());
        assert!(mentions_mux(&read_json(&dotfiles.join("claude.json"))));
    }
}
