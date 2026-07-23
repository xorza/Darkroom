use std::any::Any;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use common::test_utils;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

use crate::execution::cache::OutputSnapshot;
use crate::execution::digest::Digest;
use crate::execution::disk_store::{BlobTarget, DiskStore, StorePolicy};
use crate::library::{Library, TypeEntry};
use crate::node::lambda::OutputDemand;
use crate::runtime::context::ContextManager;
use crate::{CodecError, CustomValue, CustomValueCodec, DynamicValue, StaticValue, TypeId};

#[derive(Debug)]
struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        if std::fs::remove_file(&self.0).is_err() {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

fn temp_file(tag: &str) -> TempFile {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
    TempFile(test_utils::test_output_path(&format!(
        "scenarium/disk-store/{tag}-{}-{sequence}.bin",
        std::process::id()
    )))
}

fn target(path: &Path, digest: Digest) -> BlobTarget {
    BlobTarget {
        path: path.to_path_buf(),
        digest,
    }
}

async fn read_snapshot(
    store: &DiskStore,
    target: &BlobTarget,
    output_count: usize,
) -> Option<OutputSnapshot> {
    let demand = vec![OutputDemand::Skip; output_count];
    store.read(target, &demand).await
}

fn publication_temp_files(path: &Path) -> Vec<PathBuf> {
    let prefix = format!("{}.", path.file_name().unwrap().to_string_lossy());
    std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|candidate| {
            let name = candidate.file_name().unwrap().to_string_lossy();
            name.starts_with(&prefix) && name.ends_with(".tmp")
        })
        .collect()
}

const BLOB_TYPE: &str = "78391861-24da-4368-a3a5-2a6b7a47f112";

#[derive(Debug, PartialEq, Eq)]
struct Blob(Vec<u8>);

impl fmt::Display for Blob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Blob({} bytes)", self.0.len())
    }
}

impl CustomValue for Blob {
    fn type_id(&self) -> TypeId {
        BLOB_TYPE.into()
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
    fail_encode: bool,
}

#[async_trait::async_trait]
impl CustomValueCodec for VersionedCodec {
    fn version(&self) -> u32 {
        self.version
    }

    async fn encode(
        &self,
        value: &dyn CustomValue,
        writer: &mut (dyn AsyncWrite + Unpin + Send),
        _ctx: &mut ContextManager,
    ) -> std::result::Result<(), CodecError> {
        let blob = value
            .as_any()
            .downcast_ref::<Blob>()
            .expect("VersionedCodec is only registered for Blob");
        writer.write_all(&blob.0).await?;
        if self.fail_encode {
            return Err("injected encode failure".into());
        }
        Ok(())
    }

    async fn decode(
        &self,
        reader: &mut (dyn AsyncRead + Unpin + Send),
        byte_len: u64,
    ) -> std::result::Result<Arc<dyn CustomValue>, CodecError> {
        let mut bytes = Vec::with_capacity(usize::try_from(byte_len)?);
        reader.read_to_end(&mut bytes).await?;
        self.decode_calls.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(Blob(bytes)))
    }
}

fn versioned_library(version: u32, decode_calls: Arc<AtomicU64>, fail_encode: bool) -> Library {
    let mut library = Library::default();
    library.register_type(
        BLOB_TYPE,
        TypeEntry::custom_with_codec(
            "Blob",
            Arc::new(VersionedCodec {
                version,
                decode_calls,
                fail_encode,
            }),
        ),
    );
    library
}

fn versioned_store(version: u32, decode_calls: Arc<AtomicU64>) -> DiskStore {
    DiskStore::new(
        Arc::new(versioned_library(version, decode_calls, false)),
        None,
    )
}

