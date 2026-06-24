//! The on-disk output cache.
//!
//! A `persist` node's whole output set is one content-addressed blob at
//! `<root>/<hex(digest)>` — the [`Digest`] (computed from the flattened node by
//! the [`DigestEngine`](crate::execution::digest::DigestEngine)) names the file,
//! so the same computation resolves to the same blob on any machine. [`load`] and
//! [`store`] turn that blob to/from a node's `Vec<DynamicValue>` through the
//! registered [`CustomValueCodec`](crate::value_codec::CustomValueCodec)s.
//!
//! An in-RAM presence index keeps repeat lookups/writes off the filesystem (a
//! known miss isn't re-`read`, a written blob isn't re-`stat`'d or rewritten). See
//! `docs/disk-cache-design.md`.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use thiserror::Error;

use crate::context::ContextManager;
use crate::data::DynamicValue;
use crate::execution::cache::Cache;
use crate::execution::digest::Digest;
use crate::execution::plan::ExecutionPlan;
use crate::execution::program::ExecutionProgram;
use crate::value_codec::{self, CustomValueRegistry, deserialize_outputs, serialize_outputs};

/// Disk-backed output cache: a content-addressed blob directory plus the codec
/// registry that turns a node's outputs into bytes and back.
#[derive(Debug)]
pub(crate) struct DiskCache {
    root: PathBuf,
    /// In-RAM presence index: `true` = blob known on disk, `false` = known absent,
    /// missing = never checked. Lets repeated lookups/writes for one digest skip
    /// the filesystem. Best-effort: content addressing means a stale entry only
    /// ever costs a missed hit or a recompute, never wrong bytes, and a real read
    /// self-corrects a stale `true` (an externally-deleted file reads as `NotFound`
    /// and flips back to `false`).
    presence: Mutex<HashMap<Digest, bool>>,
    registry: CustomValueRegistry,
}

/// A real failure while storing outputs (a `None`/miss is *not* an error — it's
/// an `Ok` outcome on each side).
#[derive(Debug, Error)]
pub(crate) enum Error {
    #[error("encoding outputs for the cache failed: {0}")]
    Encode(#[from] value_codec::Error),
    #[error("writing a cache blob failed: {0}")]
    Write(#[from] io::Error),
}

impl DiskCache {
    // `new`/`default_root` have no production caller until the worker wires the
    // disk cache (Phase 8) — tests construct it directly. `load`/`store` below are
    // live (the engine calls them).
    #[allow(dead_code)]
    pub(crate) fn new(root: impl Into<PathBuf>, registry: CustomValueRegistry) -> Self {
        Self {
            root: root.into(),
            presence: Mutex::new(HashMap::new()),
            registry,
        }
    }

    /// The machine-global default location: `$XDG_CACHE_HOME/scenarium`, else
    /// `$HOME/.cache/scenarium`, else `<tempdir>/scenarium`.
    #[allow(dead_code)]
    pub(crate) fn default_root() -> PathBuf {
        if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
            return PathBuf::from(xdg).join("scenarium");
        }
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(".cache").join("scenarium");
        }
        std::env::temp_dir().join("scenarium")
    }

    /// Load the outputs cached under `digest`, or `None` on a miss or a blob that
    /// no longer decodes (corrupt, or a custom type whose codec is gone) — both
    /// are a miss, so the node simply recomputes.
    pub(crate) fn load(&self, digest: &Digest) -> Option<Vec<DynamicValue>> {
        let bytes = self.read_blob(digest)?;
        match deserialize_outputs(bytes, &self.registry) {
            Ok(values) => Some(values),
            Err(e) => {
                tracing::warn!(error = %e, "cached outputs failed to decode; recomputing");
                None
            }
        }
    }

