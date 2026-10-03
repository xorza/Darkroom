//! Per-document on-disk cache location. A node with a disk-backed `CacheMode`
//! (`Disk`/`Both`) persists its output to a blob store; darkroom roots that store
//! beside the document file so the cache travels with the project rather than
//! polluting a machine-global directory. An unsaved document has no path, so it
//! stays memory-only until first save.

use std::fs;
use std::path::{Path, PathBuf};

use common::file_utils;

use crate::core::io::document::EXTENSION;

/// The cache directory for a document: `<stem>.<EXTENSION>-cache/` beside the
/// document file (e.g. `proj/scene.darkroom` → `proj/scene.darkroom-cache/`).
/// Per-document-named so two projects in one folder keep separate stores.
/// Save-As / moving the project does *not* carry the cache along — each
/// location keeps its own store, which refills lazily as nodes recompute.
pub(crate) fn document_cache_root(doc_path: &Path) -> PathBuf {
    let stem = doc_path.file_stem().unwrap_or_default();
    let mut name = stem.to_os_string();
    name.push(".");
    name.push(EXTENSION);
    name.push("-cache");
    doc_path.with_file_name(name)
}

/// Best-effort: create `root` and drop a `*`-pattern `.gitignore`, so the whole
/// cache folder (blobs + the ignore file itself) stays out of version control.
/// Called before the first blob can land there, not when a document opens, so
/// a document with no disk-backed node leaves its folder untouched. A failure
/// just means no `.gitignore` yet — the cache still works, since blob writes
/// recreate the dir.
pub(crate) fn prepare_cache_root(root: &Path) {
    if fs::create_dir_all(root).is_err() {
        return;
    }
    let gitignore = root.join(".gitignore");
    if !gitignore.exists() {
        let _ = file_utils::publish_bytes(&gitignore, b"*\n", file_utils::PublicationMode::Cache);
    }
}

#[cfg(test)]
mod tests;
