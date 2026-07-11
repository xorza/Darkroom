use super::*;
use crate::data::{CustomValue, StaticValue, TypeId};
use std::any::Any;
use std::fmt;
use std::sync::atomic::AtomicU64;

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
        "scenarium-blob-{tag}-{}-{n}.bin",
        std::process::id()
    )))
}

fn library() -> Arc<Library> {
    Arc::new(Library::default())
}

#[tokio::test]
async fn write_then_read_round_trips_outputs_under_the_stamped_digest() {
    let file = temp_file("roundtrip");
    let digest = Digest([7u8; 32]);
    let outputs = vec![
        DynamicValue::Unbound,
        DynamicValue::Static(StaticValue::Int(7)),
        DynamicValue::Static(StaticValue::String("x".into())),
    ];
    let wrote = write(
        &file.0,
        digest,
        &outputs,
        &library(),
        &mut ContextManager::default(),
    )
    .await
    .unwrap();
    assert!(wrote, "plain values are written");

    // The stamped digest is readable off the file header without decoding.
    assert_eq!(stored_digest(&file.0), Some(digest));

    let back = read(&file.0, digest, &library()).await.expect("hit");
    assert_eq!(back.len(), 3);
    assert!(matches!(back[0], DynamicValue::Unbound));
    assert_eq!(back[1].as_i64(), Some(7));
    assert_eq!(back[2].as_string(), Some("x"));

    // Reading under any *other* digest is a miss — the header check is what lets a
    // node-keyed file be overwritten by a newer configuration without ever serving
    // the old one's bytes as the new one's.
    assert!(
        read(&file.0, Digest([8u8; 32]), &library()).await.is_none(),
        "a blob carrying a different digest is a miss"
    );
}

#[tokio::test]
async fn read_missing_is_none() {
    let file = temp_file("missing");
    assert!(read(&file.0, Digest([1u8; 32]), &library()).await.is_none());
    assert_eq!(stored_digest(&file.0), None, "no file, no stored digest");
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
    let wrote = write(
        &file.0,
        Digest([1u8; 32]),
        &outputs,
        &library(),
        &mut ContextManager::default(),
    )
    .await
    .unwrap();
    assert!(!wrote, "no codec ⇒ Ok(false), nothing written");
    assert!(!file.0.exists(), "no blob created");
}

/// A blob whose format-version header doesn't match the current one decodes to a
/// miss (recompute), not a silent mis-decode — guards against a `CachedValue` shape
/// change serving garbage through the non-self-describing bitcode frame.
#[tokio::test]
async fn read_rejects_an_unknown_format_version() {
    let file = temp_file("badversion");
    let digest = Digest([2u8; 32]);
    let outputs = vec![DynamicValue::Static(StaticValue::Int(1))];
    write(
        &file.0,
        digest,
        &outputs,
        &library(),
        &mut ContextManager::default(),
    )
    .await
    .unwrap();
    // Corrupt the 4-byte little-endian version header, which sits right after the
    // 32-byte digest — the digest itself still matches, so the miss below is the
    // codec frame's doing, not the digest check's.
    let mut bytes = std::fs::read(&file.0).unwrap();
    bytes[32] ^= 0xff;
    std::fs::write(&file.0, &bytes).unwrap();

    assert_eq!(stored_digest(&file.0), Some(digest), "digest intact");
    assert!(
        read(&file.0, digest, &library()).await.is_none(),
        "a blob with an unknown format version is treated as a miss"
    );
}
