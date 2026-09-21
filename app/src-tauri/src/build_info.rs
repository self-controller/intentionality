//! Which build is this, and has it fallen behind the source?
//!
//! Autostart points at `target/release/intentionality`, a build artifact that
//! only changes when someone remembers to rebuild. On 2026-08-29 that meant a
//! day-old binary autostarted at login and there was nothing on screen to say
//! so — an old build is indistinguishable from a fresh one until you notice a
//! feature missing. So the app says which build it is, every time.
//!
//! The comparison is timestamps, not the git commit: the working tree here is
//! dirty most of the time, and a commit check would have reported "up to date"
//! for the whole day the binary was stale.

use chrono::{DateTime, Local};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Serialize)]
pub struct BuildInfo {
    pub version: String,
    /// The running binary's own mtime. None only if it cannot be read.
    pub built_at: Option<String>,
    /// The newest source file, when it is newer than the binary. Some(..) is
    /// the whole "you need to rebuild" signal.
    pub stale_since: Option<String>,
}

/// Where the sources live, embedded at compile time. If this path is gone the
/// staleness check is skipped, not failed — a binary copied elsewhere should
/// still run and still report its own build time.
pub(crate) const REPO: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

/// Only what actually ends up in this binary. `gate/` is deliberately absent:
/// editing store.py or schema.sql needs no app rebuild, and warning about it
/// would train the reader to ignore the warning that matters.
const SOURCES: &[&str] = &[
    "app/src",
    "app/src-tauri/src",
    "app/src-tauri/Cargo.toml",
    "app/index.html",
    "app/package.json",
];

fn stamp(t: SystemTime) -> String {
    DateTime::<Local>::from(t).format("%H:%M %b %-d").to_string()
}

/// Newest mtime at or under `path`. Symlinks are reported but never descended
/// into, so a loop cannot hang this.
fn newest(path: &Path) -> Option<SystemTime> {
    let meta = fs::symlink_metadata(path).ok()?;
    if !meta.is_dir() {
        return meta.modified().ok();
    }
    let mut found: Option<SystemTime> = None;
    for entry in fs::read_dir(path).ok()? {
        let Ok(entry) = entry else { continue };
        if let Some(t) = newest(&entry.path()) {
            found = Some(found.map_or(t, |f| f.max(t)));
        }
    }
    found
}

pub fn current() -> BuildInfo {
    let built = std::env::current_exe()
        .ok()
        .and_then(|path| fs::metadata(path).ok())
        .and_then(|meta| meta.modified().ok());

    let stale_since = built.and_then(|built| {
        let repo = PathBuf::from(REPO);
        SOURCES
            .iter()
            .filter_map(|rel| newest(&repo.join(rel)))
            .max()
            .filter(|newest| *newest > built)
            .map(stamp)
    });

    BuildInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        built_at: built.map(stamp),
        stale_since,
    }
}
