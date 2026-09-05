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

/// The platform this build installs. One value today; the manifest carries it
/// so that a second one does not need a new field.
pub const PLATFORM: &str = "windows-x86_64";

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
}

/// One installable part of a bundle, and which directory it belongs in.
#[derive(Debug, Clone, Deserialize)]
pub struct Install {
    /// `vst3` or `clap`, for reporting.
    pub kind: String,
    /// The path inside the zip.
    pub path: String,
    /// Which install directory it goes into: `vst3` or `clap`.
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

/// A token from the environment, if the machine has one. Never required.
fn token() -> Option<String> {
    for k in ["GITHUB_TOKEN", "GH_TOKEN"] {
        if let Ok(v) = std::env::var(k)
            && !v.trim().is_empty()
        {
            return Some(v);
        }
    }
    None
}
