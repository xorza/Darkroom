//! On-disk I/O and session state: zipped [`project`] documents, reusable-graph
//! [`persistence`], the [`preferences`] that remember the last theme + document
//! across launches, and the per-document disk-[`cache`] location.

use std::path::PathBuf;

pub(crate) mod cache;
pub(crate) mod library;
pub(crate) mod persistence;
pub(crate) mod preferences;
pub(crate) mod project;

/// `name` resolved relative to the process working directory — the shared
/// path resolution behind the preferences and library files, which both
/// live beside the process rather than in a fixed config dir.
pub(crate) fn cwd_file(name: &str) -> PathBuf {
    std::env::current_dir().unwrap_or_default().join(name)
}
