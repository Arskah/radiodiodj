//! What is on disk under the library paths, and the rules that compare it with
//! the library. The scan applies these rules and the library check only reports
//! them, so both go through this module and cannot disagree.

use anyhow::{bail, Context, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use walkdir::WalkDir;

use super::db::IndexRow;
use crate::audio::formats;
use crate::persist::config::Config;

/// A configured library path and the content type it feeds.
pub struct ScanRoot {
    pub content_type: &'static str,
    pub path: String,
}

/// Every configured library path, music first.
pub fn configured_roots(config: &Config) -> Vec<ScanRoot> {
    ["music", "commercial", "jingle"]
        .into_iter()
        .flat_map(|content_type| {
            config
                .get_paths(content_type)
                .into_iter()
                .map(move |path| ScanRoot { content_type, path })
        })
        .collect()
}

/// The audio files found under one root. `complete` is false when part of the
/// tree could not be read, so the listing cannot prove a file is gone.
pub struct Enumeration {
    pub files: Vec<PathBuf>,
    pub complete: bool,
}

/// List the audio files under `dir`, skipping hidden entries. Fails when `dir`
/// itself is not a readable directory — an unmounted share must never look
/// like an empty one.
pub fn find_audio_files(dir: &Path) -> Result<Enumeration> {
    let meta = std::fs::metadata(dir)
        .with_context(|| format!("library path {} is unreachable", dir.display()))?;
    if !meta.is_dir() {
        bail!("library path {} is not a directory", dir.display());
    }
    let mut files = Vec::new();
    let mut complete = true;
    let walk = WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            e.depth() == 0
                || e.file_name()
                    .to_string_lossy()
                    .chars()
                    .next()
                    .map(|c| c != '.')
                    .unwrap_or(true)
        });
    for entry in walk {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                log::warn!("scan: cannot read under {}: {}", dir.display(), e);
                complete = false;
                continue;
            }
        };
        let is_audio = entry
            .path()
            .extension()
            .and_then(|x| x.to_str())
            .map(formats::is_audio_extension)
            .unwrap_or(false);
        if entry.file_type().is_file() && is_audio {
            files.push(entry.into_path());
        }
    }
    Ok(Enumeration { files, complete })
}

pub fn should_rescan(
    existing_content_type: Option<&str>,
    existing_mtime: Option<i64>,
    file_mtime_ms: i64,
    content_type: &str,
) -> bool {
    let Some(prev_ct) = existing_content_type else {
        return true;
    };
    let Some(prev_mtime) = existing_mtime else {
        return true;
    };
    if prev_ct != content_type {
        return true;
    }
    prev_mtime != file_mtime_ms
}

/// A file found on disk, with the content type of the root it was found under.
pub struct Found {
    pub path: String,
    pub content_type: &'static str,
}

impl Found {
    /// Modification time in unix ms, or 0 when it cannot be read.
    pub fn mtime_ms(&self) -> i64 {
        std::fs::metadata(&self.path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }

    /// Whether `row`, the library's record of this path, is out of date.
    pub fn changed(&self, row: &IndexRow, mtime_ms: i64) -> bool {
        should_rescan(
            Some(&row.content_type),
            row.mtime,
            mtime_ms,
            self.content_type,
        )
    }
}

/// Every configured root, listed. A file under two roots is listed once, under
/// the first.
pub struct Listing {
    pub found: Vec<Found>,
    pub seen: HashSet<String>,
    /// Roots listed completely: the only ones that can prove a file gone.
    pub complete: Vec<String>,
    /// Roots listed with unreadable parts.
    pub partial: Vec<String>,
    /// Roots that are not a readable directory.
    pub unreachable: Vec<String>,
}

pub fn list_roots(roots: &[ScanRoot]) -> Listing {
    let mut listing = Listing {
        found: Vec::new(),
        seen: HashSet::new(),
        complete: Vec::new(),
        partial: Vec::new(),
        unreachable: Vec::new(),
    };
    for root in roots {
        match find_audio_files(Path::new(&root.path)) {
            Ok(enumeration) => {
                if enumeration.complete {
                    listing.complete.push(root.path.clone());
                } else {
                    log::warn!("scan: {} was listed partially; not pruning it", root.path);
                    listing.partial.push(root.path.clone());
                }
                for file in enumeration.files {
                    let path = file.to_string_lossy().into_owned();
                    if listing.seen.insert(path.clone()) {
                        listing.found.push(Found {
                            path,
                            content_type: root.content_type,
                        });
                    }
                }
            }
            Err(e) => {
                log::warn!("scan: {e:#}; keeping its tracks");
                listing.unreachable.push(root.path.clone());
            }
        }
    }
    listing
}

impl Listing {
    /// Present rows whose file is gone: not listed, and either under a root
    /// listed completely or under no configured root at all. Roots are matched
    /// by path component, so `/Music` does not contain `/Music2`.
    pub fn gone<'a>(
        &'a self,
        index: &'a [IndexRow],
        roots: &'a [ScanRoot],
    ) -> impl Iterator<Item = &'a IndexRow> + 'a {
        index
            .iter()
            .filter(|row| row.missing_since.is_none() && !self.seen.contains(&row.path))
            .filter(|row| {
                let path = Path::new(&row.path);
                let configured = roots.iter().any(|r| path.starts_with(&r.path));
                !configured || self.complete.iter().any(|root| path.starts_with(root))
            })
    }
}
