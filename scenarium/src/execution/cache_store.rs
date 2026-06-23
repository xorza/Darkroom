//! Content-addressed blob store for cached node outputs.
//!
//! A flat directory keyed by the 32-byte BLAKE3 [`Digest`]: the file at
//! `<root>/<hex(digest)>` *is* the blob. Content addressing makes writes
//! idempotent and reads trustless — a present entry is, by construction, the
//! bytes that hash to its name, so the same computation on any machine resolves
//! to the same entry. Garbage collection and the exportable sidecar bundle land
//! in a later phase; this is just `get`/`put`/`contains`. See
//! `scenarium/docs/disk-cache-design.md`.

use std::fmt::Write as _;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::execution::digest::Digest;

/// A content-addressed blob store rooted at a directory.
#[derive(Debug)]
pub(crate) struct CacheStore {
    root: PathBuf,
}

impl CacheStore {
    pub(crate) fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The machine-global default location: `$XDG_CACHE_HOME/scenarium`, else
    /// `$HOME/.cache/scenarium`, else `<tempdir>/scenarium`.
    pub(crate) fn default_root() -> PathBuf {
        if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
            return PathBuf::from(xdg).join("scenarium");
        }
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(".cache").join("scenarium");
        }
        std::env::temp_dir().join("scenarium")
    }

    fn blob_path(&self, digest: &Digest) -> PathBuf {
        self.root.join(hex(digest))
    }

    /// Whether an entry for `digest` is present, without reading it — for the
    /// planner to decide a load before paying the read.
    pub(crate) fn contains(&self, digest: &Digest) -> bool {
        self.blob_path(digest).exists()
    }

    /// Read the blob, or `None` on a miss. A read error is treated as a miss
    /// (the cache is best-effort — recompute), logged at warn so a persistently
    /// broken cache is visible.
    pub(crate) fn get(&self, digest: &Digest) -> Option<Vec<u8>> {
        let path = self.blob_path(digest);
        match std::fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "cache read failed; treating as miss");
                None
            }
        }
    }

    /// Store `bytes` under `digest`. Idempotent — a present entry is identical by
    /// content addressing, so this is a no-op. The write goes to a per-call temp
    /// file and is `rename`d into place (atomic on one filesystem), so a reader
    /// never sees a half-written blob and concurrent writers can't corrupt one.
    pub(crate) fn put(&self, digest: &Digest, bytes: &[u8]) -> io::Result<()> {
        let final_path = self.blob_path(digest);
        if final_path.exists() {
            return Ok(());
        }
        std::fs::create_dir_all(&self.root)?;
        let tmp = self.temp_path(digest);
        std::fs::write(&tmp, bytes)?;
        match std::fs::rename(&tmp, &final_path) {
            Ok(()) => Ok(()),
            // Tolerate a race where another writer landed the same digest first;
            // either way the temp file must not linger.
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                if final_path.exists() { Ok(()) } else { Err(e) }
            }
        }
    }

    /// A temp path unique across processes and concurrent calls, so two writers
    /// never share (and interleave into) one temp file.
    fn temp_path(&self, digest: &Digest) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        self.root
            .join(format!("{}.{}.{n}.tmp", hex(digest), std::process::id()))
    }
}

/// Lowercase hex of the digest — the 64-char blob filename.
fn hex(digest: &Digest) -> String {
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// A unique temp directory removed on drop, so tests don't collide or leak.
    #[derive(Debug)]
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "scenarium-cache-test-{tag}-{}-{n}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn entries(root: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(root)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn put_then_get_round_trips_exact_bytes() {
        let dir = TempDir::new("roundtrip");
        let store = CacheStore::new(&dir.0);
        let digest = [7u8; 32];
        let bytes = vec![1, 2, 3, 250, 0, 99];

        store.put(&digest, &bytes).unwrap();
        assert_eq!(store.get(&digest), Some(bytes));
    }

    #[test]
    fn get_missing_digest_is_none() {
        let dir = TempDir::new("miss");
        let store = CacheStore::new(&dir.0);
        // Nothing written, and the root may not even exist yet.
        assert_eq!(store.get(&[0u8; 32]), None);
    }

    #[test]
    fn contains_reflects_presence() {
        let dir = TempDir::new("contains");
        let store = CacheStore::new(&dir.0);
        let digest = [3u8; 32];

        assert!(!store.contains(&digest));
        store.put(&digest, b"x").unwrap();
        assert!(store.contains(&digest));
    }

    #[test]
    fn put_is_idempotent_and_leaves_one_blob() {
        let dir = TempDir::new("idempotent");
        let store = CacheStore::new(&dir.0);
        let digest = [9u8; 32];

        store.put(&digest, b"hello").unwrap();
        store.put(&digest, b"hello").unwrap();

        assert_eq!(store.get(&digest), Some(b"hello".to_vec()));
        // Exactly the blob, named by its hex digest — no leftover temp files.
        assert_eq!(entries(&dir.0), vec![hex(&digest)]);
    }

    #[test]
    fn distinct_digests_are_independent() {
        let dir = TempDir::new("distinct");
        let store = CacheStore::new(&dir.0);
        let (d1, d2) = ([1u8; 32], [2u8; 32]);

        store.put(&d1, b"one").unwrap();
        store.put(&d2, b"two").unwrap();

        assert_eq!(store.get(&d1), Some(b"one".to_vec()));
        assert_eq!(store.get(&d2), Some(b"two".to_vec()));
        assert_eq!(entries(&dir.0).len(), 2);
    }

    #[test]
    fn hex_is_64_lowercase_chars() {
        let mut digest = [0u8; 32];
        digest[0] = 0xab;
        digest[31] = 0x0f;
        let h = hex(&digest);
        assert_eq!(h.len(), 64);
        assert!(h.starts_with("ab"));
        assert!(h.ends_with("0f"));
        assert!(
            h.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    #[test]
    fn default_root_is_named_scenarium() {
        assert_eq!(CacheStore::default_root().file_name().unwrap(), "scenarium");
    }
}
