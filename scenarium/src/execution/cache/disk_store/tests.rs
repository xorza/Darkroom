use std::fs;
use std::path::Path;

use common::TempFile;
use common::file_utils::internals::publication_temp_files;

use crate::data::codec::Codecs;
use crate::execution::cache::digest::Digest;
use crate::execution::cache::disk_store::error::StoreError;
use crate::execution::cache::disk_store::store_outcome::StoreOutcome;
use crate::execution::cache::disk_store::{BlobTarget, DiskStore, StorePolicy};
use crate::execution::cache::slot::OutputSnapshot;
use crate::graph::func::lambda::OutputDemand;
use crate::internals::blob::{BLOB_TYPE, Blob, BlobCodec};
use crate::internals::calls::Calls;
use crate::{ConstValue, DynamicValue};

fn target(path: &Path, digest: Digest) -> BlobTarget {
    BlobTarget {
        path: path.to_path_buf(),
        digest,
    }
}

/// The store under test. Every test names its blob path directly, so the store
/// needs no root; what varies is the codecs it is handed.
const STORE: DiskStore = DiskStore::new(None);

async fn read_snapshot(
    codecs: &Codecs,
    target: &BlobTarget,
    output_count: usize,
) -> Option<OutputSnapshot> {
    let demand = vec![OutputDemand::Skip; output_count];
    STORE.read(target, codecs, &demand).await
}

/// Publish, asserting the store answered `expected`. Every call has a definite
/// answer now; a test that dropped one would be back to inferring the write
/// from the filesystem alone.
async fn store_expecting(
    codecs: &Codecs,
    target: &BlobTarget,
    snapshot: &OutputSnapshot,
    policy: StorePolicy,
    expected: StoreOutcome,
) {
    let outcome = STORE.store(target, codecs, snapshot, policy).await;
    assert_eq!(
        outcome.as_ref().ok(),
        Some(&expected),
        "expected {expected:?}, got {outcome:?}"
    );
}

fn versioned_codecs(version: u32, decodes: Calls, fail_encode: bool) -> Codecs {
    let codec = BlobCodec {
        version,
        decodes,
        fail_encode,
        ..BlobCodec::default()
    };
    Codecs::clone(codec.library().codecs())
}

#[tokio::test]
async fn store_read_header_check_and_digest_replacement_round_trip() {
    let file = TempFile::new("roundtrip");
    let codecs = Codecs::default();
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
        &codecs,
        &first_target,
        &first,
        StorePolicy::KnownMiss,
        StoreOutcome::Published,
    )
    .await;
    assert!(STORE.covers(&first_target, first.values(), &codecs).await);
    assert!(!STORE.covers(&second_target, first.values(), &codecs).await);
    let restored = read_snapshot(&codecs, &first_target, 3).await.unwrap();
    assert!(matches!(restored.values()[0], DynamicValue::Unbound));
    assert_eq!(restored.values()[1].as_i64(), Some(7));
    assert_eq!(restored.values()[2].as_string(), Some("x"));

    let second = OutputSnapshot::new(vec![DynamicValue::Static(ConstValue::Int(35))]);
    // A blob under the previous digest cannot cover this one, so the probe
    // fails and the publication goes ahead.
    store_expecting(
        &codecs,
        &second_target,
        &second,
        StorePolicy::PreserveCovering,
        StoreOutcome::Published,
    )
    .await;
    assert!(read_snapshot(&codecs, &first_target, 3).await.is_none());
    assert_eq!(
        read_snapshot(&codecs, &second_target, 1)
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
    let decode_calls = Calls::default();
    let codecs = versioned_codecs(1, decode_calls.clone(), false);
    let target = target(file.path(), Digest([11; 32]));
    let partial = OutputSnapshot::new(vec![
        DynamicValue::Static(ConstValue::Int(7)),
        DynamicValue::Unbound,
    ]);
    store_expecting(
        &codecs,
        &target,
        &partial,
        StorePolicy::KnownMiss,
        StoreOutcome::Published,
    )
    .await;
    let second_output = [OutputDemand::Skip, OutputDemand::Produce];
    assert!(STORE.read(&target, &codecs, &second_output).await.is_none());
    assert!(file.exists(), "an insufficient but valid blob is retained");

    let complete = OutputSnapshot::new(vec![
        DynamicValue::Static(ConstValue::Int(7)),
        DynamicValue::from_custom(Blob(vec![1, 2, 3])),
    ]);
    store_expecting(
        &codecs,
        &target,
        &complete,
        StorePolicy::KnownMiss,
        StoreOutcome::Published,
    )
    .await;
    let complete_bytes = fs::read(file.path()).unwrap();

    store_expecting(
        &codecs,
        &target,
        &partial,
        StorePolicy::PreserveCovering,
        StoreOutcome::AlreadyCovered,
    )
    .await;
    assert_eq!(fs::read(file.path()).unwrap(), complete_bytes);
    assert!(STORE.covers(&target, complete.values(), &codecs).await);
    assert!(STORE.covers(&target, partial.values(), &codecs).await);
    let restored = read_snapshot(&codecs, &target, 2).await.unwrap();
    assert_eq!(restored.values()[0].as_i64(), Some(7));
    assert_eq!(
        restored.values()[1].as_custom::<Blob>(),
        Some(&Blob(vec![1, 2, 3]))
    );
    assert_eq!(decode_calls.count(), 1);
}

