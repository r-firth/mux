use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const SETTINGS_FILE: &str = "settings.json";

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct AppSettings {
    #[serde(default)]
    disabled_agent_profiles: BTreeSet<String>,
}

impl AppSettings {
    #[must_use]
    pub fn agent_enabled(&self, profile_id: &str) -> bool {
        !self.disabled_agent_profiles.contains(profile_id)
    }

    pub fn set_agent_enabled(&mut self, profile_id: &str, enabled: bool) {
        if enabled {
            self.disabled_agent_profiles.remove(profile_id);
        } else {
            self.disabled_agent_profiles.insert(profile_id.to_owned());
        }
    }

    pub fn load(state_dir: &Path) -> Result<Self> {
        let path = settings_path(state_dir);
        match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("read settings from {}", path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => {
                Err(error).with_context(|| format!("read settings from {}", path.display()))
            }
        }
    }

    pub fn save(&self, state_dir: &Path) -> Result<()> {
        fs::create_dir_all(state_dir)
            .with_context(|| format!("create settings directory {}", state_dir.display()))?;
        let destination = settings_path(state_dir);
        let temporary = temporary_settings_path(state_dir);
        let bytes = serde_json::to_vec_pretty(self).context("encode settings")?;
        let mut file = File::create(&temporary)
            .with_context(|| format!("create temporary settings file {}", temporary.display()))?;
        file.write_all(&bytes)
            .with_context(|| format!("write temporary settings file {}", temporary.display()))?;
        file.write_all(b"\n")
            .with_context(|| format!("finish temporary settings file {}", temporary.display()))?;
        file.sync_all()
            .with_context(|| format!("sync temporary settings file {}", temporary.display()))?;
        fs::rename(&temporary, &destination).with_context(|| {
            format!(
                "replace settings file {} with {}",
                destination.display(),
                temporary.display()
            )
        })
    }
}

fn settings_path(state_dir: &Path) -> PathBuf {
    state_dir.join(SETTINGS_FILE)
}

fn temporary_settings_path(state_dir: &Path) -> PathBuf {
    state_dir.join(format!(".{SETTINGS_FILE}.tmp"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integrations_default_to_enabled_and_round_trip() {
        let directory = tempfile::tempdir().expect("temporary settings directory");
        let mut settings = AppSettings::load(directory.path()).expect("default settings");
        assert!(settings.agent_enabled("codex-acp"));

        settings.set_agent_enabled("codex-acp", false);
        settings.set_agent_enabled("gemini", true);
        settings.save(directory.path()).expect("save settings");

        let restored = AppSettings::load(directory.path()).expect("restore settings");
        assert!(!restored.agent_enabled("codex-acp"));
        assert!(restored.agent_enabled("gemini"));
    }

    #[test]
    fn unknown_fields_are_ignored_for_forward_compatibility() {
        let decoded: AppSettings =
            serde_json::from_str(r#"{"disabled_agent_profiles":["gemini"],"future_setting":true}"#)
                .expect("forward-compatible settings");
        assert!(!decoded.agent_enabled("gemini"));
        assert!(decoded.agent_enabled("future-agent"));
    }
}
