//! What is installed on this machine, and where things go.
//!
//! The record exists so that "is this up to date" can be answered without
//! hashing an installed binary against a zip it was extracted from --- those
//! are different bytes, so the question has no direct answer. What is recorded
//! is the commit each plug-in was installed from, which is the thing a user
//! actually wants compared.
//!
//! **A missing record is not a claim that nothing is installed.** If the file
//! is gone but the bundle is on disk, the plug-in is reported as present with
//! an unknown build rather than as absent --- an installer that says "not
//! installed" about something sitting in the plug-in folder is wrong in the
//! direction that makes people install it twice.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One installed plug-in, as this program last left it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub version: String,
    pub commit: String,
    pub installed: String,
    /// Every path this program created, so an uninstall removes exactly what
    /// an install added and nothing near it.
    pub paths: Vec<PathBuf>,
}

/// The whole record, keyed by plug-in id.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    pub installed: BTreeMap<String, Record>,
}

impl State {
    pub fn load() -> State {
        let Some(p) = state_file() else {
            return State::default();
        };
        let Ok(text) = std::fs::read_to_string(p) else {
            return State::default();
        };
        // A corrupt record must not stop an install. The worst case of
        // starting fresh is that an uninstall does not know what an earlier
        // version put where, which is recoverable; refusing to run is not.
        serde_json::from_str(&text).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let Some(p) = state_file() else {
            return Err("no local data directory on this system".into());
        };
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))
    }
}

fn state_file() -> Option<PathBuf> {
    let dirs = directories::ProjectDirs::from(
        "io.github",
        "Noob Audio Engineering",
        "noob-plugin-manager",
    )?;
    Some(dirs.data_local_dir().join("installed.json"))
}

/// Whether this machine can host a part of the given kind at all.
///
/// Audio Units are a macOS format and there is nowhere on Windows for one to
/// go. A build that carries one is not a broken build --- every macOS
/// download carries all three --- so a Windows install steps over it rather
/// than refusing the whole plug-in.
pub fn belongs_here(into: &str) -> bool {
    if into == "au" {
        return cfg!(target_os = "macos");
    }
    true
}

/// Where a `vst3`, `clap` or `au` part belongs on this machine.
///
/// **Windows** uses the shared, machine-wide directories every host scans; on
/// this project's machines they are writable without elevation.
///
/// **macOS** uses the machine-wide `/Library/Audio/Plug-Ins`, and asks for a
/// password when it cannot write there. It used to install into the user's
/// own `~/Library` to avoid ever asking, which is tidy right up until the
/// plug-in does not appear: a second account, a host launched by something
/// else, or simply a machine where everything else lives in `/Library` and
/// this one thing does not. Where a plug-in goes is not a place to be clever.
///
/// The user's own folder is still there as the fallback, and
/// `prefer_user_dirs` still chooses it outright for anyone who would rather
/// not be asked.
///
/// Where a directory cannot be written, the install fails with the path in
/// the message rather than silently putting the plug-in somewhere no host
/// looks.
pub fn install_dir(into: &str) -> Result<PathBuf, String> {
    let leaf = match into {
        "vst3" => "VST3",
        "clap" => "CLAP",
        // Audio Units live in `Components`, not in a folder named after the
        // format --- the directory is older than the habit of naming one
        // after the plug-in standard, and every macOS host looks there.
        "au" => "Components",
        other => {
            return Err(format!(
                "the manifest asks for a `{other}` directory, which this installer does not know about --- update it"
            ));
        }
    };
    // The machine-wide directory, unless the person has asked for their own.
    //
    // **No silent fallback.** This used to check whether the shared folder
    // was writable and quietly use the user's when it was not, which reports
    // success and puts the plug-in somewhere nobody chose --- and on a
    // machine where everything else lives in the shared folder, "installed"
    // and "where you expected it" stop being the same thing. Returning the
    // shared path lets the write fail, and a failed write is what
    // `crate::elevate` classifies and the interface turns into an offer of
    // administrator. Asking is the point.
    if crate::settings::Settings::load().prefer_user_dirs {
        return Ok(user_root()?.join(leaf));
    }
    Ok(plugin_root()?.join(leaf))
}

/// Whether a directory can be created in and written to, asked by doing it
/// rather than by inspecting permissions --- which on Windows is the only
/// answer that means anything.
fn writable(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".noob-write-probe");
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// The per-user plug-in directory, which every host also scans and which
/// needs no administrator.
fn user_root() -> Result<PathBuf, String> {
    if cfg!(target_os = "windows") {
        let local = std::env::var("LOCALAPPDATA")
            .map_err(|_| "no LOCALAPPDATA on this system".to_string())?;
        Ok(Path::new(&local).join("Programs").join("Common"))
    } else if cfg!(target_os = "macos") {
        let home = std::env::var("HOME").map_err(|_| "no HOME on this system".to_string())?;
        Ok(Path::new(&home).join("Library/Audio/Plug-Ins"))
    } else {
        plugin_root()
    }
}

