//! One node's blob: where it lives, the digest it is stored under, and every
//! read, probe and write of it.

use std::io;
use std::path::PathBuf;

use common::file_utils::{AtomicFile, PublicationMode};
use tokio::fs;
use tokio::fs::File;
use tokio::io::{AsyncWriteExt as _, BufWriter};

use crate::DynamicValue;
use crate::data::codec::Codecs;
use crate::execution::cache::digest::Digest;
use crate::execution::cache::disk_store::StorePolicy;
use crate::execution::cache::disk_store::error::{StoreError, StoreResult};
use crate::execution::cache::disk_store::format;
use crate::execution::cache::disk_store::format::{BlobReader, BlobWriter};
use crate::execution::cache::disk_store::store_outcome::StoreOutcome;
use crate::execution::cache::runtime_slot::OutputSnapshot;
use crate::graph::func::lambda::OutputDemand;

/// Where one node's blob lives and the digest it must be stored under: every
/// read, probe and write of that blob goes through here.
#[derive(Debug)]
pub(crate) struct BlobTarget {
    pub(super) path: PathBuf,
    pub(crate) digest: Digest,
}

/// A blob opened for reading, with the length its framing is checked against.
#[derive(Debug)]
struct OpenBlob {
    file: File,
    len: u64,
}

impl OpenBlob {
    const fn reader<'a>(&'a mut self, digest: Digest, codecs: &'a Codecs) -> BlobReader<'a, File> {
        BlobReader {
            reader: &mut self.file,
            file_len: self.len,
            digest,
            codecs,
        }
    }
}

impl BlobTarget {
    /// Delete a blob whose body would not decode, so the next run
    /// recomputes instead of meeting the same unreadable frame.
    ///
    /// [`BlobTarget::covers_demand`] is header-only, so a blob with an
    /// intact header and a corrupt body goes on passing the reuse check —
    /// while the file survives, every run prunes the producer cone and
    /// fails the same decode. A removal that fails is therefore reported:
    /// nothing here can force the unlink through.
    async fn delete(&self) {
        if let Err(error) = fs::remove_file(&self.path).await
            && error.kind() != io::ErrorKind::NotFound
        {
            tracing::error!(
                path = %self.path.display(),
                %error,
                "could not delete an undecodable cache blob; it will keep failing to load",
            );
        }
    }

    /// Create the directory the blob lives in.
    async fn create_directory(&self) -> Result<(), StoreError> {
        let Some(parent) = self
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        else {
            return Ok(());
        };
        fs::create_dir_all(parent)
            .await
            .map_err(|source| StoreError::Directory {
                path: self.path.clone(),
                source,
            })
    }

    /// Open the blob and measure it, the one way all three readers below start.
    ///
    /// The two `None`s are different answers, which is why this is not a bare
    /// `Option`: `Ok(None)` is the ordinary absence — nothing written under this
    /// target yet — while `Err` is a filesystem that would not answer. Both mean
    /// "cannot serve" to every caller; only whether that is worth reporting
    /// differs, and that is the caller's to decide.
    async fn open(&self) -> io::Result<Option<OpenBlob>> {
        let file = match File::open(&self.path).await {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let len = file.metadata().await?.len();
        Ok(Some(OpenBlob { file, len }))
    }

    async fn covers(&self, outputs: &[DynamicValue], codecs: &Codecs) -> bool {
        let Ok(Some(mut blob)) = self.open().await else {
            return false;
        };
        blob.reader(self.digest, codecs)
            .covers_outputs(outputs)
            .await
            .is_ok_and(|covers| covers)
    }

    /// Whether this blob can serve this run's `demand` — the reuse verdict
    /// without the decode. Header-only (magic, version, digest, arity, codec versions,
    /// per-output coverage), so it can be answered before the run commits to reusing the
    /// node; [`read`](Self::read) decodes the body later. Any
    /// read or framing failure reads as "cannot serve" — the node then runs and republishes
    /// the blob.
    pub(crate) async fn covers_demand(&self, codecs: &Codecs, demand: &[OutputDemand]) -> bool {
        let Ok(Some(mut blob)) = self.open().await else {
            return false;
        };
        blob.reader(self.digest, codecs)
            .covers_demand(demand)
            .await
            .unwrap_or(false)
    }

    pub(crate) async fn read(
        &self,
        codecs: &Codecs,
        demand: &[OutputDemand],
    ) -> Option<OutputSnapshot> {
        // Unlike the two coverage checks, this runs after something already
        // promised the blob is there, so a filesystem that will not answer is
        // worth reporting; a plain absence still is not.
        let mut blob = match self.open().await {
            Ok(Some(blob)) => blob,
            Ok(None) => return None,
            Err(error) => {
                tracing::warn!(path = %self.path.display(), %error, "cache blob could not be read; treating as miss");
                return None;
            }
        };
        match blob.reader(self.digest, codecs).read(demand).await {
            Ok(Some(values)) => Some(OutputSnapshot::new(values)),
            Ok(None) => None,
            Err(error) => {
                tracing::warn!(path = %self.path.display(), %error, "cached outputs failed to decode; treating as miss");
                self.delete().await;
                None
            }
        }
    }

    /// Publish a snapshot directly after a known reuse miss, or first preserve an existing
    /// covering blob when the caller has no reuse verdict.
    ///
    /// A snapshot holding a type `codecs` cannot encode is
    /// [`Unsupported`](StoreOutcome::Unsupported) before any I/O: the verdict
    /// is the values' alone, so no file is opened, created or written for it.
    ///
    /// Answers rather than logs. Whether a skipped or failed write is worth
    /// telling a human depends entirely on who asked: this runs after every
    /// node of every run, and also from a flush a user clicked for. Only the
    /// caller knows which, so the reporting is theirs — see [`StoreOutcome`].
    pub(crate) async fn store(
        &self,
        codecs: &Codecs,
        snapshot: &OutputSnapshot,
        policy: StorePolicy,
    ) -> StoreResult {
        if let Some(type_id) = format::unsupported_type(snapshot.values(), codecs) {
            return Ok(StoreOutcome::Unsupported { type_id });
        }
        if policy == StorePolicy::PreserveCovering && self.covers(snapshot.values(), codecs).await {
            return Ok(StoreOutcome::AlreadyCovered);
        }
        let path = || self.path.clone();
        // The store's directory is created only when a write finds it missing:
        // the first write under a new root, or one after the root was removed.
        // Every other write costs no directory syscall.
        let file = match AtomicFile::new(&self.path, PublicationMode::Cache).await {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.create_directory().await?;
                AtomicFile::new(&self.path, PublicationMode::Cache).await
            }
            opened => opened,
        }
        .map_err(|source| StoreError::Begin {
            path: path(),
            source,
        })?;
        let mut writer = BufWriter::new(file);
        BlobWriter {
            writer: &mut writer,
            digest: self.digest,
            codecs,
        }
        .write(snapshot.values())
        .await
        .map_err(|source| StoreError::Encode {
            path: path(),
            source,
        })?;
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
mod tests;
