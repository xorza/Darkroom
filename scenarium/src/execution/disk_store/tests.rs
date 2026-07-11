use super::*;

/// `has_current_blob` is the presence probe the reuse policy trusts: true iff a
/// blob at the target's path carries exactly the target's digest — a file stamped
/// with a superseded digest, or no file at all, is a miss.
#[tokio::test]
async fn has_current_blob_requires_the_exact_stamped_digest() {
    let dir =
        std::env::temp_dir().join(format!("scenarium-diskstore-probe-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("blob");
    let stamped = Digest([3u8; 32]);
    blob::write(
        &path,
        stamped,
        &[DynamicValue::Static(crate::data::StaticValue::Int(6))],
        &Library::default(),
        &mut ContextManager::default(),
    )
    .await
    .unwrap();

    let store = DiskStore::default();
    let target = |digest| BlobTarget {
        path: path.clone(),
        digest,
    };
    assert!(
        store.has_current_blob(&target(stamped)),
        "the stamped digest is current"
    );
    assert!(
        !store.has_current_blob(&target(Digest([4u8; 32]))),
        "another digest means the blob is superseded, not present"
    );
    assert!(
        !store.has_current_blob(&BlobTarget {
            path: dir.join("absent"),
            digest: stamped,
        }),
        "no file, no blob"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
