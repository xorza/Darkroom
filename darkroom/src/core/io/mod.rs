//! On-disk I/O and session state: zipped [`document`] archives, the
//! preferences and the per-document
//! disk-[`cache`] location.

use std::path::PathBuf;

pub(crate) mod cache;
pub(crate) mod document;
pub(crate) mod preferences;

/// `name` resolved relative to the process working directory — the shared
/// path resolution behind the preferences and library files, which both
/// live beside the process rather than in a fixed config dir.
fn cwd_file(name: &str) -> PathBuf {
    std::env::current_dir().unwrap_or_default().join(name)
}
