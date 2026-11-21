//! Streamed, atomic persistence for node-output cache blobs.

pub(super) mod blob_target;
pub(crate) mod error;
mod format;
pub(crate) mod store_outcome;

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use common::{fs, task};
use hashbrown::HashSet;

use crate::exec::cache::digest::Digest;
use crate::exec::cache::disk_store::blob_target::BlobTarget;
use crate::exec::cache::disk_store::error::{PruneError, RemovalError};
use crate::graph::program_id::ProgramId;

/// Where the node-output cache persists: a directory of blobs, each named by the digest of its
/// program node's path, or nowhere. The codecs belong to each program, so the store is the root
/// alone.
///
/// A name is a digest of the path, as a file name has a length limit a path of eight ids
/// passes; the blob holds the path itself, which a read checks, so a name two paths share
/// serves neither the wrong value.
///
/// **A blob keeps its node's name, and a store is pruned against the program it serves**, so
/// it holds one blob per node of that program at most. Cost: a node deleted and restored after
/// the prune recomputes, and twins store their payload twice. Refused: naming by digest under a
/// size cap, as Bazel's disk cache, a content-addressable store that "can be garbage collected"
/// ([remote caching](https://bazel.build/remote/caching)), and sccache, whose "default cache size
/// is 10 gigabytes" ([local cache](https://github.com/mozilla/sccache/blob/main/docs/Local.md)):
/// a cap setting, access times and a collector, a cache policy of its own.
#[derive(Debug, Default)]
pub struct DiskStore {
    root: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StorePolicy {
    /// The caller already established that no usable blob covers the new snapshot.
    KnownMiss,
    /// The on-disk state is unknown, so a covering blob must survive unchanged.
    PreserveCovering,
}

impl DiskStore {
    /// Versions the blob name: a change to it orphans every blob.
    const NAME_DOMAIN: &'static [u8] = b"scenarium-blob-name-v1";
    /// How many hex digits a blob's name has.
    const NAME_LEN: usize = 32;
    /// A day: longer than any one blob takes to write, so a temporary this old is one a killed
    /// process left, never a write in progress.
    const STALE_TEMP: Duration = Duration::from_hours(24);

    /// A store under `root`, or a memory-only one for `None`.
    pub const fn new(root: Option<PathBuf>) -> Self {
        Self { root }
    }

    /// Where `program_id`'s blob under `digest` lives, or `None` for a
    /// memory-only store.
    pub(super) fn blob_target(&self, program_id: ProgramId, digest: Digest) -> Option<BlobTarget> {
        let path = self.node_path(program_id)?;
        Some(BlobTarget {
            path,
            program_id,
            digest,
        })
    }

    /// The file of `program_id`'s blob.
    fn node_path(&self, program_id: ProgramId) -> Option<PathBuf> {
        let root = self.root.as_ref()?;
        Some(root.join(Self::name(program_id)))
    }

    /// `program_id`'s blob name: 32 hex digits of the first 16 bytes of the path's blake3
    /// digest, as long as a node id's.
    fn name(program_id: ProgramId) -> String {
        let mut hasher = blake3::Hasher::new();
        hasher.update(Self::NAME_DOMAIN);
        hasher.update(&program_id.to_bytes());
        hasher.finalize().to_hex()[..Self::NAME_LEN].to_owned()
    }

    /// Remove the blobs of `program_ids`, in one blocking pass: an answer for each, in their
    /// order. A memory-only store has nothing to remove.
    pub(crate) async fn remove_nodes(
        &self,
        program_ids: Vec<ProgramId>,
    ) -> Vec<Result<(), RemovalError>> {
        let Some(root) = self.root.clone().filter(|_| !program_ids.is_empty()) else {
            return program_ids.iter().map(|_| Ok(())).collect();
        };
        task::run_blocking(move || {
            program_ids
                .into_iter()
                .map(|program_id| remove(root.join(Self::name(program_id))))
                .collect()
        })
        .await
        .expect("a blob operation ends before its runtime")
    }

