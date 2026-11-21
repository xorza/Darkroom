//! One node's blob: where it lives, the digest it is stored under, and every
//! read, probe and write of it.

use std::fs::File;
use std::io::{self, BufReader, BufWriter, Write as _};
use std::path::PathBuf;
use std::sync::Arc;

use common::fs::{self, AtomicFile, PublicationMode};
use common::{ErrorChain, task};

use crate::data::codec::Codecs;
use crate::data::dynamic_value::DynamicValue;
use crate::exec::cache::digest::Digest;
use crate::exec::cache::disk_store::error::{StoreError, StoreResult};
use crate::exec::cache::disk_store::format::{BlobReader, BlobWriter};
use crate::exec::cache::disk_store::store_outcome::StoreOutcome;
use crate::exec::cache::disk_store::{StorePolicy, format};
use crate::exec::cache::runtime_slot::OutputSnapshot;
use crate::graph::func::lambda::OutputDemand;
use crate::graph::program_id::ProgramId;

/// Where one node's blob lives, the node it holds values of, and the digest it must be stored
/// under: every read, probe and write of that blob goes through here.
///
/// Each operation is one closure on the blocking pool over `std` I/O, and takes the target by
/// value into it: a `tokio::fs::File` hops to the pool for each read of at most 2 MiB and copies
/// it once more, and a codec's many small reads and writes would each pay that hop. Refused:
/// buffering the `tokio` file, which keeps the hop per refill.
#[derive(Debug)]
pub(crate) struct BlobTarget {
    pub(super) path: PathBuf,
    pub(super) program_id: ProgramId,
    pub(crate) digest: Digest,
}

/// A blob opened for reading, with the length its framing is checked against.
#[derive(Debug)]
struct OpenBlob {
    file: BufReader<File>,
    len: u64,
}

impl OpenBlob {
    const fn reader<'a>(
        &'a mut self,
        target: &BlobTarget,
        codecs: &'a Codecs,
    ) -> BlobReader<'a, BufReader<File>> {
        BlobReader {
            reader: &mut self.file,
            file_len: self.len,
            program_id: target.program_id,
            digest: target.digest,
            codecs,
        }
    }
}

impl BlobTarget {
    /// Whether this blob can serve `demand` — the reuse verdict without the decode. Header-only
    /// (magic, version, digest, arity, codec versions, per-output coverage), so it can be answered
    /// before the run commits to reusing the node; [`read`](Self::read) decodes the body later.
    /// Any read or framing failure reads as "cannot serve" — the node then runs and republishes
    /// the blob.
    pub(crate) async fn covers_demand(
        self,
        codecs: Arc<Codecs>,
        demand: Box<[OutputDemand]>,
    ) -> bool {
        task::run_blocking(move || {
            let Ok(Some(mut blob)) = self.open() else {
                return false;
            };
            blob.reader(&self, &codecs)
                .covers_demand(&demand)
                .unwrap_or(false)
        })
        .await
        .expect("a blob operation ends before its runtime")
    }

    /// The values that serve `demand`, or `None`. The header is read again, beside the body, so
    /// the load holds the file as it is now and not as the probe saw it. A blob that would not
    /// decode, or whose payload no longer hashes as written, is removed, so the next run
    /// recomputes rather than meet it again.
    pub(crate) async fn read(
        self,
        codecs: Arc<Codecs>,
        demand: Box<[OutputDemand]>,
    ) -> Option<OutputSnapshot> {
        task::run_blocking(move || self.read_blocking(&codecs, &demand))
            .await
            .expect("a blob operation ends before its runtime")
    }

    fn read_blocking(&self, codecs: &Codecs, demand: &[OutputDemand]) -> Option<OutputSnapshot> {
        // This runs after something already promised the blob is there, so a filesystem that will
        // not answer is worth reporting; a plain absence still is not.
        let mut blob = match self.open() {
            Ok(Some(blob)) => blob,
            Ok(None) => return None,
            Err(error) => {
                tracing::warn!(
                    path = %self.path.display(),
                    error = %ErrorChain::new(&error),
                    "cache blob could not be read; treating as miss"
                );
                return None;
            }
        };
        match blob.reader(self, codecs).read(demand) {
            Ok(Some(values)) => Some(OutputSnapshot::new(values)),
            Ok(None) => None,
            Err(error) => {
                tracing::warn!(
                    path = %self.path.display(),
                    error = %ErrorChain::new(&error),
                    "cached outputs failed to decode; treating as miss"
                );
                self.delete();
                None
            }
        }
    }

