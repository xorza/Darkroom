use std::any::Any;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use ::common::TempFile;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

use crate::execution::cache::digest::Digest;
use crate::execution::cache::disk_store::error::{StoreError, StoreOutcome};
use crate::execution::cache::disk_store::{BlobTarget, DiskStore, StorePolicy};
use crate::execution::cache::slot::OutputSnapshot;
use crate::graph::func::lambda::OutputDemand;
use crate::library::{Library, TypeEntry};
use crate::runtime::context::ContextStore;
use crate::{CodecError, ConstValue, CustomValue, CustomValueCodec, DynamicValue, TypeId};

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
    store
        .read(target, &demand, &mut ContextStore::default())
        .await
}

/// Publish, asserting the store answered `expected`. Every call has a definite
/// answer now; a test that dropped one would be back to inferring the write
/// from the filesystem alone.
async fn store_expecting(
    store: &DiskStore,
    target: &BlobTarget,
    snapshot: &OutputSnapshot,
    policy: StorePolicy,
    expected: StoreOutcome,
) {
    let outcome = store
        .store(target, snapshot, policy, &mut ContextStore::default())
        .await;
    assert_eq!(
        outcome.as_ref().ok(),
        Some(&expected),
        "expected {expected:?}, got {outcome:?}"
    );
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
        _ctx: &mut ContextStore,
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
        _ctx: &mut ContextStore,
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
    DiskStore::new(&versioned_library(version, decode_calls, false), None)
}

#[tokio::test]
async fn store_read_header_check_and_digest_replacement_round_trip() {
    let file = TempFile::new("roundtrip");
    let store = DiskStore::default();
    let first_digest = Digest([7; 32]);
    let second_digest = Digest([8; 32]);
    let first_target = target(file.path(), first_digest);
    let second_target = target(file.path(), second_digest);
    let first = OutputSnapshot::new(vec![
        DynamicValue::Unbound,
        DynamicValue::Static(ConstValue::Int(7)),
        DynamicValue::Static(ConstValue::String("x".into())),
    ]);

    store_expecting(
        &store,
        &first_target,
        &first,
        StorePolicy::KnownMiss,
        StoreOutcome::Published,
    )
    .await;
    assert!(store.covers(&first_target, first.values()).await);
    assert!(!store.covers(&second_target, first.values()).await);
    let restored = read_snapshot(&store, &first_target, 3).await.unwrap();
    assert!(matches!(restored.values()[0], DynamicValue::Unbound));
    assert_eq!(restored.values()[1].as_i64(), Some(7));
    assert_eq!(restored.values()[2].as_string(), Some("x"));

    let second = OutputSnapshot::new(vec![DynamicValue::Static(ConstValue::Int(35))]);
    // A blob under the previous digest cannot cover this one, so the probe
    // fails and the publication goes ahead.
    store_expecting(
        &store,
        &second_target,
        &second,
        StorePolicy::PreserveCovering,
        StoreOutcome::Published,
    )
    .await;
    assert!(read_snapshot(&store, &first_target, 3).await.is_none());
    assert_eq!(
        read_snapshot(&store, &second_target, 1)
            .await
            .unwrap()
            .values()[0]
            .as_i64(),
        Some(35)
    );
}