    /// Remove each blob none of `live` names, and each temporary a write left over
    /// [`STALE_TEMP`](Self::STALE_TEMP) ago: one listing, in one blocking pass. A file of any
    /// other name is not the store's, and stays. What would not go is answered.
    pub(crate) async fn prune(&self, live: impl IntoIterator<Item = ProgramId>) -> Vec<PruneError> {
        let Some(root) = self.root.clone() else {
            return Vec::new();
        };
        let live: HashSet<String> = live.into_iter().map(Self::name).collect();
        task::run_blocking(move || Self::prune_blocking(&root, &live, SystemTime::now()))
            .await
            .expect("a blob operation ends before its runtime")
    }

    fn prune_blocking(root: &Path, live: &HashSet<String>, now: SystemTime) -> Vec<PruneError> {
        let mut failures = Vec::new();
        let listed = |source| PruneError::List {
            path: root.to_path_buf(),
            source,
        };
        let entries = match fs::read_dir(root) {
            Ok(entries) => entries,
            // A store nothing was written to has no folder yet.
            Err(error) if error.kind() == io::ErrorKind::NotFound => return failures,
            Err(source) => {
                failures.push(listed(source));
                return failures;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(source) => {
                    failures.push(listed(source));
                    continue;
                }
            };
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let dead = match Self::file_kind(name) {
                StoreFile::Foreign => false,
                StoreFile::Blob => !live.contains(name),
                StoreFile::Temp => fs::entry_metadata(&entry)
                    .and_then(|metadata| metadata.modified())
                    .is_ok_and(|modified| {
                        now.duration_since(modified)
                            .is_ok_and(|age| age > Self::STALE_TEMP)
                    }),
            };
            if dead && let Err(error) = remove(entry.path()) {
                failures.push(error.into());
            }
        }
        failures
    }

    /// What `name` is in a store's folder: a blob's name, the temporary of a write to one, or
    /// neither.
    fn file_kind(name: &str) -> StoreFile {
        let is_name = |name: &str| {
            name.len() == Self::NAME_LEN
                && name
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        };
        if is_name(name) {
            StoreFile::Blob
        } else if fs::temporary_destination(name).is_some_and(is_name) {
            StoreFile::Temp
        } else {
            StoreFile::Foreign
        }
    }
}

/// What a file in a store's folder is to the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StoreFile {
    Blob,
    Temp,
    Foreign,
}

/// Remove the file at `path`; one already gone is removed.
fn remove(path: PathBuf) -> Result<(), RemovalError> {
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(RemovalError { path, source }),
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use std::path::PathBuf;

    use common::fs;

    use crate::exec::cache::disk_store::{DiskStore, format};
    use crate::graph::program_id::ProgramId;

    impl DiskStore {
        /// Where this node's blob lives, so a fixture can corrupt, read back or delete one.
        pub(crate) fn blob_path(&self, program_id: ProgramId) -> PathBuf {
            self.node_path(program_id)
                .expect("a disk-backed store has a root")
        }

        /// Replace the first output payload's value tag with an unknown one, leaving the header —
        /// and so `BlobTarget::covers_demand`'s verdict — intact: a blob that passes the probe and
        /// then fails to decode.
        pub(crate) fn corrupt_payload(&self, program_id: ProgramId, output_count: usize) {
            let path = self
                .node_path(program_id)
                .expect("a disk-backed store has a root");
            let mut bytes = fs::read(&path).unwrap();
            bytes[format::header_len(output_count)] = u8::MAX;
            fs::write(&path, bytes).unwrap();
        }

        /// Rewrite the blob's header to the previous format version, as an earlier build wrote it.
        pub(crate) fn age_format(&self, program_id: ProgramId) {
            let path = self
                .node_path(program_id)
                .expect("a disk-backed store has a root");
            let mut bytes = fs::read(&path).unwrap();
            format::internals::age_format(&mut bytes);
            fs::write(&path, bytes).unwrap();
        }
    }
}

#[cfg(test)]
mod tests;