#[tokio::test]
async fn store_read_header_check_and_digest_replacement_round_trip() {
    let file = temp_file("roundtrip");
    let store = DiskStore::default();
    let first_digest = Digest([7; 32]);
    let second_digest = Digest([8; 32]);
    let first_target = target(&file.0, first_digest);
    let second_target = target(&file.0, second_digest);
    let first = OutputSnapshot::new(vec![
        DynamicValue::Unbound,
        DynamicValue::Static(StaticValue::Int(7)),
        DynamicValue::Static(StaticValue::String("x".into())),
    ]);

    store
        .store(
            &first_target,
            &first,
            StorePolicy::KnownMiss,
            &mut ContextManager::default(),
        )
        .await;
    assert_eq!(store.store_io.coverage_probes.load(Ordering::Relaxed), 0);
    assert_eq!(
        store.store_io.publication_attempts.load(Ordering::Relaxed),
        1
    );
    assert!(store.covers(&first_target, &first.values).await);
    assert!(!store.covers(&second_target, &first.values).await);
    let restored = read_snapshot(&store, &first_target, 3).await.unwrap();
    assert!(matches!(restored.values[0], DynamicValue::Unbound));
    assert_eq!(restored.values[1].as_i64(), Some(7));
    assert_eq!(restored.values[2].as_string(), Some("x"));

    let second = OutputSnapshot::new(vec![DynamicValue::Static(StaticValue::Int(35))]);
    store
        .store(
            &second_target,
            &second,
            StorePolicy::PreserveCovering,
            &mut ContextManager::default(),
        )
        .await;
    assert_eq!(store.store_io.coverage_probes.load(Ordering::Relaxed), 1);
    assert_eq!(
        store.store_io.publication_attempts.load(Ordering::Relaxed),
        2
    );
    assert!(read_snapshot(&store, &first_target, 3).await.is_none());
    assert_eq!(
        read_snapshot(&store, &second_target, 1)
            .await
            .unwrap()
            .values[0]
            .as_i64(),
        Some(35)
    );
}

