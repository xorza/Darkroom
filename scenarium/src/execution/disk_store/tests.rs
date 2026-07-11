use super::*;
use crate::data::{CustomValue, StaticValue, TypeId};
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
    let outputs_a = vec![
        DynamicValue::Unbound,
        DynamicValue::Static(StaticValue::Int(7)),
        DynamicValue::Static(StaticValue::String("x".into())),
    ];
    store
        .store(
            &target(&file.0, d_a),
            &outputs_a,
            &mut ContextManager::default(),
        )
        .await;

    // The stamped digest is probed off the header alone; any other digest — or a
    // missing file — is not a hit.
    assert_eq!(stored_digest(&file.0), Some(d_a));
    assert!(store.has_current_blob(&target(&file.0, d_a)));
    assert!(
        !store.has_current_blob(&target(&file.0, d_b)),
        "another digest means the blob is superseded, not present"
    );
    let absent = temp_file("absent");
    assert_eq!(stored_digest(&absent.0), None, "no file, no stored digest");
    assert!(!store.has_current_blob(&target(&absent.0, d_a)));
    assert!(store.read(&target(&absent.0, d_a)).await.is_none());

    let back = store.read(&target(&file.0, d_a)).await.expect("hit");
    assert_eq!(back.len(), 3);
    assert!(matches!(back[0], DynamicValue::Unbound));
    assert_eq!(back[1].as_i64(), Some(7));
    assert_eq!(back[2].as_string(), Some("x"));
    assert!(
        store.read(&target(&file.0, d_b)).await.is_none(),
        "a blob carrying a different digest is a miss"
    );

    // Config B supersedes A: same node file, new digest — overwritten in place.
    let outputs_b = vec![DynamicValue::Static(StaticValue::Int(35))];
    store
        .store(
            &target(&file.0, d_b),
            &outputs_b,
            &mut ContextManager::default(),
        )
        .await;
    assert_eq!(stored_digest(&file.0), Some(d_b), "blob re-stamped D_B");
    let back = store.read(&target(&file.0, d_b)).await.expect("hit");
    assert_eq!(back.len(), 1);
    assert_eq!(back[0].as_i64(), Some(35));
    assert!(
        store.read(&target(&file.0, d_a)).await.is_none(),
        "config A's bytes were overwritten, not kept beside B's"
    );
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
    let outputs = vec![
        DynamicValue::Static(StaticValue::Int(1)),
        DynamicValue::from_custom(Opaque),
    ];
    DiskStore::default()
        .store(
            &target(&file.0, Digest([1u8; 32])),
            &outputs,
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
    let outputs = vec![DynamicValue::Static(StaticValue::Int(1))];
    store
        .store(
            &target(&file.0, digest),
            &outputs,
            &mut ContextManager::default(),
        )
        .await;
    // Corrupt the 4-byte little-endian version header, which sits right after the
    // 32-byte digest — the digest itself still matches, so the miss below is the
    // codec frame's doing, not the digest check's.
    let mut bytes = std::fs::read(&file.0).unwrap();
    bytes[32] ^= 0xff;
    std::fs::write(&file.0, &bytes).unwrap();

    assert_eq!(stored_digest(&file.0), Some(digest), "digest intact");
    assert!(
        store.read(&target(&file.0, digest)).await.is_none(),
        "a blob with an unknown format version is treated as a miss"
    );
}
