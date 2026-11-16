//! Streamed, atomic persistence for node-output cache blobs.

pub(super) mod blob_target;
pub(crate) mod error;
mod format;
pub(crate) mod store_outcome;

use std::io;
use std::path::PathBuf;

use common::fs;

use crate::exec::cache::digest::Digest;
use crate::exec::cache::disk_store::blob_target::BlobTarget;
use crate::exec::cache::disk_store::error::RemovalError;
use crate::graph::program_id::ProgramId;

/// Where the node-output cache persists: a directory of blobs, each named by the digest of its
/// program node's path, or nowhere. The codecs belong to each program, so the store is the root
/// alone.
///
/// A name is a digest of the path, as a file name has a length limit a path of eight ids
/// passes; the blob holds the path itself, which a read checks, so a name two paths share
/// serves neither the wrong value.
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

    /// The file of `program_id`'s blob: 32 hex digits of the first 16 bytes of the path's
    /// blake3 digest, as long as a node id's.
    fn node_path(&self, program_id: ProgramId) -> Option<PathBuf> {
        let root = self.root.as_ref()?;
        let mut hasher = blake3::Hasher::new();
        hasher.update(Self::NAME_DOMAIN);
        hasher.update(&program_id.to_bytes());
        Some(root.join(&hasher.finalize().to_hex()[..32]))
    }

    pub(crate) async fn remove_node(&self, program_id: ProgramId) -> Result<(), RemovalError> {
        let Some(path) = self.node_path(program_id) else {
            return Ok(());
        };
        match fs::remove_file_async(&path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(RemovalError { path, source }),
        }
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