/// The directory both formats sit under on this platform.
fn plugin_root() -> Result<PathBuf, String> {
    if cfg!(target_os = "windows") {
        let common = std::env::var("CommonProgramFiles")
            .unwrap_or_else(|_| r"C:\Program Files\Common Files".to_string());
        Ok(PathBuf::from(common))
    } else if cfg!(target_os = "macos") {
        // The machine-wide one. Writing here needs a password, which
        // `crate::elevate` asks for rather than silently going elsewhere.
        Ok(PathBuf::from("/Library/Audio/Plug-Ins"))
    } else {
        Err(format!(
            "this installer knows where plug-ins go on Windows and macOS, and this is {}.              The builds exist; only the destination is missing.",
            std::env::consts::OS
        ))
    }
}

/// Whether the shared, machine-wide plug-in folders can be written.
///
/// The settings panel asks so it can say whether installing into your own
/// folders is a preference or already the only thing that can happen ---
/// which on this project's machines differs between VST3 and CLAP.
pub fn shared_writable() -> bool {
    let Ok(root) = plugin_root() else {
        return false;
    };
    ["VST3", "CLAP"].iter().all(|l| writable(&root.join(l)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A part this machine cannot host is skipped, not refused.**
    ///
    /// Every macOS download carries all three formats and the Windows one
    /// carries two, so a Windows machine reading a macOS manifest --- which is
    /// what happens the moment anybody shares a link --- must step over the
    /// Audio Unit rather than fail the whole install. The opposite mistake is
    /// worse: silently skipping something this machine *can* host would leave
    /// a plug-in half installed and report success.
    #[test]
    fn only_the_parts_this_machine_can_host() {
        assert!(belongs_here("vst3"), "vst3 belongs everywhere");
        assert!(belongs_here("clap"), "clap belongs everywhere");
        assert_eq!(
            belongs_here("au"),
            cfg!(target_os = "macos"),
            "an Audio Unit belongs on macOS and nowhere else"
        );
    }

    /// Every kind the manifests actually publish has somewhere to go, on the
    /// platform that can host it.
    ///
    /// A kind the installer does not know about is an error with the name in
    /// it, which is right --- but only if the kinds we do publish are known,
    /// and this is what says they are.
    #[test]
    fn every_published_kind_has_a_home() {
        for kind in ["vst3", "clap", "au"] {
            if !belongs_here(kind) {
                continue;
            }
            let dir = install_dir(kind).unwrap_or_else(|e| panic!("`{kind}` has no home: {e}"));
            let leaf = dir.file_name().and_then(|s| s.to_str()).unwrap_or("");
            let want = match kind {
                "vst3" => "VST3",
                "clap" => "CLAP",
                // Audio Units live in `Components`: the directory is older
                // than the habit of naming one after the plug-in standard.
                "au" => "Components",
                _ => unreachable!(),
            };
            assert_eq!(leaf, want, "`{kind}` was sent to {}", dir.display());
        }
    }

    /// A kind from the future is refused by name rather than guessed at.
    #[test]
    fn an_unknown_kind_says_what_it_was() {
        let e = install_dir("aax").expect_err("an unknown kind should not resolve");
        assert!(e.contains("aax"), "the message does not name the kind: {e}");
    }
    /// **The machine-wide directory, and no quiet detour.**
    ///
    /// The point of asking for a password is that the plug-in ends up where
    /// the person expects. An installer that notices it cannot write and puts
    /// the file somewhere else instead reports success and leaves them
    /// looking for it, which is the failure this replaced.
    #[test]
    fn the_shared_directory_is_the_one_chosen() {
        let dir = install_dir("vst3").expect("vst3 has a home");
        let shared = plugin_root().expect("this platform has a plug-in root");
        assert!(
            dir.starts_with(&shared),
            "{} is not under the machine-wide {}",
            dir.display(),
            shared.display()
        );
        if cfg!(target_os = "macos") {
            assert!(
                dir.starts_with("/Library/Audio/Plug-Ins"),
                "macOS should install machine-wide, got {}",
                dir.display()
            );
        }
    }

    /// And the two roots really are different places, or the fallback and the
    /// escalation are the same thing and neither means anything.
    #[test]
    fn the_user_root_is_somewhere_else() {
        let (shared, user) = (plugin_root().unwrap(), user_root().unwrap());
        assert_ne!(
            shared, user,
            "the shared and user roots are the same path, so asking for a              password could never change where anything goes"
        );
    }

}
