use std::time::SystemTime;

use common::{TempFile, fs};
use hashbrown::HashSet;

use crate::exec::cache::disk_store::error::PruneError;
use crate::exec::cache::disk_store::{DiskStore, StoreFile};

/// A blob's name is 32 lowercase hex digits, and a temporary is one of those as `AtomicFile`
/// names its write; any other name is not the store's.
#[test]
fn a_store_owns_its_blobs_and_their_temporaries_alone() {
    let blob = "0123456789abcdef0123456789abcdef";
    let cases = [
        (blob.to_owned(), StoreFile::Blob),
        (format!("{blob}.42.7.tmp"), StoreFile::Temp),
        (blob.to_uppercase(), StoreFile::Foreign),
        (blob[1..].to_owned(), StoreFile::Foreign),
        (format!("{blob}0"), StoreFile::Foreign),
        (format!("{}.42.7.tmp", &blob[1..]), StoreFile::Foreign),
        (format!("{blob}.42.tmp"), StoreFile::Foreign),
        (".gitignore".to_owned(), StoreFile::Foreign),
    ];
    for (name, kind) in cases {
        assert_eq!(DiskStore::file_kind(&name), kind, "{name}");
    }
}

/// A root that will not list fails the prune as a listing, naming the root; a root not made yet
/// is a store nothing was written to, with nothing to prune.
#[test]
fn a_root_that_will_not_list_is_reported_as_such() {
    let file = TempFile::new("prune-root");
    let live = HashSet::new();
    assert!(DiskStore::prune_blocking(file.path(), &live, SystemTime::now()).is_empty());

    fs::write(file.path(), b"not a folder").unwrap();
    let failures = DiskStore::prune_blocking(file.path(), &live, SystemTime::now());
    let [PruneError::List { path, .. }] = failures.as_slice() else {
        panic!("a file for a root fails the listing, got {failures:?}");
    };
    assert_eq!(path, file.path());
}
