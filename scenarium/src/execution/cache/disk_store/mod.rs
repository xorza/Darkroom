//! Streamed, atomic persistence for node-output cache blobs.

pub(super) mod blob_target;
pub(crate) mod error;
mod format;
pub(crate) mod store_outcome;

use std::io;
use std::path::PathBuf;

use tokio::fs;

use crate::execution::cache::digest::Digest;
use crate::execution::cache::disk_store::blob_target::BlobTarget;
use crate::execution::cache::disk_store::error::RemovalError;
use crate::execution::compile::compiled_graph::ExecutionNode;
use crate::graph::identity::NodeId;

/// Where the node-output cache persists: a directory of blobs named by node
/// id, or nowhere. The codecs that read and write the blobs belong to each
/// program, so the store is the root alone.
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
    /// A store under `root`, or a memory-only one for `None`.
    pub const fn new(root: Option<PathBuf>) -> Self {
        Self { root }
    }

    pub(super) fn blob_target(
        &self,
        node_id: NodeId,
        e_node: &ExecutionNode,
        digest: Option<Digest>,
    ) -> Option<BlobTarget> {
        if !e_node.cache.persists_to_disk() {
            return None;
        }
        let digest = digest?;
        let path = self.node_path(node_id)?;
        Some(BlobTarget { path, digest })
    }

    fn node_path(&self, node_id: NodeId) -> Option<PathBuf> {
        let mut buf = [0u8; 32];
        let name = node_id.hex(&mut buf);
        Some(self.root.as_ref()?.join(name))
    }

    pub(crate) async fn remove_node(&self, node_id: NodeId) -> Result<(), RemovalError> {
        let Some(path) = self.node_path(node_id) else {
            return Ok(());
        };
        match fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(RemovalError { path, source }),
        }
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use std::fs;
    use std::path::PathBuf;

    use crate::execution::cache::disk_store::{DiskStore, format};
    use crate::graph::identity::NodeId;

    impl DiskStore {
        /// Where this node's blob lives, so a fixture can corrupt, read back or
        /// delete one without re-deriving the store's own naming.
        pub(crate) fn blob_path(&self, node_id: NodeId) -> PathBuf {
            self.node_path(node_id)
                .expect("a disk-backed store has a root")
        }

        /// Replace the first output payload's value tag with an unknown one, leaving the
        /// header — and so `BlobTarget::covers_demand`'s verdict — intact. Models a blob
        /// that passes the resolver's probe and then fails to decode.
        pub(crate) fn corrupt_payload(&self, node_id: NodeId, output_count: usize) {
            let path = self
                .node_path(node_id)
                .expect("a disk-backed store has a root");
            let mut bytes = fs::read(&path).unwrap();
            bytes[format::header_len(output_count)] = u8::MAX;
            fs::write(&path, bytes).unwrap();
        }

        /// Rewrite the blob's header to the previous format version, the way a blob an
        /// earlier build wrote looks to this one.
        pub(crate) fn age_format(&self, node_id: NodeId) {
            let path = self
                .node_path(node_id)
                .expect("a disk-backed store has a root");
            let mut bytes = fs::read(&path).unwrap();
            format::internals::age_format(&mut bytes);
            fs::write(&path, bytes).unwrap();
        }
    }
}
