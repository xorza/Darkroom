use std::any::Any;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::execution::cache::{CachedOutputCoverage, OutputSnapshot};
use crate::execution::digest::Digest;
use crate::execution::disk_store::header;
use crate::execution::disk_store::{BlobTarget, DiskStore};
use crate::library::{Library, TypeEntry};
use crate::runtime::context::ContextManager;
use crate::{CodecError, CustomValue, CustomValueCodec, DynamicValue, StaticValue, TypeId};

/// A unique temp file path removed on drop.
struct TempFile(PathBuf);
impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn temp_file(tag: &str) -> TempFile {
    static C: AtomicU64 = AtomicU64::new(0);
    let n = C.fetch_add(1, Ordering::Relaxed);
    TempFile(std::env::temp_dir().join(format!(
        "scenarium-diskstore-{tag}-{}-{n}.bin",
        std::process::id()
    )))
}

fn target(path: &Path, digest: Digest) -> BlobTarget {
    BlobTarget {
        path: path.to_path_buf(),
        digest,
    }
}

fn complete_snapshot(values: Vec<DynamicValue>) -> OutputSnapshot {
    OutputSnapshot::new(values)
}

/// The full store↔read contract on one file: a stored blob round-trips under the
/// digest it was stamped with, every probe/read under any *other* digest is a miss
/// (never a stale hit), and a store under a new digest *overwrites* the node's one
/// blob — the old configuration's bytes are replaced, not orphaned.
#[tokio::test]
async fn store_then_read_round_trips_and_overwrites_under_a_new_digest() {
    let file = temp_file("roundtrip");
    let store = DiskStore::default();
    let d_a = Digest([7u8; 32]);
    let d_b = Digest([8u8; 32]);

    // Config A: three plain values, stamped D_A.
    let snapshot_a = OutputSnapshot::new(vec![
        DynamicValue::Unbound,
        DynamicValue::Static(StaticValue::Int(7)),
        DynamicValue::Static(StaticValue::String("x".into())),
    ]);
    store
        .store(
            &target(&file.0, d_a),
            &snapshot_a,
            &mut ContextManager::default(),
        )
        .await;

    // The stamped digest is probed off the header alone; any other digest — or a
    // missing file — is not a hit.
    assert_eq!(
        store.coverage(&target(&file.0, d_a)),
        Some(CachedOutputCoverage {
            ports: vec![false, true, true]
        })
    );
    assert!(
        store.coverage(&target(&file.0, d_b)).is_none(),
        "another digest means the blob is superseded, not present"
    );
    let absent = temp_file("absent");
    assert!(store.coverage(&target(&absent.0, d_a)).is_none());
    assert!(store.read(&target(&absent.0, d_a)).await.is_none());

    let back = store.read(&target(&file.0, d_a)).await.expect("hit");
    assert_eq!(back.values.len(), 3);
    assert!(matches!(back.values[0], DynamicValue::Unbound));
    assert_eq!(back.values[1].as_i64(), Some(7));
    assert_eq!(back.values[2].as_string(), Some("x"));
    assert!(
        store.read(&target(&file.0, d_b)).await.is_none(),
        "a blob carrying a different digest is a miss"
    );

    // Config B supersedes A: same node file, new digest — overwritten in place.
    let snapshot_b = complete_snapshot(vec![DynamicValue::Static(StaticValue::Int(35))]);
    store
        .store(
            &target(&file.0, d_b),
            &snapshot_b,
            &mut ContextManager::default(),
        )
        .await;
    assert_eq!(
        store.coverage(&target(&file.0, d_b)),
        Some(CachedOutputCoverage { ports: vec![true] }),
        "blob re-stamped D_B"
    );
    let back = store.read(&target(&file.0, d_b)).await.expect("hit");
    assert_eq!(back.values.len(), 1);
    assert_eq!(back.values[0].as_i64(), Some(35));
    assert!(
        store.read(&target(&file.0, d_a)).await.is_none(),
        "config A's bytes were overwritten, not kept beside B's"
    );
}

