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
//!
//! **And never leave one half-*installed* without saying so.** That promise
//! held for each part on its own and not for the three together: a build whose
//! VST3 and CLAP were written and whose Audio Unit was refused returned a
//! plain error, and the caller recorded nothing. Two bundles sat in the
//! plug-in folders that the manager did not know it had put there --- listed
//! as "installed, build unknown", and untouchable by `uninstall`, which will
//! not remove what it cannot account for. So a failure carries the parts that
//! did land, and the caller records them.

use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::registry::Manifest;
use crate::state::{Record, install_dir};

/// A failed install, and whatever it managed to put on disk first.
///
/// The parts are installed one at a time, so a refusal on the third leaves the
/// first two in place. Reporting only the error loses them: nothing records
/// them, `uninstall` will not touch what it has no record of, and they sit in
/// the plug-in folder as somebody else's problem.
pub struct Failed {
    /// What went wrong, in the words the caller prints.
    pub error: String,
    /// The parts that were installed before it did, if any. The caller records
    /// this so that what is on disk and what the manager believes is on disk
    /// are the same thing.
    pub partial: Option<Record>,
}

/// Download, verify and install one plug-in. Returns the paths it created, or
/// the failure together with anything that landed before it.
///
/// The error is boxed because it carries a whole `Record`, and an unboxed
/// `Result` would be as large as its failure case on every successful call.
pub fn install(agent: &ureq::Agent, m: &Manifest) -> Result<Record, Box<Failed>> {
    // Nothing below this point has touched a plug-in folder yet, so these
    // failures carry no partial install.
    let zip = fetch(agent, m).map_err(Failed::nothing_landed)?;
    let staged = std::env::temp_dir().join(format!("noob-install-{}-{}", m.id, std::process::id()));
    // A stale staging directory from an interrupted run must not be mistaken
    // for this one's work.
    let _ = std::fs::remove_dir_all(&staged);
    std::fs::create_dir_all(&staged)
        .map_err(|e| Failed::nothing_landed(format!("{}: {e}", staged.display())))?;

    // Held outside the closure so that a failure can still report what was
    // placed before it --- which is the whole point of `Failed::partial`.
    let mut created: Vec<PathBuf> = Vec::new();
    let result = (|created: &mut Vec<PathBuf>| {
        unpack(&zip, &staged)?;
        for part in &m.installs {
            // An Audio Unit in a Windows download is not an error to report,
            // it is a part of the build this machine has no use for.
            if !crate::state::belongs_here(&part.into) {
                continue;
            }
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
        Ok(())
    })(&mut created);

    let _ = std::fs::remove_dir_all(&staged);

    let record = |paths: Vec<PathBuf>| Record {
        version: m.version.clone(),
        commit: m.commit.clone(),
        // The manifest's own word for which build this is, not this machine's
        // guess. They agree on a correct install; recording the manifest's is
        // what makes a wrong one detectable afterwards.
        platform: Some(m.platform.clone()),
        installed: now(),
        paths,
    };

    match result {
        Ok(()) => Ok(record(created)),
        Err(error) => Err(Box::new(Failed {
            error,
            // Only when something actually landed. An empty record would
            // claim an install that never began.
            partial: (!created.is_empty()).then(|| record(created)),
        })),
    }
}

impl Failed {
    /// A failure from before any part was placed.
    fn nothing_landed(error: String) -> Box<Failed> {
        Box::new(Failed {
            error,
            partial: None,
        })
    }
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
        drop(file);

        // **The executable bit, which the archive carries and `File::create`
        // does not.**
        //
        // On Windows this is nothing: there is no such bit, and the plug-in
        // loads either way. On macOS it is the whole install. A `.vst3` is a
        // bundle whose real content is `Contents/MacOS/<name>`, and the
        // system will not load that unless it is executable --- so the
        // directory appears exactly where it should, with the right
        // `Info.plist` and the right binary inside it, and the host finds
        // nothing. Installed and invisible, on one platform only, which is
        // why it survived: every check anyone ran was run here.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // Masked: `unix_mode` hands back the file-type bits too
            // (0o100755 for a regular file), and only the permission bits
            // belong in a `chmod`.
            if let Some(mode) = entry.unix_mode() {
                std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode & 0o777))
                    .map_err(|e| format!("{}: {e}", out.display()))?;
            }
        }
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
    use crate::elevate::Denial;
    match crate::elevate::classify(path) {
        Denial::Directory => format!(
            "{} could not be written: {e}\n  \
             The folder itself refuses writes. Administrator rights would fix \
             this, or you can install into your own plug-in folders instead.\n  \
             {}",
            path.display(),
            crate::gui::ELEVATABLE
        ),
        Denial::FileInUse => format!(
            "{} could not be replaced: {e}\n  \
             A plug-in that is loaded is held open by the host, and \
             administrator rights do not change that. Close the project --- or \
             the whole host --- and try again.",
            path.display()
        ),
        Denial::Other => format!("{} could not be written: {e}", path.display()),
    }
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

/// Where the parts of `m` this machine can host would go, for reporting before
/// anything is done.
///
/// Filtered by `belongs_here`, or a Windows machine reading a macOS manifest
/// would be told it is missing an Audio Unit it has nowhere to put.
pub fn targets(m: &Manifest) -> Vec<(String, PathBuf)> {
    m.installs
        .iter()
        .filter(|p| crate::state::belongs_here(&p.into))
        .filter_map(|p| {
            install_dir(&p.into)
                .ok()
                .map(|d| (p.kind.clone(), d.join(&p.path)))
        })
        .collect()
}

