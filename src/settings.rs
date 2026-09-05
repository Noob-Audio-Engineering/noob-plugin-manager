//! What the person using this has chosen, kept between runs.
//!
//! Deliberately small. Every field here is one somebody has a reason to want
//! different and that this program cannot work out for itself --- where to put
//! things when both places would work, and a token it is never entitled to
//! invent.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Install into the user's own plug-in folders even where the shared ones
    /// are writable. Off by default, because a shared install is visible to
    /// every account on the machine and that is usually what somebody means.
    pub prefer_user_dirs: bool,
    /// A GitHub token, used only to raise the rate limit --- everything read
    /// is public. Stored here so it survives a restart; the environment still
    /// wins when it has one, so a machine-wide token needs no setting.
    pub token: Option<String>,
}

impl Settings {
    pub fn load() -> Settings {
        let Some(p) = file() else {
            return Settings::default();
        };
        std::fs::read_to_string(p)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let Some(p) = file() else {
            return Err("no local data directory on this system".into());
        };
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))
    }

    /// The token to use, if any. The environment wins so that a machine
    /// already set up for `gh` needs nothing entered here.
    pub fn effective_token(&self) -> Option<String> {
        for k in ["GITHUB_TOKEN", "GH_TOKEN"] {
            if let Ok(v) = std::env::var(k)
                && !v.trim().is_empty()
            {
                return Some(v);
            }
        }
        self.token
            .as_ref()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
    }
}

fn file() -> Option<PathBuf> {
    let dirs = directories::ProjectDirs::from(
        "io.github",
        "Noob Audio Engineering",
        "noob-plugin-manager",
    )?;
    Some(dirs.data_local_dir().join("settings.json"))
}