    /// Serialize `outputs` and store them under `digest`. A node that isn't
    /// cacheable (a custom output has no registered codec) is silently skipped —
    /// `Ok(())` with nothing written, same as a node already present. `Err` is a
    /// real encode or write failure for the caller to surface.
    pub(crate) async fn store(
        &self,
        digest: &Digest,
        outputs: &[DynamicValue],
        ctx: &mut ContextManager,
    ) -> Result<(), Error> {
        // Already on disk (or written this session) ⇒ skip the *serialize* too, not
        // just the write — that's the costly part (an image encode / GPU readback).
        // The presence index makes this a RAM check, no fs stat.
        if self.contains(digest) {
            return Ok(());
        }
        let blob = match serialize_outputs(outputs, &self.registry, ctx).await {
            Ok(blob) => blob,
            // A custom output with no registered codec ⇒ the node isn't cacheable;
            // skip it (not a failure).
            Err(value_codec::Error::UnknownType(_)) => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        self.write_blob(digest, &blob)?;
        Ok(())
    }

    // === Content-addressed blob I/O ===

    fn blob_path(&self, digest: &Digest) -> PathBuf {
        self.root.join(hex(digest))
    }

    /// Whether a blob for `digest` is present, without reading it. The RAM index
    /// answers a known digest; an unknown one costs one `exists` check, memoized.
    fn contains(&self, digest: &Digest) -> bool {
        if let Some(&present) = self.presence.lock().unwrap().get(digest) {
            return present;
        }
        let present = self.blob_path(digest).exists();
        self.presence.lock().unwrap().insert(*digest, present);
        present
    }

    /// Read the blob, or `None` on a miss. A digest already found absent returns
    /// `None` with no filesystem touch. A read error is treated as a miss (the
    /// cache is best-effort — recompute), logged at warn but *not* memoized (it may
    /// be transient).
    fn read_blob(&self, digest: &Digest) -> Option<Vec<u8>> {
        if self.presence.lock().unwrap().get(digest) == Some(&false) {
            return None; // negative cache: already known absent
        }
        let path = self.blob_path(digest);
        match std::fs::read(&path) {
            Ok(bytes) => {
                self.presence.lock().unwrap().insert(*digest, true);
                Some(bytes)
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                self.presence.lock().unwrap().insert(*digest, false);
                None
            }
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "cache read failed; treating as miss");
                None
            }
        }
    }

    /// Store `bytes` under `digest`. The write goes to a per-call temp file and is
    /// `rename`d into place (atomic on one filesystem), so a reader never sees a
    /// half-written blob and concurrent writers can't corrupt one. Records presence
    /// in RAM on success. (`store` already short-circuits a known-present digest, so
    /// no extra check here beyond the rename race.)
    fn write_blob(&self, digest: &Digest, bytes: &[u8]) -> io::Result<()> {
        let final_path = self.blob_path(digest);
        std::fs::create_dir_all(&self.root)?;
        let tmp = self.temp_path(digest);
        std::fs::write(&tmp, bytes)?;
        let landed = match std::fs::rename(&tmp, &final_path) {
            Ok(()) => true,
            // Tolerate a race where another writer landed the same digest first;
            // either way the temp file must not linger.
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                if final_path.exists() {
                    true
                } else {
                    return Err(e);
                }
            }
        };
        if landed {
            self.presence.lock().unwrap().insert(*digest, true);
        }
        Ok(())
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

/// Coordinates the optional disk cache with the RAM [`Cache`] around a run:
/// hydrates persisted `persist` outputs into RAM before planning, and persists
/// freshly-computed ones after. A `None` inner cache makes both no-ops — the
/// memory-only default — so the engine never branches on disk presence itself.
#[derive(Debug, Default)]
pub(crate) struct DiskCacheLayer {
    inner: Option<DiskCache>,
}

impl DiskCacheLayer {
    #[allow(dead_code)] // no production caller until the worker wires the cache.
    pub(crate) fn set(&mut self, cache: DiskCache) {
        self.inner = Some(cache);
    }

