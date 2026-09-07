//! Which plug-ins exist, and what the newest build of each one is.
//!
//! **Discovered rather than listed.** The alternative was a registry file in
//! this repository naming the five plug-ins, and it would have been wrong the
//! day a sixth shipped --- by somebody who had no reason to think an installer
//! in another repository needed editing. Instead this asks the organisation
//! what it has, and treats *publishing a manifest* as the thing that makes a
//! repository a plug-in.
//!
//! So a new plug-in appears here the moment its own release pipeline runs, and
//! a repository that is not a plug-in never appears at all, because it has no
//! manifest to find.

use serde::Deserialize;

/// The organisation whose repositories are searched.
pub const ORG: &str = "Noob-Audio-Engineering";

/// The platform this build installs, which is the platform it was *compiled
/// for* --- not a constant anybody edits.
///
/// **This was `"windows-x86_64"` on every platform.** The manifests were
/// already published per platform and everything downstream of here already
/// knew about macOS: `state.rs` picks `/Library/Audio/Plug-Ins`, `install.rs`
/// restores the executable bit, `belongs_here` admits an Audio Unit. All of
/// it was reached with the Windows build in hand, because this line chose
/// which manifest to read before any of that ran.
///
/// What it did on a Mac was install: the zip unpacked, the checksum matched,
/// the `.vst3` directory appeared in the right place, and inside it was
/// `Contents/x86_64-win/` --- a Windows DLL in a bundle macOS will never load.
/// The Audio Unit was not installed at all, because a Windows build has none
/// to install. Six plug-ins, "installed", invisible in every host, with
/// nothing anywhere reporting a failure.
///
/// The labels are the release workflow's matrix labels, which is what the
/// manifest's `platform` field and the asset names are built from. macOS ships
/// one universal build for both architectures, so both map to the same label.
pub const PLATFORM: &str = if cfg!(target_os = "windows") {
    "windows-x86_64"
} else if cfg!(target_os = "macos") {
    "macos-universal"
} else {
    // No builds are published for anything else. Naming the platform rather
    // than defaulting to one keeps the failure honest: nothing matches, the
    // manager says it found no plug-ins, and it does not install a foreign
    // build to a machine that cannot run it.
    std::env::consts::OS
};

/// What a plug-in's pipeline publishes beside its zip.
///
/// This is the contract between a plug-in's release workflow and this program.
/// `schema` exists so that a later version can add fields without an older
/// installer guessing at them.
#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub schema: u32,
    pub id: String,
    pub version: String,
    pub commit: String,
    pub built: String,
    pub platform: String,
    pub sha256: String,
    pub url: String,
    pub installs: Vec<Install>,
    /// How the plug-in describes itself, from its own crate manifest. Absent
    /// on a build made before this existed, and on a crate that has not filled
    /// it in --- both of which mean "not said" rather than "wrong".
    #[serde(default)]
    pub display: Option<Display>,
    /// A photograph of the plug-in running, taken by its own pipeline from the
    /// build this manifest describes. Absent when the picture could not be
    /// taken --- the manager then shows the plug-in without one rather than
    /// treating the release as broken.
    #[serde(default)]
    pub banner: Option<String>,
}

/// A plug-in's own words about itself. None of it is written here: the
/// installer holding a description of somebody else's work would be a
/// description kept where its author never looks.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct Display {
    pub name: Option<String>,
    pub tagline: Option<String>,
    #[serde(default)]
    pub features: Vec<String>,
    pub accent: Option<String>,
    /// `instrument` or `effect`, in the plug-in's own words. Anything else,
    /// or nothing, files it under neither --- a plug-in that has not said
    /// what it is still needs to be installable.
    #[serde(default)]
    pub kind: Option<String>,
}

/// One installable part of a bundle, and which directory it belongs in.
#[derive(Debug, Clone, Deserialize)]
pub struct Install {
    /// `vst3`, `clap` or `au`, for reporting.
    pub kind: String,
    /// The path inside the zip.
    pub path: String,
    /// Which install directory it goes into: `vst3`, `clap` or `au`. A macOS
    /// build carries all three; a Windows one carries the first two.
    pub into: String,
}

impl Manifest {
    /// The first seven characters of the commit, for display.
    pub fn short_commit(&self) -> &str {
        let n = self.commit.len().min(7);
        &self.commit[..n]
    }

    /// The date part of the build stamp, which is what a reader wants: these
    /// builds move with every commit, so the version rarely tells you how old
    /// one is and the date always does.
    pub fn built_day(&self) -> &str {
        self.built.split('T').next().unwrap_or(&self.built)
    }
}

#[derive(Deserialize)]
struct Repo {
    name: String,
}