#[tokio::test]
async fn broader_same_digest_blob_is_preserved() {
    let file = TempFile::new("coverage");
    let decode_calls = Arc::new(AtomicU64::new(0));
    let store = versioned_store(1, decode_calls.clone());
    let target = target(file.path(), Digest([11; 32]));
    let partial = OutputSnapshot::new(vec![
        DynamicValue::Static(ConstValue::Int(7)),
        DynamicValue::Unbound,
    ]);
    store_expecting(
        &store,
        &target,
        &partial,
        StorePolicy::KnownMiss,
        StoreOutcome::Published,
    )
    .await;
    let second_output = [OutputDemand::Skip, OutputDemand::Produce];
    assert!(
        store
            .read(&target, &second_output, &mut ContextStore::default())
            .await
            .is_none()
    );
    assert!(file.exists(), "an insufficient but valid blob is retained");

    let complete = OutputSnapshot::new(vec![
        DynamicValue::Static(ConstValue::Int(7)),
        DynamicValue::from_custom(Blob(vec![1, 2, 3])),
    ]);
    store_expecting(
        &store,
        &target,
        &complete,
        StorePolicy::KnownMiss,
        StoreOutcome::Published,
    )
    .await;
    let complete_bytes = std::fs::read(file.path()).unwrap();

    store_expecting(
        &store,
        &target,
        &partial,
        StorePolicy::PreserveCovering,
        StoreOutcome::AlreadyCovered,
    )
    .await;
    assert_eq!(std::fs::read(file.path()).unwrap(), complete_bytes);
    assert!(store.covers(&target, complete.values()).await);
    assert!(store.covers(&target, partial.values()).await);
    let restored = read_snapshot(&store, &target, 2).await.unwrap();
    assert_eq!(restored.values()[0].as_i64(), Some(7));
    assert_eq!(
        restored.values()[1].as_custom::<Blob>(),
        Some(&Blob(vec![1, 2, 3]))
    );
    assert_eq!(decode_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn missing_and_changed_codecs_miss_before_decode() {
    let file = TempFile::new("codec-version");
    let target = target(file.path(), Digest([12; 32]));
    let snapshot = OutputSnapshot::new(vec![DynamicValue::from_custom(Blob(vec![9]))]);
    let old_calls = Arc::new(AtomicU64::new(0));
    let old_store = versioned_store(1, old_calls.clone());
    store_expecting(
        &old_store,
        &target,
        &snapshot,
        StorePolicy::KnownMiss,
        StoreOutcome::Published,
    )
    .await;

    assert!(
        !DiskStore::default()
            .covers(&target, snapshot.values())
            .await
    );
    assert!(
        read_snapshot(&DiskStore::default(), &target, 1)
            .await
            .is_none()
    );

    let new_calls = Arc::new(AtomicU64::new(0));
    let new_store = versioned_store(2, new_calls.clone());
    assert!(!new_store.covers(&target, snapshot.values()).await);
    assert!(read_snapshot(&new_store, &target, 1).await.is_none());
    assert_eq!(new_calls.load(Ordering::SeqCst), 0);

    store_expecting(
        &new_store,
        &target,
        &snapshot,
        StorePolicy::KnownMiss,
        StoreOutcome::Published,
    )
    .await;
    assert!(!old_store.covers(&target, snapshot.values()).await);
    assert!(read_snapshot(&new_store, &target, 1).await.is_some());
    assert_eq!(new_calls.load(Ordering::SeqCst), 1);
    assert_eq!(old_calls.load(Ordering::SeqCst), 0);
}

/// A type with no codec is reported as unwritten rather than as a failure: the
/// write was fine, the library simply cannot represent this value on disk, and
/// no retry changes that. A caller reporting to a human needs the distinction —
/// it is the difference between "try again" and "this node will never persist".
#[tokio::test]
async fn unregistered_custom_value_is_reported_unsupported_not_failed() {
    let file = TempFile::new("unregistered");
    let snapshot = OutputSnapshot::new(vec![DynamicValue::from_custom(Blob(vec![1]))]);
    store_expecting(
        &DiskStore::default(),
        &target(file.path(), Digest([1; 32])),
        &snapshot,
        StorePolicy::KnownMiss,
        StoreOutcome::Unsupported {
            type_id: BLOB_TYPE.into(),
        },
    )
    .await;
    assert!(!file.exists());
}

#[tokio::test]
async fn failed_streaming_encode_preserves_previous_blob() {
    let file = TempFile::new("encode-failure");
    let calls = Arc::new(AtomicU64::new(0));
    let good_store = versioned_store(1, calls.clone());
    let original_target = target(file.path(), Digest([4; 32]));
    store_expecting(
        &good_store,
        &original_target,
        &OutputSnapshot::new(vec![DynamicValue::from_custom(Blob(vec![1, 2]))]),
        StorePolicy::KnownMiss,
        StoreOutcome::Published,
    )
    .await;
    let original = std::fs::read(file.path()).unwrap();

    let failing_store = DiskStore::new(
        &versioned_library(1, Arc::new(AtomicU64::new(0)), true),
        None,
    );
    let failed = failing_store
        .store(
            &target(file.path(), Digest([5; 32])),
            &OutputSnapshot::new(vec![DynamicValue::from_custom(Blob(vec![8; 1024]))]),
            StorePolicy::KnownMiss,
            &mut ContextStore::default(),
        )
        .await;
    // A codec that rejects the value it was handed is a failure, unlike a type
    // with no codec at all — and it names the blob it could not write.
    let Err(StoreError::Encode { path, source }) = &failed else {
        panic!("a failing codec must report an encode failure, got {failed:?}");
    };
    assert_eq!(path, file.path());
    assert!(
        source.to_string().contains("injected encode failure"),
        "the codec's own message survives: {source}"
    );
    assert_eq!(std::fs::read(file.path()).unwrap(), original);
    assert!(publication_temp_files(file.path()).is_empty());
    assert!(
        read_snapshot(&good_store, &original_target, 1)
            .await
            .is_some()
    );
}

/// A publication that cannot land leaves the directory it was writing into
/// exactly as it found it — neighbours intact, no temporary left behind.
#[tokio::test]
async fn a_failed_publication_disturbs_nothing_around_it() {
    let file = TempFile::new("publication-failure");
    std::fs::create_dir_all(file.path()).unwrap();
    let survivor = file.path().join("survivor");
    std::fs::write(&survivor, b"old").unwrap();
    let store = DiskStore::default();
    let failed = store
        .store(
            &target(file.path(), Digest([9; 32])),
            &OutputSnapshot::new(vec![DynamicValue::Static(ConstValue::Int(9))]),
            StorePolicy::PreserveCovering,
            &mut ContextStore::default(),
        )
        .await;

    // The blob path is a directory here. The body still streams fine — an
    // `AtomicFile` writes to a temporary beside the destination — so the
    // failure lands on the publication, not on the encode.
    let Err(StoreError::Publish { path, .. }) = &failed else {
        panic!("a blocked destination must fail at publication, got {failed:?}");
    };
    assert_eq!(path, file.path());
    assert_eq!(std::fs::read(survivor).unwrap(), b"old");
    assert!(publication_temp_files(file.path()).is_empty());
}

#[tokio::test]
async fn truncated_blob_is_rejected_by_header_check_and_read() {
    let file = TempFile::new("truncated");
    let store = DiskStore::default();
    let target = target(file.path(), Digest([6; 32]));
    store_expecting(
        &store,
        &target,
        &OutputSnapshot::new(vec![DynamicValue::Static(ConstValue::String(
            "payload".into(),
        ))]),
        StorePolicy::KnownMiss,
        StoreOutcome::Published,
    )
    .await;
    let mut bytes = std::fs::read(file.path()).unwrap();
    bytes.pop();
    std::fs::write(file.path(), bytes).unwrap();
    let expected = [DynamicValue::Static(ConstValue::String("payload".into()))];
    assert!(!store.covers(&target, &expected).await);
    assert!(read_snapshot(&store, &target, 1).await.is_none());
    assert!(!file.exists(), "a corrupt cache blob is removed");
}
