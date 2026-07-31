//! Per-document on-disk cache location. A node with a disk-backed `CacheMode`
//! (`Disk`/`Both`) persists its output to a blob store; darkroom roots that store
//! beside the document file so the cache travels with the project rather than
//! polluting a machine-global directory. An unsaved document has no path, so it
//! stays memory-only until first save.

use std::path::{Path, PathBuf};

use common::file_utils;

/// The cache directory for a document: `<stem>.darkroom-cache/` beside the
/// document file (e.g. `proj/scene.darkroom` → `proj/scene.darkroom-cache/`).
/// Per-document-named so two projects in one folder keep separate stores.
pub(crate) fn document_cache_root(doc_path: &Path) -> PathBuf {
    let stem = doc_path.file_stem().unwrap_or_default();
    let mut name = stem.to_os_string();
    name.push(".darkroom-cache");
    doc_path.with_file_name(name)
}

/// The document's blob-store root, ensuring the dir and a self-ignoring
/// `.gitignore` exist. Save-As / moving the project does *not* carry the cache
/// along — each location keeps its own store, which refills lazily as nodes
/// recompute.
pub(crate) fn prepare_document_cache_root(doc_path: &Path) -> PathBuf {
    let root = document_cache_root(doc_path);
    ensure_gitignore(&root);
    root
}

/// Best-effort: create `root` and drop a `*`-pattern `.gitignore`, so the whole
/// cache folder (blobs + the ignore file itself) stays out of version control.
/// A failure just means no `.gitignore` yet — the cache still works, since blob
/// writes recreate the dir.
fn ensure_gitignore(root: &Path) {
    if std::fs::create_dir_all(root).is_err() {
        return;
    }
    let gitignore = root.join(".gitignore");
    if !gitignore.exists() {
        let _ = file_utils::publish_bytes(&gitignore, b"*\n", file_utils::PublicationMode::Cache);
    }
}

#[cfg(test)]
mod tests;
