//! Per-document on-disk cache location. A node with a disk-backed `CacheMode`
//! (`Disk`/`Both`) persists its output to a blob store; darkroom roots that store
//! beside the document file so the cache travels with the project rather than
//! polluting a machine-global directory. An unsaved document has no path, so it
//! stays memory-only until first save.

use std::io;
use std::path::{Path, PathBuf};

use common::fs;

use crate::core::io::document::document_format::DocumentFormat;

/// The cache directory for a document: `<stem>.<extension>-cache/` beside the
/// document file, the extension in its lower case (e.g. `proj/scene.darkroom` →
/// `proj/scene.darkroom-cache/`, `proj/scene.darkroomz` → `proj/scene.darkroomz-cache/`).
/// Per-document-named so two projects in one folder keep separate stores.
/// Save-As / moving the project does *not* carry the cache along — each
/// location keeps its own store, which refills lazily as nodes recompute.
pub(crate) fn root(document_path: &Path) -> PathBuf {
    let format = DocumentFormat::of_path(document_path)
        .expect("a document's path carries a project extension");
    let stem = document_path.file_stem().unwrap_or_default();
    let mut name = stem.to_os_string();
    name.push(".");
    name.push(format.extension());
    name.push("-cache");
    document_path.with_file_name(name)
}

/// Create `root` and drop a `*`-pattern `.gitignore`, so the whole cache
/// folder (blobs + the ignore file itself) stays out of version control.
/// Called before the first blob can land there, not when a document opens, so
/// a document with no disk-backed node leaves its folder untouched. A failure
/// means no `.gitignore` yet — the cache still works, since blob writes
/// recreate the dir — so the caller logs it and tries again later.
pub(crate) fn prepare(root: &Path) -> io::Result<()> {
    fs::create_dir_all(root)?;
    let gitignore = root.join(".gitignore");
    if !fs::exists(&gitignore) {
        fs::publish_bytes(&gitignore, b"*\n", fs::PublicationMode::Cache)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
