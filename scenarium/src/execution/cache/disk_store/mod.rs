//! Streamed, atomic persistence for node-output cache blobs.

pub(crate) mod error;
mod format;
pub(crate) mod store_outcome;

use std::io;
use std::path::PathBuf;

use ::common::file_utils::{AtomicFile, PublicationMode};
use tokio::io::{AsyncWriteExt as _, BufWriter};

use crate::DynamicValue;
use crate::data::codec::Codecs;
use crate::data::codec::error::CodecFormatError;
use crate::execution::cache::digest::Digest;
use crate::execution::cache::disk_store::error::{RemovalError, StoreError, StoreResult};
use crate::execution::cache::disk_store::store_outcome::StoreOutcome;
use crate::execution::cache::slot::OutputSnapshot;
use crate::execution::compile::compiled_graph::ExecutionNode;
use crate::graph::func::lambda::OutputDemand;
use crate::graph::identity::NodeId;
use crate::library::Library;
use crate::runtime::context::ContextStore;

#[derive(Debug, Default)]
pub struct DiskStore {
    codecs: Codecs,
    disk_root: Option<PathBuf>,
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
    path: PathBuf,
    pub(super) digest: Digest,
}

impl BlobTarget {
    /// Delete a blob whose body would not decode, so the next run
    /// recomputes instead of meeting the same unreadable frame.
    ///
    /// [`DiskStore::covers_demand`] is header-only, so a blob with an
    /// intact header and a corrupt body goes on passing the reuse check —
    /// while the file survives, every run prunes the producer cone and
    /// fails the same decode. Discarding the removal error left that both
    /// permanent and invisible; reporting it is what makes it
    /// diagnosable, since nothing here can force the unlink through.
    async fn delete(&self) {
        if let Err(error) = tokio::fs::remove_file(&self.path).await
            && error.kind() != io::ErrorKind::NotFound
        {
            tracing::error!(
                path = %self.path.display(),
                %error,
                "could not delete an undecodable cache blob; it will keep failing to load",
            );
        }
    }
}

impl DiskStore {
    pub fn new(library: &Library, disk_root: Option<PathBuf>) -> Self {
        Self {
            codecs: library.codecs(),
            disk_root,
        }
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
        let name = node_id.as_uuid().simple().encode_lower(&mut buf);
        Some(self.disk_root.as_ref()?.join(name))
    }

    pub(crate) async fn remove_node(&self, node_id: NodeId) -> Result<(), RemovalError> {
        let Some(path) = self.node_path(node_id) else {
            return Ok(());
        };
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(RemovalError { path, source }),
        }
    }

    /// Open a blob and measure it, the one way all three readers below start.
    ///
    /// The two `None`s are different answers, which is why this is not a bare
    /// `Option`: `Ok(None)` is the ordinary absence — nothing written under this
    /// target yet — while `Err` is a filesystem that would not answer. Both mean
    /// "cannot serve" to every caller; only whether that is worth reporting
    /// differs, and that is the caller's to decide.
    async fn open_blob(&self, target: &BlobTarget) -> io::Result<Option<(tokio::fs::File, u64)>> {
        let file = match tokio::fs::File::open(&target.path).await {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let file_len = file.metadata().await?.len();
        Ok(Some((file, file_len)))
    }

    async fn covers(&self, target: &BlobTarget, outputs: &[DynamicValue]) -> bool {
        let Ok(Some((mut file, file_len))) = self.open_blob(target).await else {
            return false;
        };
        format::covers_outputs(&mut file, file_len, target.digest, outputs, &self.codecs)
            .await
            .is_ok_and(|covers| covers)
    }

    /// Whether the blob at `target` can serve this run's `demand` — the reuse verdict
    /// without the decode. Header-only (magic, version, digest, arity, codec versions,
    /// per-output coverage), so it needs no [`ContextStore`] and can be answered before the
    /// run commits to reusing the node; [`read`](Self::read) decodes the body later. Any
    /// read or framing failure reads as "cannot serve" — the node then runs and republishes
    /// the blob.
    pub(crate) async fn covers_demand(&self, target: &BlobTarget, demand: &[OutputDemand]) -> bool {
        let Ok(Some((mut file, file_len))) = self.open_blob(target).await else {
            return false;
        };
        format::covers_demand(&mut file, file_len, target.digest, &self.codecs, demand)
            .await
            .unwrap_or(false)
    }

    pub(crate) async fn read(
        &self,
        target: &BlobTarget,
        demand: &[OutputDemand],
        ctx: &mut ContextStore,
    ) -> Option<OutputSnapshot> {
        // Unlike the two coverage checks, this runs after something already
        // promised the blob is there, so a filesystem that will not answer is
        // worth reporting; a plain absence still is not.
        let (mut file, file_len) = match self.open_blob(target).await {
            Ok(Some(opened)) => opened,
            Ok(None) => return None,
            Err(error) => {
                tracing::warn!(path = %target.path.display(), %error, "cache blob could not be read; treating as miss");
                return None;
            }
        };
        match format::read(
            &mut file,
            file_len,
            target.digest,
            &self.codecs,
            ctx,
            demand,
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
    ///
    /// Answers rather than logs. Whether a skipped or failed write is worth
    /// telling a human depends entirely on who asked: this runs after every
    /// node of every run, and also from a flush a user clicked for. Only the
    /// caller knows which, so the reporting is theirs — see [`StoreOutcome`].
    pub(crate) async fn store(
        &self,
        target: &BlobTarget,
        snapshot: &OutputSnapshot,
        policy: StorePolicy,
        ctx: &mut ContextStore,
    ) -> StoreResult {
        if policy == StorePolicy::PreserveCovering && self.covers(target, snapshot.values()).await {
            return Ok(StoreOutcome::AlreadyCovered);
        }
        let path = || target.path.clone();
        if let Some(parent) = target
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|source| StoreError::Directory {
                    path: path(),
                    source,
                })?;
        }

        let file = AtomicFile::new(&target.path, PublicationMode::Cache)
            .await
            .map_err(|source| StoreError::Begin {
                path: path(),
                source,
            })?;
        let mut writer = BufWriter::new(file);
        if let Err(error) = format::write(
            &mut writer,
            target.digest,
            snapshot.values(),
            &self.codecs,
            ctx,
        )
        .await
        {
            // The one encode failure that is not a failure: a type this
            // library has no codec for was never going to be written.
            let CodecFormatError::UnknownType(type_id) = error else {
                return Err(StoreError::Encode {
                    path: path(),
                    source: error,
                });
            };
            return Ok(StoreOutcome::Unsupported { type_id });
        }
        writer.flush().await.map_err(|source| StoreError::Write {
            path: path(),
            source,
        })?;
        writer
            .into_inner()
            .commit()
            .await
            .map_err(|source| StoreError::Publish {
                path: path(),
                source,
            })?;
        Ok(StoreOutcome::Published)
    }
}

#[cfg(test)]
pub(crate) mod internals {
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
        /// header — and so [`DiskStore::covers_demand`]'s verdict — intact. Models a blob
        /// that passes the resolver's probe and then fails to decode.
        pub(crate) fn corrupt_payload(&self, node_id: NodeId, output_count: usize) {
            let path = self
                .node_path(node_id)
                .expect("a disk-backed store has a root");
            let mut bytes = std::fs::read(&path).unwrap();
            bytes[format::internals::body_offset(output_count)] = u8::MAX;
            std::fs::write(&path, bytes).unwrap();
        }
    }
}

#[cfg(test)]
mod tests;