#[tokio::test]
async fn store_replaces_same_digest_blob_when_coverage_and_manifest_expand() {
    let file = temp_file("expanded-materialization");
    let decode_calls = Arc::new(AtomicU64::new(0));
    let store = versioned_store(1, decode_calls.clone());
    let digest = Digest([11; 32]);
    let target = target(&file.0, digest);
    let partial = OutputSnapshot::new(vec![
        DynamicValue::Static(StaticValue::Int(7)),
        DynamicValue::Unbound,
    ]);
    store
        .store(&target, &partial, &mut ContextManager::default())
        .await;
    let partial_blob = std::fs::read(&file.0).unwrap();
    assert!(
        header::parse(&partial_blob)
            .unwrap()
            .codecs
            .bytes
            .is_empty()
    );

    let complete = complete_snapshot(vec![
        DynamicValue::Static(StaticValue::Int(7)),
        DynamicValue::from_custom(Opaque),
    ]);
    store
        .store(&target, &complete, &mut ContextManager::default())
        .await;

    let cached = store.read(&target).await.expect("expanded blob");
    assert_eq!(
        store.coverage(&target),
        Some(CachedOutputCoverage {
            ports: vec![true, true]
        })
    );
    assert_eq!(cached.values[0].as_i64(), Some(7));
    assert!(cached.values[1].as_custom::<Opaque>().is_some());
    assert_eq!(decode_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        header::parse(&std::fs::read(&file.0).unwrap())
            .unwrap()
            .codecs
            .entry_count,
        1
    );
}

/// A custom value with no registered codec — never cacheable.
const OPAQUE_TYPE: &str = "78391861-24da-4368-a3a5-2a6b7a47f112";

#[derive(Debug)]
struct Opaque;
impl fmt::Display for Opaque {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Opaque")
    }
}
impl CustomValue for Opaque {
    fn type_id(&self) -> TypeId {
        OPAQUE_TYPE.into()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
        self
    }
}

#[derive(Debug)]
struct VersionedCodec {
    version: u32,
    decode_calls: Arc<AtomicU64>,
}

#[async_trait::async_trait]
impl CustomValueCodec for VersionedCodec {
    fn version(&self) -> u32 {
        self.version
    }

    async fn encode(
        &self,
        value: &dyn CustomValue,
        _ctx: &mut ContextManager,
    ) -> std::result::Result<Vec<u8>, CodecError> {
        value
            .as_any()
            .downcast_ref::<Opaque>()
            .expect("VersionedCodec is only registered for Opaque");
        Ok(Vec::new())
    }

    fn decode(&self, bytes: Vec<u8>) -> std::result::Result<Arc<dyn CustomValue>, CodecError> {
        assert!(bytes.is_empty());
        self.decode_calls.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(Opaque))
    }
}

fn versioned_library(version: u32, decode_calls: Arc<AtomicU64>) -> Library {
    let mut library = Library::default();
    library.register_type(
        OPAQUE_TYPE,
        TypeEntry::custom_with_codec(
            "Opaque",
            Arc::new(VersionedCodec {
                version,
                decode_calls,
            }),
        ),
    );
    library
}

fn versioned_store(version: u32, decode_calls: Arc<AtomicU64>) -> DiskStore {
    DiskStore::new(Arc::new(versioned_library(version, decode_calls)), None)
}

#[tokio::test]
async fn non_codecable_custom_is_skipped_not_written() {
    let file = temp_file("noncodec");
    let snapshot = complete_snapshot(vec![
        DynamicValue::Static(StaticValue::Int(1)),
        DynamicValue::from_custom(Opaque),
    ]);
    DiskStore::default()
        .store(
            &target(&file.0, Digest([1u8; 32])),
            &snapshot,
            &mut ContextManager::default(),
        )
        .await;
    assert!(!file.0.exists(), "no codec ⇒ silent skip, no blob created");
}