#[tokio::test]
async fn missing_and_changed_codecs_miss_before_decode() {
    let file = TempFile::new("codec-version");
    let target = target(file.path(), Digest([12; 32]));
    let snapshot = OutputSnapshot::new(vec![DynamicValue::from_custom(Blob(vec![9]))]);
    let old_calls = Calls::default();
    let old_codecs = versioned_codecs(1, old_calls.clone(), false);
    store_expecting(
        &old_codecs,
        &target,
        &snapshot,
        StorePolicy::KnownMiss,
        StoreOutcome::Published,
    )
    .await;

    assert!(
        !STORE
            .covers(&target, snapshot.values(), &Codecs::default())
            .await
    );
    assert!(
        read_snapshot(&Codecs::default(), &target, 1)
            .await
            .is_none()
    );

    let new_calls = Calls::default();
    let new_codecs = versioned_codecs(2, new_calls.clone(), false);
    assert!(!STORE.covers(&target, snapshot.values(), &new_codecs).await);
    assert!(read_snapshot(&new_codecs, &target, 1).await.is_none());
    assert_eq!(new_calls.count(), 0);

    store_expecting(
        &new_codecs,
        &target,
        &snapshot,
        StorePolicy::KnownMiss,
        StoreOutcome::Published,
    )
    .await;
    assert!(!STORE.covers(&target, snapshot.values(), &old_codecs).await);
    assert!(read_snapshot(&new_codecs, &target, 1).await.is_some());
    assert_eq!(new_calls.count(), 1);
    assert_eq!(old_calls.count(), 0);
}

/// A type with no codec is reported as unwritten rather than as a failure: the
/// write was fine, the library simply cannot represent this value on disk, and
/// no retry changes that. A caller reporting to a human needs the distinction —
/// it is the difference between "try again" and "this node will never persist".
///
/// The verdict comes before any I/O under either policy: the blob's directory
/// is not created, and no covering blob is looked for.
#[tokio::test]
async fn unregistered_custom_value_is_reported_unsupported_not_failed() {
    let file = TempFile::new("unregistered");
    let blob = file.path().join("never-created").join("blob");
    let snapshot = OutputSnapshot::new(vec![
        DynamicValue::Static(ConstValue::Int(1)),
        DynamicValue::from_custom(Blob(vec![1])),
    ]);
    for policy in [StorePolicy::KnownMiss, StorePolicy::PreserveCovering] {
        store_expecting(
            &Codecs::default(),
            &target(&blob, Digest([1; 32])),
            &snapshot,
            policy,
            StoreOutcome::Unsupported { type_id: BLOB_TYPE },
        )
        .await;
        assert!(!file.exists(), "{policy:?} touched the disk");
    }
}

#[tokio::test]
async fn failed_streaming_encode_preserves_previous_blob() {
    let file = TempFile::new("encode-failure");
    let calls = Calls::default();
    let good_codecs = versioned_codecs(1, calls.clone(), false);
    let original_target = target(file.path(), Digest([4; 32]));
    store_expecting(
        &good_codecs,
        &original_target,
        &OutputSnapshot::new(vec![DynamicValue::from_custom(Blob(vec![1, 2]))]),
        StorePolicy::KnownMiss,
        StoreOutcome::Published,
    )
    .await;
    let original = fs::read(file.path()).unwrap();

    let failing_codecs = versioned_codecs(1, Calls::default(), true);
    let failed = STORE
        .store(
            &target(file.path(), Digest([5; 32])),
            &failing_codecs,
            &OutputSnapshot::new(vec![DynamicValue::from_custom(Blob(vec![8; 1024]))]),
            StorePolicy::KnownMiss,
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
    assert_eq!(fs::read(file.path()).unwrap(), original);
    assert!(publication_temp_files(file.path()).is_empty());
    assert!(
        read_snapshot(&good_codecs, &original_target, 1)
            .await
            .is_some()
    );
}

/// A publication that cannot land leaves the directory it was writing into
/// exactly as it found it — neighbours intact, no temporary left behind.
#[tokio::test]
async fn a_failed_publication_disturbs_nothing_around_it() {
    let file = TempFile::new("publication-failure");
    fs::create_dir_all(file.path()).unwrap();
    let survivor = file.path().join("survivor");
    fs::write(&survivor, b"old").unwrap();
    let codecs = Codecs::default();
    let failed = STORE
        .store(
            &target(file.path(), Digest([9; 32])),
            &codecs,
            &OutputSnapshot::new(vec![DynamicValue::Static(ConstValue::Int(9))]),
            StorePolicy::PreserveCovering,
        )
        .await;

    // The blob path is a directory here. The body still streams fine — an
    // `AtomicFile` writes to a temporary beside the destination — so the
    // failure lands on the publication, not on the encode.
    let Err(StoreError::Publish { path, .. }) = &failed else {
        panic!("a blocked destination must fail at publication, got {failed:?}");
    };
    assert_eq!(path, file.path());
    assert_eq!(fs::read(survivor).unwrap(), b"old");
    assert!(publication_temp_files(file.path()).is_empty());
}

#[tokio::test]
async fn truncated_blob_is_rejected_by_header_check_and_read() {
    let file = TempFile::new("truncated");
    let codecs = Codecs::default();
    let target = target(file.path(), Digest([6; 32]));
    store_expecting(
        &codecs,
        &target,
        &OutputSnapshot::new(vec![DynamicValue::Static(ConstValue::String(
            "payload".into(),
        ))]),
        StorePolicy::KnownMiss,
        StoreOutcome::Published,
    )
    .await;
    let mut bytes = fs::read(file.path()).unwrap();
    bytes.pop();
    fs::write(file.path(), bytes).unwrap();
    let expected = [DynamicValue::Static(ConstValue::String("payload".into()))];
    assert!(!STORE.covers(&target, &expected, &codecs).await);
    assert!(read_snapshot(&codecs, &target, 1).await.is_none());
    assert!(!file.exists(), "a corrupt cache blob is removed");
}
