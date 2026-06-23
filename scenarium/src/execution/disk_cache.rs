//! Glue between the content digest, the value codec, and the blob store: load a
//! node's cached outputs by digest, or serialize-and-store them under it. Keyed
//! purely by [`Digest`] — the caller computes that from the flattened node via
//! the [`DigestEngine`](crate::execution::digest::DigestEngine), so this layer
//! owns no scheduling and no program. It's the seam the executor drives in the
//! integration phase. See `scenarium/docs/disk-cache-design.md`.

use thiserror::Error;

use crate::context::ContextManager;
use crate::data::DynamicValue;
use crate::execution::cache_store::CacheStore;
use crate::execution::digest::Digest;
use crate::value_cache::{self, CustomValueRegistry, deserialize_outputs, serialize_outputs};

/// Disk-backed output cache: the blob store plus the codec registry that turns a
/// node's `Vec<DynamicValue>` into bytes and back. The whole output set is one
/// blob keyed by the node's digest, since they all come from one computation.
#[derive(Debug)]
pub(crate) struct DiskCache {
    blobs: CacheStore,
    registry: CustomValueRegistry,
}

/// A real failure while storing outputs (a `None`/miss is *not* an error — it's
/// an `Ok` outcome on each side).
#[derive(Debug, Error)]
pub(crate) enum Error {
    #[error("encoding outputs for the cache failed: {0}")]
    Encode(#[from] value_cache::Error),
    #[error("writing a cache blob failed: {0}")]
    Write(#[from] std::io::Error),
}

impl DiskCache {
    pub(crate) fn new(blobs: CacheStore, registry: CustomValueRegistry) -> Self {
        Self { blobs, registry }
    }

    /// Whether outputs for `digest` are present, without reading them.
    pub(crate) fn contains(&self, digest: &Digest) -> bool {
        self.blobs.contains(digest)
    }

    /// Load the outputs cached under `digest`, or `None` on a miss or a blob that
    /// no longer decodes (corrupt, or a custom type whose codec is gone) — both
    /// are a miss, so the node simply recomputes.
    pub(crate) fn load(&self, digest: &Digest) -> Option<Vec<DynamicValue>> {
        let bytes = self.blobs.get(digest)?;
        match deserialize_outputs(&bytes, &self.registry) {
            Ok(values) => Some(values),
            Err(e) => {
                tracing::warn!(error = %e, "cached outputs failed to decode; recomputing");
                None
            }
        }
    }

    /// Serialize `outputs` and store them under `digest`. `Ok(false)` when the
    /// node isn't cacheable (a custom output has no registered codec) — nothing
    /// is written; `Ok(true)` when stored. `Err` is a real encode or write
    /// failure for the caller to surface.
    pub(crate) async fn store(
        &self,
        digest: &Digest,
        outputs: &[DynamicValue],
        ctx: &mut ContextManager,
    ) -> Result<bool, Error> {
        let Some(blob) = serialize_outputs(outputs, &self.registry, ctx).await? else {
            return Ok(false);
        };
        self.blobs.put(digest, &blob)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::{CustomValue, StaticValue, TypeDef};
    use std::any::Any;
    use std::fmt;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

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
        DiskCache::new(CacheStore::new(&dir.0), CustomValueRegistry::default())
    }

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
        let dir = TempDir::new("roundtrip");
        let cache = cache(&dir);
        let digest = [5u8; 32];
        let outputs = vec![
            DynamicValue::Unbound,
            DynamicValue::Static(StaticValue::Int(7)),
            DynamicValue::Static(StaticValue::String("x".into())),
        ];

        assert!(
            cache
                .store(&digest, &outputs, &mut ContextManager::default())
                .await
                .unwrap()
        );

        let back = cache.load(&digest).expect("hit");
        assert_eq!(back.len(), 3);
        assert!(matches!(back[0], DynamicValue::Unbound));
        assert_eq!(back[1].as_i64(), Some(7));
        assert_eq!(back[2].as_string(), Some("x"));
    }

    #[tokio::test]
    async fn load_unknown_digest_is_miss() {
        let dir = TempDir::new("miss");
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
    async fn contains_reflects_store() {
        let dir = TempDir::new("contains");
        let cache = cache(&dir);
        let digest = [3u8; 32];

        assert!(!cache.contains(&digest));
        cache
            .store(
                &digest,
                &[DynamicValue::Static(StaticValue::Bool(true))],
                &mut ContextManager::default(),
            )
            .await
            .unwrap();
        assert!(cache.contains(&digest));
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

        // No codec for `Opaque` ⇒ `store` reports not-cacheable and writes nothing.
        assert!(
            !cache
                .store(&digest, &outputs, &mut ContextManager::default())
                .await
                .unwrap()
        );
        assert!(!cache.contains(&digest));
        assert!(cache.load(&digest).is_none());
    }
}
