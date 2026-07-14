use super::*;
use crate::{CustomValue, DynamicValue, StaticValue, TypeId};
use std::any::Any;
use std::fmt;

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
    let coverage = CachedOutputCoverage::from_values(&values);
    OutputSnapshot::new(values, coverage)
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
    let snapshot_a = OutputSnapshot::new(
        vec![
            DynamicValue::Unbound,
            DynamicValue::Static(StaticValue::Int(7)),
            DynamicValue::Static(StaticValue::String("x".into())),
        ],
        CachedOutputCoverage::from_bytes(&[0, 1, 1]).unwrap(),
    );
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
        target(&file.0, d_a).coverage(),
        Some(CachedOutputCoverage::from_bytes(&[0, 1, 1]).unwrap())
    );
    assert!(
        target(&file.0, d_b).coverage().is_none(),
        "another digest means the blob is superseded, not present"
    );
    let absent = temp_file("absent");
    assert!(target(&absent.0, d_a).coverage().is_none());
    assert!(store.read(&target(&absent.0, d_a)).await.is_none());

    let back = store.read(&target(&file.0, d_a)).await.expect("hit");
    assert_eq!(back.values.len(), 3);
    assert_eq!(
        back.coverage,
        CachedOutputCoverage::from_bytes(&[0, 1, 1]).unwrap()
    );
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
        target(&file.0, d_b).coverage(),
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
async fn store_replaces_same_digest_blob_when_output_coverage_expands() {
    let file = temp_file("expanded-materialization");
    let store = DiskStore::default();
    let digest = Digest([11; 32]);
    let target = target(&file.0, digest);
    let partial = OutputSnapshot::new(
        vec![
            DynamicValue::Static(StaticValue::Int(7)),
            DynamicValue::Unbound,
        ],
        CachedOutputCoverage::from_bytes(&[1, 0]).unwrap(),
    );
    store
        .store(&target, &partial, &mut ContextManager::default())
        .await;

    let complete = complete_snapshot(vec![
        DynamicValue::Static(StaticValue::Int(7)),
        DynamicValue::Static(StaticValue::Int(9)),
    ]);
    store
        .store(&target, &complete, &mut ContextManager::default())
        .await;

    let cached = store.read(&target).await.expect("expanded blob");
    assert_eq!(
        cached.coverage,
        CachedOutputCoverage {
            ports: vec![true, true],
        }
    );
    assert_eq!(cached.values[0].as_i64(), Some(7));
    assert_eq!(cached.values[1].as_i64(), Some(9));
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
    fn type_id(&self) -> TypeId {
        "78391861-24da-4368-a3a5-2a6b7a47f112".into()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
        self
    }
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

/// A blob whose format-version header doesn't match the current one decodes to a
/// miss (recompute), not a silent mis-decode — guards against a `CachedValue` shape
/// change serving garbage through the non-self-describing bitcode frame.
#[tokio::test]
async fn read_rejects_an_unknown_format_version() {
    let file = temp_file("badversion");
    let store = DiskStore::default();
    let digest = Digest([2u8; 32]);
    let snapshot = complete_snapshot(vec![DynamicValue::Static(StaticValue::Int(1))]);
    store
        .store(
            &target(&file.0, digest),
            &snapshot,
            &mut ContextManager::default(),
        )
        .await;
    // Corrupt the 4-byte little-endian version header, which sits right after the
    // 32-byte digest — the digest itself still matches, so the miss below is the
    // codec frame's doing, not the digest check's.
    let mut bytes = std::fs::read(&file.0).unwrap();
    bytes[32] ^= 0xff;
    std::fs::write(&file.0, &bytes).unwrap();

    assert_eq!(
        &std::fs::read(&file.0).unwrap()[..32],
        digest.0.as_slice(),
        "digest intact"
    );
    assert!(
        store.read(&target(&file.0, digest)).await.is_none(),
        "a blob with an unknown format version is treated as a miss"
    );
}