    /// Publish `values` directly after a known reuse miss, or first preserve an existing covering
    /// blob when the caller has no reuse verdict.
    ///
    /// Values holding a type `codecs` cannot encode are
    /// [`Unsupported`](StoreOutcome::Unsupported) before any I/O: the verdict
    /// is the values' alone, so no file is opened, created or written for it.
    ///
    /// Answers rather than logs: this runs after every node of every run, and also from a flush a
    /// user clicked for, so only the caller knows whether a skipped or failed write is worth
    /// telling a human — see [`StoreOutcome`].
    pub(crate) async fn store(
        self,
        codecs: Arc<Codecs>,
        values: Vec<DynamicValue>,
        policy: StorePolicy,
    ) -> StoreResult {
        if let Some(type_id) = format::unsupported_type(&values, &codecs) {
            return Ok(StoreOutcome::Unsupported { type_id });
        }
        task::run_blocking(move || self.store_blocking(&codecs, &values, policy))
            .await
            .expect("a blob operation ends before its runtime")
    }

    fn store_blocking(
        &self,
        codecs: &Codecs,
        values: &[DynamicValue],
        policy: StorePolicy,
    ) -> StoreResult {
        if policy == StorePolicy::PreserveCovering && self.covers(values, codecs) {
            return Ok(StoreOutcome::AlreadyCovered);
        }
        let path = || self.path.clone();
        // The directory is created only when a write finds it missing (the first write under a new
        // root, or after the root was removed), so every other write costs no directory syscall.
        let file = match AtomicFile::new(&self.path, PublicationMode::Cache) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.create_dir()?;
                AtomicFile::new(&self.path, PublicationMode::Cache)
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
            program_id: self.program_id,
            digest: self.digest,
            codecs,
        }
        .write(values)
        .map_err(|source| StoreError::Encode {
            path: path(),
            source,
        })?;
        writer.flush().map_err(|source| StoreError::Write {
            path: path(),
            source,
        })?;
        writer
            .into_inner()
            .map_err(|error| StoreError::Write {
                path: path(),
                source: error.into_error(),
            })?
            .commit()
            .map_err(|source| StoreError::Publish {
                path: path(),
                source,
            })?;
        Ok(StoreOutcome::Published)
    }

    /// Delete a blob whose body would not decode, so the next run recomputes instead of meeting
    /// the same unreadable frame.
    ///
    /// A probe is header-only, so a blob with an intact header and a corrupt body goes on passing
    /// the reuse check — every run would prune the producer cone and fail the same decode. A
    /// removal that fails is therefore reported: nothing here can force the unlink through.
    fn delete(&self) {
        if let Err(error) = fs::remove_file(&self.path)
            && error.kind() != io::ErrorKind::NotFound
        {
            tracing::error!(
                path = %self.path.display(),
                error = %ErrorChain::new(&error),
                "could not delete an undecodable cache blob; it will keep failing to load",
            );
        }
    }

    /// Create the directory the blob lives in.
    fn create_dir(&self) -> Result<(), StoreError> {
        let Some(parent) = self
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        else {
            return Ok(());
        };
        fs::create_dir_all(parent).map_err(|source| StoreError::Dir {
            path: self.path.clone(),
            source,
        })
    }

    /// Open the blob and measure it, the one way every reader above starts.
    ///
    /// `Ok(None)` is the ordinary absence — nothing written under this target yet — while `Err`
    /// is a filesystem that would not answer. Both mean "cannot serve"; only whether that is
    /// worth reporting differs, and that is the caller's to decide.
    fn open(&self) -> io::Result<Option<OpenBlob>> {
        let file = match fs::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let len = file.metadata()?.len();
        Ok(Some(OpenBlob {
            file: BufReader::new(file),
            len,
        }))
    }

    fn covers(&self, values: &[DynamicValue], codecs: &Codecs) -> bool {
        let Ok(Some(mut blob)) = self.open() else {
            return false;
        };
        blob.reader(self, codecs)
            .covers_outputs(values)
            .is_ok_and(|covers| covers)
    }
}

#[cfg(test)]
mod tests;