#[tokio::test]
async fn codec_manifest_is_selective_and_rejects_used_version_before_decode() {
    let file = temp_file("codec-version");
    let digest = Digest([12; 32]);
    let target = target(&file.0, digest);
    let snapshot = complete_snapshot(vec![DynamicValue::from_custom(Opaque)]);
    let old_decode_calls = Arc::new(AtomicU64::new(0));
    let old_store = versioned_store(1, old_decode_calls.clone());
    old_store
        .store(&target, &snapshot, &mut ContextManager::default())
        .await;
    assert_eq!(
        old_store.coverage(&target),
        Some(CachedOutputCoverage { ports: vec![true] })
    );

    let original = std::fs::read(&file.0).unwrap();
    let parsed = header::parse(&original).unwrap();
    let body = parsed.body.to_vec();
    let mut mismatched = header::encode(
        digest,
        &[DynamicValue::Static(StaticValue::Int(0))],
        &Library::default(),
    )
    .unwrap();
    mismatched.extend_from_slice(&body);
    std::fs::write(&file.0, mismatched).unwrap();
    assert!(old_store.read(&target).await.is_none());
    assert_eq!(
        old_decode_calls.load(Ordering::SeqCst),
        0,
        "a header/body manifest mismatch is rejected before decoding"
    );
    std::fs::write(&file.0, original).unwrap();

    let unrelated_type = TypeId::unique();
    let unchanged_decode_calls = Arc::new(AtomicU64::new(0));
    for unrelated_version in [1, 2] {
        let mut library = versioned_library(1, unchanged_decode_calls.clone());
        library.register_type(
            unrelated_type,
            TypeEntry::custom_with_codec(
                "Unused",
                Arc::new(VersionedCodec {
                    version: unrelated_version,
                    decode_calls: Arc::new(AtomicU64::new(0)),
                }),
            ),
        );
        let store = DiskStore::new(Arc::new(library), None);
        assert_eq!(
            store.coverage(&target),
            Some(CachedOutputCoverage { ports: vec![true] }),
            "adding or changing an unused codec preserves this blob"
        );
        assert!(store.read(&target).await.is_some());
    }
    assert_eq!(unchanged_decode_calls.load(Ordering::SeqCst), 2);

    let missing_store = DiskStore::default();
    assert!(missing_store.coverage(&target).is_none());
    assert!(missing_store.read(&target).await.is_none());

    let new_decode_calls = Arc::new(AtomicU64::new(0));
    let new_store = versioned_store(2, new_decode_calls.clone());
    assert!(
        new_store.coverage(&target).is_none(),
        "the same content digest under another codec version is a miss"
    );
    assert!(new_store.read(&target).await.is_none());
    assert_eq!(
        new_decode_calls.load(Ordering::SeqCst),
        0,
        "old bytes are rejected before invoking the new decoder"
    );

    new_store
        .store(&target, &snapshot, &mut ContextManager::default())
        .await;
    assert!(old_store.coverage(&target).is_none());
    assert_eq!(
        new_store.coverage(&target),
        Some(CachedOutputCoverage { ports: vec![true] })
    );
    let restored = new_store.read(&target).await.expect("new-version blob");
    assert!(restored.values[0].as_custom::<Opaque>().is_some());
    assert_eq!(new_decode_calls.load(Ordering::SeqCst), 1);
    assert_eq!(old_decode_calls.load(Ordering::SeqCst), 0);
}

/// Coverage metadata must agree exactly with the decoded output materialization.
#[tokio::test]
async fn read_rejects_coverage_body_disagreement() {
    let file = temp_file("bad-coverage");
    let store = DiskStore::default();
    let digest = Digest([2u8; 32]);
    let snapshot = OutputSnapshot::new(vec![
        DynamicValue::Static(StaticValue::Int(1)),
        DynamicValue::Unbound,
    ]);
    store
        .store(
            &target(&file.0, digest),
            &snapshot,
            &mut ContextManager::default(),
        )
        .await;
    let original = std::fs::read(&file.0).unwrap();
    let parsed = header::parse(&original).unwrap();
    let body = parsed.body.to_vec();
    let mut bytes = header::encode(
        digest,
        &[DynamicValue::Unbound, DynamicValue::Unbound],
        &Library::default(),
    )
    .unwrap();
    bytes.extend_from_slice(&body);
    std::fs::write(&file.0, &bytes).unwrap();
    assert!(
        store.read(&target(&file.0, digest)).await.is_none(),
        "coverage metadata cannot omit a decoded bound value"
    );

    let mut bytes = header::encode(
        digest,
        &[
            DynamicValue::Static(StaticValue::Int(0)),
            DynamicValue::Static(StaticValue::Int(0)),
        ],
        &Library::default(),
    )
    .unwrap();
    bytes.extend_from_slice(&body);
    std::fs::write(&file.0, &bytes).unwrap();
    assert!(
        store.read(&target(&file.0, digest)).await.is_none(),
        "coverage metadata cannot claim a decoded unbound value"
    );
}