    /// Pull any disk-cached `persist` output into its slot, so a digest the disk
    /// already holds becomes a plain RAM hit for the planner. A no-op without a
    /// disk cache, for a node whose RAM already matches, or for one with no digest
    /// (an impure cone — not reproducible, so never disk-cached).
    pub(crate) fn load_into(&self, program: &ExecutionProgram, cache: &mut Cache) {
        let Some(disk) = self.inner.as_ref() else {
            return;
        };
        for idx in 0..program.e_nodes.len() {
            if !program.e_nodes[idx].persist {
                continue;
            }
            let Some(digest) = cache.current_digest(idx) else {
                continue;
            };
            if cache.is_hit(idx) {
                continue; // RAM already holds it for this digest.
            }
            if let Some(values) = disk.load(&digest) {
                cache.hydrate(idx, values, digest);
            }
        }
    }

    /// Snapshot the `(digest, outputs)` of every `persist` node that ran this
    /// round, ready to hand to [`Self::store_pending`]. Done synchronously and
    /// up front so the non-`Sync` [`Cache`] borrow never crosses an await; the
    /// clone is cheap (custom values are `Arc`). Empty without a disk cache.
    pub(crate) fn pending_persists(
        &self,
        program: &ExecutionProgram,
        plan: &ExecutionPlan,
        cache: &Cache,
    ) -> Vec<(Digest, Vec<DynamicValue>)> {
        let mut pending = Vec::new();
        if self.inner.is_none() {
            return pending;
        }
        for &idx in &plan.execute_order {
            if !program.e_nodes[idx].persist {
                continue;
            }
            let Some(digest) = cache.current_digest(idx) else {
                continue;
            };
            if let Some(outputs) = cache.output_values(idx) {
                pending.push((digest, outputs.clone()));
            }
        }
        pending
    }

    /// Persist the snapshot from [`Self::pending_persists`]. Best-effort: a store
    /// failure is logged, not propagated — caching never fails a run. The store
    /// short-circuits a digest already on disk (a RAM check via the blob store's
    /// presence index), so an unchanged re-run costs nothing.
    pub(crate) async fn store_pending(
        &self,
        pending: Vec<(Digest, Vec<DynamicValue>)>,
        ctx: &mut ContextManager,
    ) {
        let Some(disk) = self.inner.as_ref() else {
            return;
        };
        for (digest, outputs) in &pending {
            if let Err(e) = disk.store(digest, outputs, ctx).await {
                tracing::warn!(error = %e, "failed to persist node outputs to disk cache");
            }
        }
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
    use crate::data::{CustomValue, StaticValue, TypeDef};
    use std::any::Any;
    use std::fmt;
    use std::path::Path;
    use std::sync::Arc;

    /// A unique temp directory removed on drop, so tests don't collide or leak.
    #[derive(Debug)]
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "scenarium-diskcache-test-{tag}-{}-{n}",
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

    fn cache(dir: &TempDir) -> DiskCache {
        DiskCache::new(&dir.0, CustomValueRegistry::default())
    }

    fn entries(root: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(root)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    // === Blob I/O ===

    #[test]
    fn write_then_read_blob_round_trips_exact_bytes() {
        let dir = TempDir::new("roundtrip");
        let store = cache(&dir);
        let digest = [7u8; 32];
        let bytes = vec![1, 2, 3, 250, 0, 99];

        store.write_blob(&digest, &bytes).unwrap();
        assert_eq!(store.read_blob(&digest), Some(bytes));
    }

    #[test]
    fn read_missing_blob_is_none() {
        let dir = TempDir::new("miss");
        assert_eq!(cache(&dir).read_blob(&[0u8; 32]), None);
    }

    #[test]
    fn contains_reflects_writes() {
        let dir = TempDir::new("contains");
        let store = cache(&dir);
        let digest = [3u8; 32];

        assert!(!store.contains(&digest));
        store.write_blob(&digest, b"x").unwrap();
        assert!(store.contains(&digest));
    }

    #[test]
    fn distinct_digests_are_independent() {
        let dir = TempDir::new("distinct");
        let store = cache(&dir);
        let (d1, d2) = ([1u8; 32], [2u8; 32]);

        store.write_blob(&d1, b"one").unwrap();
        store.write_blob(&d2, b"two").unwrap();

        assert_eq!(store.read_blob(&d1), Some(b"one".to_vec()));
        assert_eq!(store.read_blob(&d2), Some(b"two".to_vec()));
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
        assert_eq!(DiskCache::default_root().file_name().unwrap(), "scenarium");
    }

    // === Presence index (filesystem-access minimization) ===

    #[test]
    fn negative_cache_skips_refetch_after_miss() {
        let dir = TempDir::new("negcache");
        let store = cache(&dir);
        let digest = [4u8; 32];

        // First read records the digest as absent.
        assert_eq!(store.read_blob(&digest), None);
        // Plant the blob behind the store's back.
        std::fs::write(store.blob_path(&digest), b"late").unwrap();
        // The store still answers None — it remembered the miss and never re-reads.
        assert_eq!(store.read_blob(&digest), None);
    }

    #[test]
    fn positive_cache_skips_rewrite_after_write() {
        let dir = TempDir::new("poscache");
        let store = cache(&dir);
        let digest = [5u8; 32];

        store.write_blob(&digest, b"v1").unwrap();
        // Delete the blob behind the store's back.
        std::fs::remove_file(store.blob_path(&digest)).unwrap();
        // `contains` answers from the RAM index without touching the filesystem, so
        // it still reports present even though the file is gone.
        assert!(store.contains(&digest));
    }

    // === load / store (value codec layer) ===

    /// A custom value with no registered codec — never cacheable.
    #[derive(Debug)]
    struct Opaque;

    impl fmt::Display for Opaque {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "Opaque")
        }
    }

