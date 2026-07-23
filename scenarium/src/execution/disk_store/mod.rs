//! Streamed, atomic persistence for node-output cache blobs.

mod format;

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};

use common::file_utils::{AtomicFile, PublicationMode};
use tokio::io::{AsyncWriteExt as _, BufWriter};

use crate::DynamicValue;
use crate::execution::cache::OutputSnapshot;
use crate::execution::codec;
use crate::execution::digest::Digest;
use crate::execution::identity::ExecutionNodeId;
use crate::execution::program::ExecutionNode;
use crate::library::Library;
use crate::node::lambda::OutputDemand;
use crate::runtime::context::ContextManager;

#[derive(Debug, Default)]
pub struct DiskStore {
    library: Arc<Library>,
    disk_root: Option<PathBuf>,
    #[cfg(test)]
    pub(crate) store_io: StoreIoCounts,
}

#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct StoreIoCounts {
    pub(crate) coverage_probes: AtomicU64,
    pub(crate) publication_attempts: AtomicU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StorePolicy {
    /// The caller already established that no usable blob covers the new snapshot.
    KnownMiss,
    /// The on-disk state is unknown, so a covering blob must survive unchanged.
    PreserveCovering,
}

#[derive(Debug)]
pub(crate) struct BlobTarget {
    pub(crate) path: PathBuf,
    pub(crate) digest: Digest,
}

impl BlobTarget {
    pub(crate) async fn delete(&self) {
        let _ = tokio::fs::remove_file(&self.path).await;
    }
}

impl DiskStore {
    pub fn new(library: Arc<Library>, disk_root: Option<PathBuf>) -> Self {
        Self {
            library,
            disk_root,
            #[cfg(test)]
            store_io: StoreIoCounts::default(),
        }
    }

    pub(crate) fn blob_target(
        &self,
        e_node_id: ExecutionNodeId,
        e_node: &ExecutionNode,
        digest: Option<Digest>,
    ) -> Option<BlobTarget> {
        if !e_node.cache.persists_to_disk() {
            return None;
        }
        let digest = digest?;
        let path = self.node_path(e_node_id)?;
        Some(BlobTarget { path, digest })
    }

    fn node_path(&self, e_node_id: ExecutionNodeId) -> Option<PathBuf> {
        let mut buf = [0u8; 32];
        let name = e_node_id.as_uuid().simple().encode_lower(&mut buf);
        Some(self.disk_root.as_ref()?.join(name))
    }

    pub(crate) async fn remove_node(&self, e_node_id: ExecutionNodeId) -> io::Result<()> {
        let Some(path) = self.node_path(e_node_id) else {
            return Ok(());
        };
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(io::Error::new(
                error.kind(),
                format!("failed to remove {}: {error}", path.display()),
            )),
        }
    }

    pub(crate) async fn covers(&self, target: &BlobTarget, outputs: &[DynamicValue]) -> bool {
        let Ok(mut file) = tokio::fs::File::open(&target.path).await else {
            return false;
        };
        let Some(file_len) = file.metadata().await.ok().map(|metadata| metadata.len()) else {
            return false;
        };
        format::covers_outputs(&mut file, file_len, target.digest, outputs, &self.library)
            .await
            .is_ok_and(|covers| covers)
    }

    pub(crate) async fn read(
        &self,
        target: &BlobTarget,
        demand: &[OutputDemand],
    ) -> Option<OutputSnapshot> {
        let mut file = match tokio::fs::File::open(&target.path).await {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return None,
            Err(error) => {
                tracing::warn!(path = %target.path.display(), %error, "cache read failed; treating as miss");
                return None;
            }
        };
        let file_len = match file.metadata().await {
            Ok(metadata) => metadata.len(),
            Err(error) => {
                tracing::warn!(path = %target.path.display(), %error, "cache metadata read failed; treating as miss");
                return None;
            }
        };
        match format::read(
            &mut file,
            file_len,
            target.digest,
            &self.library,
            demand.len(),
            |index| !demand[index].is_skip(),
        )
        .await
        {
            Ok(Some(values)) => Some(OutputSnapshot::new(values)),
            Ok(None) => None,
            Err(error) => {
                tracing::warn!(path = %target.path.display(), %error, "cached outputs failed to decode; treating as miss");
                target.delete().await;
                None
            }
        }
    }

    /// Publish a snapshot directly after a known reuse miss, or first preserve an existing
    /// covering blob when the caller has no reuse verdict.
    pub(crate) async fn store(
        &self,
        target: &BlobTarget,
        snapshot: &OutputSnapshot,
        policy: StorePolicy,
        ctx: &mut ContextManager,
    ) {
        if policy == StorePolicy::PreserveCovering {
            #[cfg(test)]
            self.store_io
                .coverage_probes
                .fetch_add(1, Ordering::Relaxed);
        }
        if policy == StorePolicy::PreserveCovering && self.covers(target, &snapshot.values).await {
            return;
        }
        #[cfg(test)]
        self.store_io
            .publication_attempts
            .fetch_add(1, Ordering::Relaxed);
        if let Some(parent) = target
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            && let Err(error) = tokio::fs::create_dir_all(parent).await
        {
            tracing::warn!(path = %target.path.display(), %error, "failed to create output-cache directory");
            return;
        }

        let file = AtomicFile::new(&target.path, PublicationMode::Cache).await;
        let file = match file {
            Ok(file) => file,
            Err(error) => {
                tracing::warn!(path = %target.path.display(), %error, "failed to begin output-cache publication");
                return;
            }
        };
        let mut writer = BufWriter::new(file);
        if let Err(error) = format::write(
            &mut writer,
            target.digest,
            &snapshot.values,
            &self.library,
            ctx,
        )
        .await
        {
            if !matches!(error, codec::Error::UnknownType(_)) {
                tracing::warn!(path = %target.path.display(), %error, "failed to encode output cache");
            }
            return;
        }
        if let Err(error) = writer.flush().await {
            tracing::warn!(path = %target.path.display(), %error, "failed to flush output cache");
            return;
        }
        if let Err(error) = writer.into_inner().commit().await {
            tracing::warn!(path = %target.path.display(), %error, "failed to publish output cache");
        }
    }
}

#[cfg(test)]
mod tests;
