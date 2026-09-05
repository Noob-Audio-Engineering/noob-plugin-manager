//! Fetching a build, checking it is the one the manifest describes, and
//! putting it where hosts look.
//!
//! Two rules shape all of it.
//!
//! **Verify before unpacking.** The manifest publishes the zip's SHA-256, and
//! a download that does not match it is not written to disk at all. A
//! half-correct plug-in is worse than none: it loads, it sounds wrong, and
//! nothing says why.
//!
//! **Never leave a bundle half-replaced.** A plug-in open in a host holds its
//! file, so an install can fail partway through and leave a bundle that is
//! neither the old build nor the new one. Everything is staged beside the
//! target and moved into place only once all of it has arrived, and a locked
//! file is reported as a locked file rather than as a mysterious failure.

use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::registry::Manifest;
use crate::state::{Record, install_dir};

/// Download, verify and install one plug-in. Returns the paths it created.
pub fn install(agent: &ureq::Agent, m: &Manifest) -> Result<Record, String> {
    let zip = fetch(agent, m)?;
    let staged = std::env::temp_dir().join(format!("noob-install-{}-{}", m.id, std::process::id()));
    // A stale staging directory from an interrupted run must not be mistaken
    // for this one's work.
    let _ = std::fs::remove_dir_all(&staged);
    std::fs::create_dir_all(&staged).map_err(|e| format!("{}: {e}", staged.display()))?;

    let result = (|| {
        unpack(&zip, &staged)?;
        let mut created = Vec::new();
        for part in &m.installs {
            let dir = install_dir(&part.into)?;
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("{} cannot be created: {e}", dir.display()))?;
            let from = staged.join(&part.path);
            if !from.exists() {
                return Err(format!(
                    "the build does not contain `{}`, which its own manifest says it does",
                    part.path
                ));
            }
            let to = dir.join(&part.path);
            replace(&from, &to)?;
            created.push(to);
        }
        Ok(created)
    })();

    let _ = std::fs::remove_dir_all(&staged);
    let created = result?;

    Ok(Record {
        version: m.version.clone(),
        commit: m.commit.clone(),
        installed: now(),
        paths: created,
    })
}

/// Remove everything a previous install of `id` created.
pub fn uninstall(record: &Record) -> Result<usize, String> {
    let mut removed = 0;
    for p in &record.paths {
        if !p.exists() {
            continue;
        }
        let r = if p.is_dir() {
            std::fs::remove_dir_all(p)
        } else {
            std::fs::remove_file(p)
        };
        match r {
            Ok(()) => removed += 1,
            Err(e) => return Err(locked(p, &e)),
        }
    }
    Ok(removed)
}

fn fetch(agent: &ureq::Agent, m: &Manifest) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    agent
        .get(&m.url)
        .call()
        .map_err(|e| format!("downloading {}: {e}", m.url))?
        .into_reader()
        .read_to_end(&mut body)
        .map_err(|e| format!("reading the download: {e}"))?;

    let got = hex(Sha256::digest(&body).as_slice());
    if got != m.sha256.to_lowercase() {
        return Err(format!(
            "the download does not match the checksum its manifest publishes.\n  \
             expected {}\n  received {}\n  \
             Nothing was written. Try again; if it repeats, the release is wrong rather than the network.",
            m.sha256, got
        ));
    }
    Ok(body)
}

fn unpack(bytes: &[u8], into: &Path) -> Result<(), String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| format!("the download is not a readable zip: {e}"))?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        // `enclosed_name` refuses paths that would escape the directory, so a
        // malformed archive cannot write outside the staging area.
        let Some(rel) = entry.enclosed_name() else {
            return Err(format!(
                "the archive contains an unsafe path: {}",
                entry.name()
            ));
        };
        let out = into.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        let mut file =
            std::fs::File::create(&out).map_err(|e| format!("{}: {e}", out.display()))?;
        std::io::copy(&mut entry, &mut file).map_err(|e| format!("{}: {e}", out.display()))?;
    }
    Ok(())
}

/// Put `from` at `to`, replacing whatever is there.
fn replace(from: &Path, to: &Path) -> Result<(), String> {
    if to.exists() {
        let r = if to.is_dir() {
            std::fs::remove_dir_all(to)
        } else {
            std::fs::remove_file(to)
        };
        r.map_err(|e| locked(to, &e))?;
    }
    // A rename across volumes fails, and the temp directory is often on
    // another one, so fall back to copying rather than reporting a failure
    // that is really a detail of where Windows put the temp folder.
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    copy_tree(from, to).map_err(|e| locked(to, &std::io::Error::other(e)))
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    if from.is_dir() {
        std::fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
        for entry in std::fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))? {
            let entry = entry.map_err(|e| e.to_string())?;
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to)
            .map(|_| ())
            .map_err(|e| format!("{}: {e}", to.display()))
    }
}

/// The message for a file a host is holding open, which is the ordinary way
/// this fails and deserves to say so rather than reading as corruption.
fn locked(path: &Path, e: &std::io::Error) -> String {
    format!(
        "{} could not be replaced: {e}\n  \
         A plug-in that is loaded is held open by the host. Close the project \
         --- or the whole host --- and run this again.",
        path.display()
    )
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn now() -> String {
    // Seconds since the epoch is enough to order installs and needs no
    // dependency to produce.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

/// Where the parts of `m` would go, for reporting before anything is done.
pub fn targets(m: &Manifest) -> Vec<(String, PathBuf)> {
    m.installs
        .iter()
        .filter_map(|p| {
            install_dir(&p.into)
                .ok()
                .map(|d| (p.kind.clone(), d.join(&p.path)))
        })
        .collect()
}