    impl CustomValue for Opaque {
        fn type_def(&self) -> Arc<TypeDef> {
            Arc::new(TypeDef {
                type_id: "78391861-24da-4368-a3a5-2a6b7a47f112".into(),
                display_name: "Opaque".into(),
            })
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    #[tokio::test]
    async fn store_then_load_round_trips() {
        let dir = TempDir::new("vroundtrip");
        let cache = cache(&dir);
        let digest = [5u8; 32];
        let outputs = vec![
            DynamicValue::Unbound,
            DynamicValue::Static(StaticValue::Int(7)),
            DynamicValue::Static(StaticValue::String("x".into())),
        ];

        cache
            .store(&digest, &outputs, &mut ContextManager::default())
            .await
            .unwrap();

        let back = cache.load(&digest).expect("hit");
        assert_eq!(back.len(), 3);
        assert!(matches!(back[0], DynamicValue::Unbound));
        assert_eq!(back[1].as_i64(), Some(7));
        assert_eq!(back[2].as_string(), Some("x"));
    }

    #[tokio::test]
    async fn load_unknown_digest_is_miss() {
        let dir = TempDir::new("vmiss");
        assert!(cache(&dir).load(&[1u8; 32]).is_none());
    }

    #[tokio::test]
    async fn load_under_a_different_digest_is_miss() {
        let dir = TempDir::new("wrong");
        let cache = cache(&dir);
        cache
            .store(
                &[1u8; 32],
                &[DynamicValue::Static(StaticValue::Int(1))],
                &mut ContextManager::default(),
            )
            .await
            .unwrap();
        // A changed input would yield a different digest — must not hit.
        assert!(cache.load(&[2u8; 32]).is_none());
    }

    #[tokio::test]
    async fn non_cacheable_custom_is_not_stored() {
        let dir = TempDir::new("noncacheable");
        let cache = cache(&dir);
        let digest = [8u8; 32];
        let outputs = vec![
            DynamicValue::Static(StaticValue::Int(1)),
            DynamicValue::from_custom(Opaque),
        ];

        // No codec for `Opaque` ⇒ `store` writes nothing (a silent skip), so a
        // later load misses.
        cache
            .store(&digest, &outputs, &mut ContextManager::default())
            .await
            .unwrap();
        assert!(cache.load(&digest).is_none());
    }
}
