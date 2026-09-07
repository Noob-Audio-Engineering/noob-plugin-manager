//! The manifest a plug-in's pipeline publishes is the contract between two
//! repositories that are released independently, so it is tested against
//! **real published files** rather than ones written here to match the parser.
//!
//! `tests/published-manifest.json` and `tests/published-manifest-macos.json`
//! were downloaded from live `latest` releases. A fixture invented alongside
//! the code agrees with the code by construction and proves nothing about what
//! actually ships --- which is the failure this project has spent its time
//! finding elsewhere.
//!
//! **Both platforms, because for months only one was read.** The installer
//! hard-coded `windows-x86_64` as the platform it looked for, so on macOS it
//! fetched the Windows manifest and installed a Windows DLL into a `.vst3`
//! directory --- and never installed the Audio Unit at all, a Windows build
//! having none. Every test here passed throughout: there was one fixture, and
//! it was the Windows one.

use serde::Deserialize;

// The shape under test, mirrored here because the binary does not expose a
// library target. If these drift, the test stops describing the program ---
// so the fields are the ones `registry.rs` declares, and nothing more.
#[derive(Debug, Deserialize)]
struct Manifest {
    schema: u32,
    id: String,
    version: String,
    commit: String,
    built: String,
    platform: String,
    sha256: String,
    url: String,
    installs: Vec<Install>,
}

#[derive(Debug, Deserialize)]
struct Install {
    kind: String,
    path: String,
    into: String,
}

const PUBLISHED_WINDOWS: &str = include_str!("published-manifest.json");
const PUBLISHED_MACOS: &str = include_str!("published-manifest-macos.json");

/// Both real manifests, named, so a failure says which platform it was.
fn published() -> Vec<(&'static str, Manifest)> {
    [
        ("windows-x86_64", PUBLISHED_WINDOWS),
        ("macos-universal", PUBLISHED_MACOS),
    ]
    .into_iter()
    .map(|(label, text)| {
        let m: Manifest = serde_json::from_str(text)
            .unwrap_or_else(|e| panic!("the published {label} manifest must parse: {e}"));
        (label, m)
    })
    .collect()
}

/// Everything an install needs, on every platform that publishes a build.
#[test]
fn a_published_manifest_parses_and_carries_what_an_install_needs() {
    for (label, m) in published() {
        assert_eq!(
            m.schema, 1,
            "{label}: schema 1 is what this installer understands"
        );
        assert_eq!(m.id, "noob-resonator", "{label}");

        // The label in the asset name and the field inside must agree, or the
        // installer selects a manifest by one and installs a build described
        // by the other.
        assert_eq!(
            m.platform, label,
            "{label}: the manifest names another platform"
        );

        // A checksum that is not 64 hex characters cannot be a SHA-256, and an
        // install that trusted it would verify nothing.
        assert_eq!(m.sha256.len(), 64, "{label}: sha256 is 64 hex characters");
        assert!(
            m.sha256.chars().all(|c| c.is_ascii_hexdigit()),
            "{label}: sha256 must be hex: {}",
            m.sha256
        );

        // The url must point at the asset the manifest names, or the installer
        // downloads something other than what it checksummed.
        assert!(
            m.url.starts_with("https://github.com/"),
            "{label}: url must be a github release asset: {}",
            m.url
        );
        assert!(
            m.url.ends_with(".zip"),
            "{label}: url must name the zip: {}",
            m.url
        );

        // And it must be *this* platform's asset. A manifest pointing at the
        // other platform's zip is exactly the shape of the bug this file
        // exists to catch, and every field above would still be valid.
        assert!(
            m.url.contains(label),
            "{label}: the url does not name this platform's asset: {}",
            m.url
        );

        assert!(!m.version.is_empty(), "{label}");
        assert!(!m.commit.is_empty(), "{label}");
        assert!(m.built.ends_with('Z'), "{label}: built is UTC: {}", m.built);

        let kinds: Vec<&str> = m.installs.iter().map(|i| i.kind.as_str()).collect();
        assert!(
            kinds.contains(&"vst3"),
            "{label}: a vst3 is installed: {kinds:?}"
        );
        assert!(
            kinds.contains(&"clap"),
            "{label}: a clap is installed: {kinds:?}"
        );
        for i in &m.installs {
            assert!(
                matches!(i.into.as_str(), "vst3" | "clap" | "au"),
                "{label}: `{}` is not a directory this installer knows",
                i.into
            );
            assert!(
                !i.path.contains('/') && !i.path.contains('\\'),
                "{label}: a path inside the zip must be a plain name, not a path: {}",
                i.path
            );
        }
    }
}

/// **The macOS build carries an Audio Unit and the Windows one does not.**
///
/// This is what the installer was silently getting wrong: reading the Windows
/// manifest on a Mac, it found two parts instead of three and installed no
/// Audio Unit, which is not a failure any of its own checks could report ---
/// it installed exactly what the manifest it was holding described.
#[test]
fn only_the_macos_build_carries_an_audio_unit() {
    for (label, m) in published() {
        let au = m.installs.iter().find(|i| i.into == "au");
        match label {
            "macos-universal" => {
                let au = au.unwrap_or_else(|| {
                    panic!("the macOS build must carry an Audio Unit, and this one carries none")
                });
                assert_eq!(au.kind, "au");
                assert!(
                    au.path.ends_with(".component"),
                    "an Audio Unit is a `.component`, not {}",
                    au.path
                );
            }
            _ => assert!(
                au.is_none(),
                "{label} carries an Audio Unit, which is a macOS format"
            ),
        }
    }
}

#[test]
fn the_commit_shortens_and_the_date_is_the_day() {
    let m: Manifest = serde_json::from_str(PUBLISHED_WINDOWS).unwrap();
    assert_eq!(&m.commit[..7], "03d1e9a");
    assert_eq!(m.built.split('T').next().unwrap(), "2026-09-05");
}

/// A manifest from a later schema must be refused rather than guessed at.
#[test]
fn a_newer_schema_is_still_readable_so_it_can_be_refused_by_number() {
    let later = PUBLISHED_WINDOWS.replace("\"schema\": 1", "\"schema\": 2");
    let m: Manifest = serde_json::from_str(&later).expect("a later schema still parses");
    assert_eq!(
        m.schema, 2,
        "the number must survive parsing, because that is what the refusal reads"
    );
}

/// Truncated JSON is a failed download, not a manifest.
#[test]
fn a_truncated_manifest_is_rejected() {
    let half = &PUBLISHED_WINDOWS[..PUBLISHED_WINDOWS.len() / 2];
    assert!(
        serde_json::from_str::<Manifest>(half).is_err(),
        "half a manifest must not parse"
    );
}