#[derive(Deserialize)]
struct Release {
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

/// Every plug-in the organisation publishes for this platform, newest build
/// each.
///
/// Errors from a single repository are returned alongside the successes rather
/// than aborting the run: one plug-in whose pipeline is mid-flight should not
/// stop you installing the other four.
pub fn discover(agent: &ureq::Agent) -> (Vec<Manifest>, Vec<String>) {
    let mut found = Vec::new();
    let mut problems = Vec::new();

    let url = format!("https://api.github.com/orgs/{ORG}/repos?per_page=100&type=public");
    let repos: Vec<Repo> = match get_json(agent, &url) {
        Ok(r) => r,
        Err(e) => {
            problems.push(format!("could not list the {ORG} repositories: {e}"));
            return (found, problems);
        }
    };

    for repo in repos {
        match newest(agent, &repo.name) {
            Ok(Some(m)) => found.push(m),
            // No `latest` release, or no manifest in it: not a plug-in, or not
            // one that has built yet. Neither is a problem worth reporting.
            Ok(None) => {}
            Err(e) => problems.push(format!("{}: {e}", repo.name)),
        }
    }

    found.sort_by(|a, b| a.id.cmp(&b.id));
    (found, problems)
}

/// The manifest in a repository's rolling `latest` release, if it has one for
/// this platform.
fn newest(agent: &ureq::Agent, repo: &str) -> Result<Option<Manifest>, String> {
    let url = format!("https://api.github.com/repos/{ORG}/{repo}/releases/tags/latest");
    let release: Release = match get_json(agent, &url) {
        Ok(r) => r,
        // A repository with no `latest` release is not a plug-in yet.
        Err(e) if e.contains("404") => return Ok(None),
        Err(e) => return Err(e),
    };

    let want = format!("-{PLATFORM}.json");
    let Some(asset) = release.assets.iter().find(|a| a.name.ends_with(&want)) else {
        return Ok(None);
    };

    let body = agent
        .get(&asset.browser_download_url)
        .call()
        .map_err(|e| format!("fetching {}: {e}", asset.name))?
        .into_string()
        .map_err(|e| format!("reading {}: {e}", asset.name))?;
    let manifest: Manifest = serde_json::from_str(&body).map_err(|e| {
        format!(
            "{} is not a manifest this version understands: {e}",
            asset.name
        )
    })?;

    if manifest.schema != 1 {
        return Err(format!(
            "{} declares manifest schema {}, and this installer understands 1 --- update it",
            manifest.id, manifest.schema
        ));
    }
    if manifest.platform != PLATFORM {
        return Ok(None);
    }
    Ok(Some(manifest))
}

fn get_json<T: serde::de::DeserializeOwned>(agent: &ureq::Agent, url: &str) -> Result<T, String> {
    let mut req = agent.get(url).set("Accept", "application/vnd.github+json");
    // A token is not required and is used only to raise the rate limit: the
    // releases this reads are public. `gh auth token` puts one in the
    // environment on a machine that already has the CLI.
    if let Some(tok) = token() {
        req = req.set("Authorization", &format!("Bearer {tok}"));
    }
    match req.call() {
        Ok(r) => r.into_json().map_err(|e| e.to_string()),
        // A rate limit is the one failure that will actually happen to
        // somebody, and reported as a plain error it reads as "there are no
        // plug-ins" --- which is a different and much more alarming claim.
        Err(ureq::Error::Status(403 | 429, r)) => {
            let reset = r
                .header("x-ratelimit-reset")
                .and_then(|s| s.parse::<u64>().ok());
            let mins = reset
                .and_then(|t| {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .ok()?
                        .as_secs();
                    Some(t.saturating_sub(now).div_ceil(60))
                })
                .unwrap_or(0);
            Err(format!(
                "GitHub is rate-limiting this machine{}.                  Nothing is wrong with the plug-ins or with this program --- it has                  simply asked too often. Set GITHUB_TOKEN (`gh auth token`) to raise                  the limit, or wait.",
                if mins > 0 {
                    format!(" for about {mins} more minute(s)")
                } else {
                    String::new()
                }
            ))
        }
        Err(e) => Err(e.to_string()),
    }
}

/// A token, from the environment or the settings. Never required.
fn token() -> Option<String> {
    crate::settings::Settings::load().effective_token()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The platform this installs for is the platform it runs on.**
    ///
    /// This is the test that was missing. `PLATFORM` was the literal
    /// `"windows-x86_64"`, so a macOS build of this program asked every
    /// repository for the Windows manifest, downloaded the Windows zip, and
    /// installed a `.vst3` directory whose only content was
    /// `Contents/x86_64-win/` --- a DLL, in a bundle macOS cannot load. The
    /// Audio Unit never arrived either, because the Windows build has none.
    ///
    /// Nothing reported a failure: the checksum matched, the archive unpacked,
    /// the directory landed where it belonged. It was correct in every respect
    /// except which platform's build it was.
    #[test]
    fn the_platform_installed_is_the_platform_running() {
        let want = if cfg!(target_os = "windows") {
            "windows-x86_64"
        } else if cfg!(target_os = "macos") {
            "macos-universal"
        } else {
            std::env::consts::OS
        };
        assert_eq!(
            PLATFORM,
            want,
            "this build installs {PLATFORM} builds on {}, which is somebody else's plug-in",
            std::env::consts::OS
        );
    }

    /// **On a Mac, the Windows build is never the one selected.** Stated as
    /// its own test because it is the exact sentence that was false, and
    /// because it fails loudly on the machine the bug was reported from.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_mac_never_selects_the_windows_build() {
        assert_ne!(
            PLATFORM, "windows-x86_64",
            "a Mac is asking for the Windows build again"
        );
        assert_eq!(PLATFORM, "macos-universal");
    }

    /// The asset name this searches for is the one the pipeline publishes:
    /// `<crate>-<label>.json`. Both platforms' manifests sit in the same
    /// release, so the suffix has to tell them apart --- and it is the
    /// separator that makes it able to.
    #[test]
    fn the_asset_searched_for_is_this_platforms_and_not_the_other() {
        let want = format!("-{PLATFORM}.json");
        assert!(
            format!("noob-q{want}").ends_with(&want),
            "the suffix must match this platform's real asset name"
        );
        // The other platform's manifest is published beside it, under the
        // same prefix, and must not be picked up.
        let other = if cfg!(target_os = "macos") {
            "noob-q-windows-x86_64.json"
        } else {
            "noob-q-macos-universal.json"
        };
        assert!(
            !other.ends_with(&want),
            "{other} matches the suffix `{want}` this platform searches for"
        );
    }
}