/// The parts `m` says this machine should have that are not on disk.
///
/// **A record is not enough to answer "is this installed".** The formats go in
/// one at a time, so an install can leave the VST3 and CLAP in place and be
/// refused the Audio Unit --- and the record that describes it names the right
/// commit and the right platform, so anything reading only the record calls it
/// up to date. It looks complete in the window and in `list`, and the Audio
/// Unit never appears in any host.
pub fn missing_parts(m: &Manifest) -> Vec<String> {
    targets(m)
        .into_iter()
        .filter(|(_, path)| !path.exists())
        .map(|(kind, _)| format_kind(&kind))
        .collect()
}

/// A format's name as somebody would say it out loud. The manifest's `kind` is
/// a key, and "installed without its au" is not a sentence.
fn format_kind(kind: &str) -> String {
    match kind {
        "au" => "Audio Unit".to_string(),
        "vst3" => "VST3".to_string(),
        "clap" => "CLAP".to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a zip in memory holding one executable and one plain file, the
    /// way a plug-in bundle is shaped.
    fn bundle_zip() -> Vec<u8> {
        use zip::write::SimpleFileOptions;
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let exe = SimpleFileOptions::default().unix_permissions(0o755);
            let plain = SimpleFileOptions::default().unix_permissions(0o644);
            w.start_file("thing.vst3/Contents/MacOS/thing", exe)
                .unwrap();
            std::io::Write::write_all(&mut w, b"\xcf\xfa\xed\xfe not really a binary").unwrap();
            w.start_file("thing.vst3/Contents/Info.plist", plain)
                .unwrap();
            std::io::Write::write_all(&mut w, b"<plist/>").unwrap();
            w.finish().unwrap();
        }
        buf.into_inner()
    }

    /// Every kind the manifests publish reads as a name in a sentence, because
    /// the reason a plug-in is not current is shown to a person: "missing its
    /// Audio Unit", not "missing its au".
    #[test]
    fn every_kind_has_a_name_somebody_would_say() {
        assert_eq!(format_kind("au"), "Audio Unit");
        assert_eq!(format_kind("vst3"), "VST3");
        assert_eq!(format_kind("clap"), "CLAP");
        // A kind from a later manifest is passed through rather than dropped:
        // an unfamiliar name in the message beats a silent omission.
        assert_eq!(format_kind("aax"), "aax");
    }

    /// **A directory that refuses writes is marked for the window.**
    ///
    /// `locked` is what turns a failed write into a message, and the marker it
    /// puts there is the only thing that makes the window offer
    /// administrator. Asserted from the real function rather than from a
    /// string written here, so that changing the wording breaks this instead
    /// of quietly removing the offer.
    #[cfg(unix)]
    #[test]
    fn a_refused_directory_is_marked_elevatable() {
        use std::os::unix::fs::PermissionsExt;

        // A directory this test makes read-only, rather than one borrowed
        // from the system: `/usr/lib` and friends are under SIP and refuse
        // writes with an error that is not `PermissionDenied` at all, which
        // classifies as `Other` and would have made this test a no-op.
        let dir = std::env::temp_dir().join(format!("noob-refuse-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();

        let target = dir.join("x.component");
        // Both taken while the directory is still refusing, because both read
        // it: `locked` classifies the path itself, which is the behaviour
        // under test.
        let got = crate::elevate::classify(&target);
        let msg = locked(
            &target,
            &std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        );
        // Put back before asserting, so a failure does not leave an
        // unremovable directory behind.
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755));
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(
            got,
            crate::elevate::Denial::Directory,
            "the fixture directory was writable, so this proves nothing"
        );
        assert!(
            msg.contains(crate::gui::ELEVATABLE),
            "a refused directory was not marked, so the window cannot offer administrator: {msg}"
        );
        // And it is not on the first line, which is the only part the window's
        // footer keeps --- the reason the marker has to be carried separately.
        assert!(
            !msg.lines().next().unwrap().contains(crate::gui::ELEVATABLE),
            "the marker moved to the first line; `gui::install_these` assumes it is not"
        );
    }

    /// **The binary has to come out executable.**
    ///
    /// A `.vst3` is a bundle whose real content is `Contents/MacOS/<name>`,
    /// and macOS will not load that unless the executable bit is set. The
    /// archive carries the bit and `File::create` does not, so an extractor
    /// that ignores it lays the bundle down perfectly --- right directory,
    /// right `Info.plist`, right bytes --- and the host finds nothing. It is
    /// invisible on macOS and harmless on Windows, which is why it lasted.
    #[test]
    fn unpacking_keeps_the_executable_bit() {
        let dir = std::env::temp_dir().join(format!("noob-unpack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        unpack(&bundle_zip(), &dir).expect("the archive should unpack");

        let bin = dir.join("thing.vst3/Contents/MacOS/thing");
        assert!(bin.exists(), "the binary was not written");

        // The archive must be carrying the bit in the first place, or the
        // extractor has nothing to restore and this test proves nothing.
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(bundle_zip())).unwrap();
        let mode = z
            .by_name("thing.vst3/Contents/MacOS/thing")
            .unwrap()
            .unix_mode();
        assert_eq!(
            mode.map(|m| m & 0o777),
            Some(0o755),
            "the archive did not carry a mode"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let got = std::fs::metadata(&bin).unwrap().permissions().mode() & 0o777;
            assert_eq!(got, 0o755, "the binary came out as {got:o}, not executable");
            let plist = dir.join("thing.vst3/Contents/Info.plist");
            let got = std::fs::metadata(plist).unwrap().permissions().mode() & 0o777;
            assert_eq!(got, 0o644, "a plain file was made executable");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}