#[tokio::test]
async fn broader_same_digest_blob_is_preserved() {
    let file = temp_file("coverage");
    let decode_calls = Arc::new(AtomicU64::new(0));
    let store = versioned_store(1, decode_calls.clone());
    let target = target(&file.0, Digest([11; 32]));
    let partial = OutputSnapshot::new(vec![
        DynamicValue::Static(StaticValue::Int(7)),
        DynamicValue::Unbound,
    ]);
    store
        .store(
            &target,
            &partial,
            StorePolicy::KnownMiss,
            &mut ContextManager::default(),
        )
        .await;
    assert_eq!(store.store_io.coverage_probes.load(Ordering::Relaxed), 0);
    assert_eq!(
        store.store_io.publication_attempts.load(Ordering::Relaxed),
        1
    );
    let second_output = [OutputDemand::Skip, OutputDemand::Produce];
    assert!(store.read(&target, &second_output).await.is_none());
    assert!(
        file.0.exists(),
        "an insufficient but valid blob is retained"
    );

    let complete = OutputSnapshot::new(vec![
        DynamicValue::Static(StaticValue::Int(7)),
        DynamicValue::from_custom(Blob(vec![1, 2, 3])),
    ]);
    store
        .store(
            &target,
            &complete,
            StorePolicy::KnownMiss,
            &mut ContextManager::default(),
        )
        .await;
    assert_eq!(store.store_io.coverage_probes.load(Ordering::Relaxed), 0);
    assert_eq!(
        store.store_io.publication_attempts.load(Ordering::Relaxed),
        2
    );
    let complete_bytes = std::fs::read(&file.0).unwrap();

    store
        .store(
            &target,
            &partial,
            StorePolicy::PreserveCovering,
            &mut ContextManager::default(),
        )
        .await;
    assert_eq!(store.store_io.coverage_probes.load(Ordering::Relaxed), 1);
    assert_eq!(
        store.store_io.publication_attempts.load(Ordering::Relaxed),
        2
    );
    assert_eq!(std::fs::read(&file.0).unwrap(), complete_bytes);
    assert!(store.covers(&target, &complete.values).await);
    assert!(store.covers(&target, &partial.values).await);
    let restored = read_snapshot(&store, &target, 2).await.unwrap();
    assert_eq!(restored.values[0].as_i64(), Some(7));
    assert_eq!(
        restored.values[1].as_custom::<Blob>(),
        Some(&Blob(vec![1, 2, 3]))
    );
    assert_eq!(decode_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn missing_and_changed_codecs_miss_before_decode() {
    let file = temp_file("codec-version");
    let target = target(&file.0, Digest([12; 32]));
    let snapshot = OutputSnapshot::new(vec![DynamicValue::from_custom(Blob(vec![9]))]);
    let old_calls = Arc::new(AtomicU64::new(0));
    let old_store = versioned_store(1, old_calls.clone());
    old_store
        .store(
            &target,
            &snapshot,
            StorePolicy::KnownMiss,
            &mut ContextManager::default(),
        )
        .await;

    assert!(!DiskStore::default().covers(&target, &snapshot.values).await);
    assert!(
        read_snapshot(&DiskStore::default(), &target, 1)
            .await
            .is_none()
    );

    let new_calls = Arc::new(AtomicU64::new(0));
    let new_store = versioned_store(2, new_calls.clone());
    assert!(!new_store.covers(&target, &snapshot.values).await);
    assert!(read_snapshot(&new_store, &target, 1).await.is_none());
    assert_eq!(new_calls.load(Ordering::SeqCst), 0);

    new_store
        .store(
            &target,
            &snapshot,
            StorePolicy::KnownMiss,
            &mut ContextManager::default(),
        )
        .await;
    assert!(!old_store.covers(&target, &snapshot.values).await);
    assert!(read_snapshot(&new_store, &target, 1).await.is_some());
    assert_eq!(new_calls.load(Ordering::SeqCst), 1);
    assert_eq!(old_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn unregistered_custom_value_is_not_written() {
    let file = temp_file("unregistered");
    let snapshot = OutputSnapshot::new(vec![DynamicValue::from_custom(Blob(vec![1]))]);
    DiskStore::default()
        .store(
            &target(&file.0, Digest([1; 32])),
            &snapshot,
            StorePolicy::KnownMiss,
            &mut ContextManager::default(),
        )
        .await;
    assert!(!file.0.exists());
}

#[tokio::test]
async fn failed_streaming_encode_preserves_previous_blob() {
    let file = temp_file("encode-failure");
    let calls = Arc::new(AtomicU64::new(0));
    let good_store = versioned_store(1, calls.clone());
    let original_target = target(&file.0, Digest([4; 32]));
    good_store
        .store(
            &original_target,
            &OutputSnapshot::new(vec![DynamicValue::from_custom(Blob(vec![1, 2]))]),
            StorePolicy::KnownMiss,
            &mut ContextManager::default(),
        )
        .await;
    let original = std::fs::read(&file.0).unwrap();

    let failing_store = DiskStore::new(
        Arc::new(versioned_library(1, Arc::new(AtomicU64::new(0)), true)),
        None,
    );
    failing_store
        .store(
            &target(&file.0, Digest([5; 32])),
            &OutputSnapshot::new(vec![DynamicValue::from_custom(Blob(vec![8; 1024]))]),
            StorePolicy::KnownMiss,
            &mut ContextManager::default(),
        )
        .await;
    assert_eq!(std::fs::read(&file.0).unwrap(), original);
    assert!(publication_temp_files(&file.0).is_empty());
    assert!(
        read_snapshot(&good_store, &original_target, 1)
            .await
            .is_some()
    );
}

#[tokio::test]
async fn failed_publication_does_not_repeat_coverage_probe() {
    let file = temp_file("publication-failure");
    std::fs::create_dir_all(&file.0).unwrap();
    let survivor = file.0.join("survivor");
    std::fs::write(&survivor, b"old").unwrap();
    let store = DiskStore::default();
    store
        .store(
            &target(&file.0, Digest([9; 32])),
            &OutputSnapshot::new(vec![DynamicValue::Static(StaticValue::Int(9))]),
            StorePolicy::PreserveCovering,
            &mut ContextManager::default(),
        )
        .await;

    assert_eq!(store.store_io.coverage_probes.load(Ordering::Relaxed), 1);
    assert_eq!(
        store.store_io.publication_attempts.load(Ordering::Relaxed),
        1
    );
    assert_eq!(std::fs::read(survivor).unwrap(), b"old");
    assert!(publication_temp_files(&file.0).is_empty());
}

#[tokio::test]
async fn truncated_blob_is_rejected_by_header_check_and_read() {
    let file = temp_file("truncated");
    let store = DiskStore::default();
    let target = target(&file.0, Digest([6; 32]));
    store
        .store(
            &target,
            &OutputSnapshot::new(vec![DynamicValue::Static(StaticValue::String(
                "payload".into(),
            ))]),
            StorePolicy::KnownMiss,
            &mut ContextManager::default(),
        )
        .await;
    let mut bytes = std::fs::read(&file.0).unwrap();
    bytes.pop();
    std::fs::write(&file.0, bytes).unwrap();
    let expected = [DynamicValue::Static(StaticValue::String("payload".into()))];
    assert!(!store.covers(&target, &expected).await);
    assert!(read_snapshot(&store, &target, 1).await.is_none());
    assert!(!file.0.exists(), "a corrupt cache blob is removed");
}
