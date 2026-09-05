//! The manifest a plug-in's pipeline publishes is the contract between two
//! repositories that are released independently, so it is tested against a
//! **real published file** rather than one written here to match the parser.
//!
//! `tests/published-manifest.json` was downloaded from a live `latest`
//! release. A fixture invented alongside the code agrees with the code by
//! construction and proves nothing about what actually ships --- which is the
//! failure this project has spent its time finding elsewhere.

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

const PUBLISHED: &str = include_str!("published-manifest.json");

#[test]
fn a_published_manifest_parses_and_carries_what_an_install_needs() {
    let m: Manifest =
        serde_json::from_str(PUBLISHED).expect("a real published manifest must parse");

    assert_eq!(m.schema, 1, "schema 1 is what this installer understands");
    assert_eq!(m.id, "noob-resonator");
    assert_eq!(m.platform, "windows-x86_64");

    // A checksum that is not 64 hex characters cannot be a SHA-256, and an
    // install that trusted it would verify nothing.
    assert_eq!(m.sha256.len(), 64, "sha256 is 64 hex characters");
    assert!(
        m.sha256.chars().all(|c| c.is_ascii_hexdigit()),
        "sha256 must be hex: {}",
        m.sha256
    );

    // The url must point at the asset the manifest names, or the installer
    // downloads something other than what it checksummed.
    assert!(
        m.url.starts_with("https://github.com/"),
        "url must be a github release asset: {}",
        m.url
    );
    assert!(m.url.ends_with(".zip"), "url must name the zip: {}", m.url);

    assert!(!m.version.is_empty());
    assert!(!m.commit.is_empty());
    assert!(m.built.ends_with('Z'), "built is UTC: {}", m.built);

    // Both formats, each with a directory the installer knows.
    let kinds: Vec<&str> = m.installs.iter().map(|i| i.kind.as_str()).collect();
    assert!(kinds.contains(&"vst3"), "a vst3 is installed: {kinds:?}");
    assert!(kinds.contains(&"clap"), "a clap is installed: {kinds:?}");
    for i in &m.installs {
        assert!(
            i.into == "vst3" || i.into == "clap",
            "`{}` is not a directory this installer knows",
            i.into
        );
        assert!(
            !i.path.contains('/') && !i.path.contains('\\'),
            "a path inside the zip must be a plain name, not a path: {}",
            i.path
        );
    }
}

#[test]
fn the_commit_shortens_and_the_date_is_the_day() {
    let m: Manifest = serde_json::from_str(PUBLISHED).unwrap();
    assert_eq!(&m.commit[..7], "03d1e9a");
    assert_eq!(m.built.split('T').next().unwrap(), "2026-09-05");
}

/// A manifest from a later schema must be refused rather than guessed at.
#[test]
fn a_newer_schema_is_still_readable_so_it_can_be_refused_by_number() {
    let later = PUBLISHED.replace("\"schema\": 1", "\"schema\": 2");
    let m: Manifest = serde_json::from_str(&later).expect("a later schema still parses");
    assert_eq!(
        m.schema, 2,
        "the number must survive parsing, because that is what the refusal reads"
    );
}

/// Truncated JSON is a failed download, not a manifest.
#[test]
fn a_truncated_manifest_is_rejected() {
    let half = &PUBLISHED[..PUBLISHED.len() / 2];
    assert!(
        serde_json::from_str::<Manifest>(half).is_err(),
        "half a manifest must not parse"
    );
}
