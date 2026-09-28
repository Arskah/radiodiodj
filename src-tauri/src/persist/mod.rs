pub mod config;
pub mod session;

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Replace a file's contents in one step: write a sibling temporary and rename
/// it over the target. A reader never observes a partial file, and a crash
/// between the two leaves the previous contents.
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = tmp_sibling(path);
    fs::write(&tmp, bytes).with_context(|| format!("write {}", tmp.display()))?;
    fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

fn tmp_sibling(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|f| f.to_os_string())
        .unwrap_or_default();
    name.push(".tmp");
    path.with_file_name(name)
}
