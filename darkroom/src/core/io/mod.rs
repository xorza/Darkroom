//! On-disk I/O and session state: file-dialog [`persistence`] for
//! documents/themes/subgraphs, the [`preferences`] that remembers the last theme +
//! document across launches, and the per-document disk-[`cache`] location.

use std::path::PathBuf;

pub mod cache;
pub mod library;
pub mod persistence;
pub mod preferences;

/// `name` resolved relative to the process working directory — the shared
/// path resolution behind the preferences and library files, which both
/// live beside the process rather than in a fixed config dir.
pub(crate) fn cwd_file(name: &str) -> PathBuf {
    std::env::current_dir().unwrap_or_default().join(name)
}
