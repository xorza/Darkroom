//! Per-document on-disk cache location. A node with a disk-backed `CacheMode`
//! (`Disk`/`Both`) persists its output to a content-addressed store; darkroom roots that store
//! beside the document file so the cache travels with the project rather than
//! polluting a machine-global directory. An unsaved document has no path, so it
//! stays memory-only until first save.

use std::path::{Path, PathBuf};

/// The cache directory for a document: `<stem>.darkroom-cache/` beside the
/// document file (e.g. `proj/scene.rhai` → `proj/scene.darkroom-cache/`).
/// Per-document-named so two projects in one folder keep separate stores.
pub(crate) fn document_cache_root(doc_path: &Path) -> PathBuf {
    let stem = doc_path.file_stem().unwrap_or_default();
    let mut name = stem.to_os_string();
    name.push(".darkroom-cache");
    doc_path.with_file_name(name)
}

/// The document's content-addressed store root, ensuring the dir and a
/// self-ignoring `.gitignore` exist. Save-As / moving the project does *not* carry
/// the cache along — each location keeps its own store, content-addressed so the
/// new one refills lazily.
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
        let _ = std::fs::write(&gitignore, "*\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn cache_root_is_named_after_stem_beside_the_file() {
        // Absolute path: cache sits in the same dir, stem + `.darkroom-cache`.
        assert_eq!(
            document_cache_root(Path::new("/proj/scene.rhai")),
            PathBuf::from("/proj/scene.darkroom-cache")
        );
        // Relative path keeps its (empty) parent.
        assert_eq!(
            document_cache_root(Path::new("scene.rhai")),
            PathBuf::from("scene.darkroom-cache")
        );
        // No extension → the whole filename is the stem.
        assert_eq!(
            document_cache_root(Path::new("/proj/scene")),
            PathBuf::from("/proj/scene.darkroom-cache")
        );
        // Two projects in one dir get distinct stores.
        assert_ne!(
            document_cache_root(Path::new("/proj/a.rhai")),
            document_cache_root(Path::new("/proj/b.rhai"))
        );
    }

    #[test]
    fn build_creates_dir_and_self_ignoring_gitignore() {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("darkroom-cache-test-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let doc_path = dir.join("scene.rhai");

        let root = prepare_document_cache_root(&doc_path);

        assert_eq!(root, dir.join("scene.darkroom-cache"));
        assert!(root.is_dir(), "cache dir created beside the document");
        let gitignore = root.join(".gitignore");
        assert_eq!(
            std::fs::read_to_string(&gitignore).unwrap(),
            "*\n",
            "the cache folder ignores its own contents"
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
