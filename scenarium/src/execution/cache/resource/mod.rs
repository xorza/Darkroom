//! Per-run filesystem identity collection.
//!
//! Filesystem metadata walks run on Tokio's blocking pool. One job serves the
//! producer-first digest pass and the late bound-path restamps alike, so each
//! path is observed once per run and digest folding itself performs no I/O.

pub(crate) mod error;

use crate::execution::cache::resource::error::StampError;
use std::io;
use std::path::{Path, PathBuf};

use ::common::CancelToken;
use hashbrown::HashSet;

use crate::execution::cache::digest::{Digest, DigestHasher};

/// Metadata identity of one filesystem entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FileId {
    len: u64,
    /// Nanoseconds from the Unix epoch, **negative before it**. Signed
    /// because `duration_since(..).ok().unwrap_or(0)` gave every pre-1970
    /// mtime the same `0` as the epoch itself.
    mtime_ns: i128,
}

/// Signed nanoseconds between `time` and the Unix epoch.
///
/// Split out from [`FileId::from_metadata`] because the pre-epoch arm is
/// the whole point and setting a real file's mtime to 1969 needs a
/// syscall this crate has no dependency for.
fn epoch_offset_ns(time: std::time::SystemTime) -> i128 {
    match time.duration_since(std::time::UNIX_EPOCH) {
        Ok(after) => after.as_nanos() as i128,
        // Pre-epoch: the error carries the distance the other way.
        Err(before) => -(before.duration().as_nanos() as i128),
    }
}

impl FileId {
    /// Fails when the filesystem reports no modification time. Length
    /// alone is not an identity — a same-length edit would reuse the
    /// cache — so the path is given up rather than stamped on half of it.
    fn from_metadata(metadata: &std::fs::Metadata) -> io::Result<Self> {
        Ok(Self {
            len: metadata.len(),
            mtime_ns: epoch_offset_ns(metadata.modified()?),
        })
    }

    fn hash(&self, hasher: &mut DigestHasher) {
        hasher.write_pod(self.len).write_pod(self.mtime_ns);
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) enum FsPathId {
    File(FileId),
    /// Every file beneath the root, folded in relative-path order.
    Directory(Digest),
}

impl FsPathId {
    pub(super) fn hash(&self, hasher: &mut DigestHasher) {
        match self {
            Self::File(file) => {
                hasher.write_bytes(&[0]);
                file.hash(hasher);
            }
            Self::Directory(digest) => {
                hasher.write_bytes(&[1]).write_bytes(&digest.0);
            }
        }
    }
}

/// One stamping pass, and every buffer it works in.
///
/// Owned whole, because a blocking-pool task must own what it touches:
/// the job moves to the pool, runs, and moves back — so the walk borrows
/// nothing of the cache that owns it, and the pass reuses its queue and
/// scratch instead of allocating them again. Nothing of a directory
/// survives its walk but the 32-byte digest.
///
/// The walk only; what a path *was* is remembered by the
/// [`RuntimeCache`](crate::execution::cache::runtime::RuntimeCache) this
/// job belongs to, beside the slots and the digests that fold it.
#[derive(Debug, Default)]
pub(super) struct StampJob {
    /// Paths queued for this pass, deduplicated — a path fifty nodes read
    /// is stamped once.
    requests: HashSet<String>,
    /// What the pass identified, drained into the cache's memo when the
    /// job comes home.
    stamped: Vec<(String, FsPathId)>,
    /// Files anywhere beneath the directory being walked, relative to it,
    /// kept as **raw bytes**.
    ///
    /// `to_string_lossy` maps every non-UTF-8 name onto the same
    /// replacement text, so on any filesystem that admits arbitrary byte
    /// names a rename between two such names left the directory's
    /// identity — and so a pure node's cache key — exactly where it was.
    files: Vec<PathBuf>,
    /// Directories seen but not yet listed, relative to the root.
    pending: Vec<PathBuf>,
}

impl StampJob {
    /// Queue `path` for the next pass, unless it is already queued.
    pub(super) fn request(&mut self, path: &str) {
        if !self.requests.contains(path) {
            self.requests.insert(path.to_string());
        }
    }

    /// Whether [`run`](Self::run) has anything to do — the check that keeps an
    /// all-const run off the blocking pool entirely.
    pub(super) fn is_queued(&self) -> bool {
        !self.requests.is_empty()
    }

    /// Drop the queue without walking it, for a cache that is starting a fresh
    /// run or being emptied. The capacity stays.
    pub(super) fn clear_queue(&mut self) {
        self.requests.clear();
    }

    /// Hand over what the last pass identified, leaving the buffer empty for the
    /// next one. The results live here only between the walk and the cache's
    /// memo; nothing reads them from the job afterwards.
    pub(super) fn drain_stamped(&mut self) -> impl Iterator<Item = (String, FsPathId)> + '_ {
        self.stamped.drain(..)
    }

    /// Stamp every queued path, draining the queue. The first path with no
    /// determinate identity stops the pass and takes its error with it —
    /// a node whose inputs cannot be identified has no sound cache key,
    /// and continuing would mean silently never caching it again.
    pub(super) fn run(&mut self, cancel: &CancelToken) -> Result<(), StampError> {
        // The queue steps out so the walk can borrow the rest of the job
        // while it drains, and steps back in empty, with its capacity.
        let mut requests = std::mem::take(&mut self.requests);
        let resolved = requests.drain().try_for_each(|path| {
            let identity = self.stamp(&path, cancel)?;
            self.stamped.push((path, identity));
            Ok(())
        });
        self.requests = requests;
        resolved
    }

    fn stamp(&mut self, path: &str, cancel: &CancelToken) -> Result<FsPathId, StampError> {
        if cancel.is_cancelled() {
            return Err(StampError::Cancelled);
        }
        // Follows a symlinked root, unlike the walk below: the path a node
        // was handed names what it means to read.
        let metadata = std::fs::metadata(path)?;
        if metadata.is_dir() {
            self.stamp_directory(Path::new(path), cancel)
        } else {
            Ok(FsPathId::File(FileId::from_metadata(&metadata)?))
        }
    }

    /// List the subtree, then fold it — two passes, so the buffer is a
    /// plain list of paths rather than a list of records: nothing has to
    /// be remembered about a file between seeing it and hashing it.
    ///
    /// A file that vanishes between the passes fails the stamp. That is
    /// the race the one-pass walk already had between `read_dir` and its
    /// `stat`, only wider — and failing is the point: a subtree seen in
    /// part must not hash like one seen whole.
    ///
    /// **Only files are folded.** A directory has no content of its own,
    /// and everything it holds surfaces here as a file appearing,
    /// vanishing, or changing its relative path. What that gives up is
    /// the empty directory, which no longer moves the identity at all —
    /// nothing a node reads is a function of it.
    fn stamp_directory(
        &mut self,
        root: &Path,
        cancel: &CancelToken,
    ) -> Result<FsPathId, StampError> {
        self.collect_files(root, cancel)?;
        // `read_dir` order is not a property of the directory, so the fold
        // takes the files in an order the filesystem does not choose.
        // Unstable: relative paths are unique, and it borrows no scratch
        // buffer of its own.
        self.files.sort_unstable();

        let mut hasher = DigestHasher::new();
        hasher.write_pod(self.files.len() as u64);
        for rel in &self.files {
            // `symlink_metadata`, so the second pass reads a link exactly
            // as the first one classified it.
            let metadata = std::fs::symlink_metadata(root.join(rel))?;
            hasher.write_len_prefixed(rel.as_os_str().as_encoded_bytes());
            FileId::from_metadata(&metadata)?.hash(&mut hasher);
        }
        Ok(FsPathId::Directory(hasher.finish()))
    }

    /// Walk **the whole subtree** under `root`, listing every file beneath
    /// it rather than only its immediate children: a pure function handed
    /// a directory consumes it recursively, so the recursive contents are
    /// what its output is a function of. Stamping one level deep let
    /// `root/sub/file` change freely while `root`'s fingerprint — and
    /// every cache key folding it — stood still.
    ///
    /// **Symlinks are listed, never followed.** `DirEntry::file_type`
    /// does not traverse them, so a link to a directory is a leaf here:
    /// the walk terminates (a link pointing back up the tree would
    /// otherwise never stop) and repointing the link still moves the
    /// identity. It is also the cheap question — the type usually arrives
    /// with the directory entry itself, leaving one `stat` per file in
    /// all and none per directory.
    ///
    /// Iterative, because the depth comes from a user-chosen directory
    /// and a deep tree must not decide the stack size.
    fn collect_files(&mut self, root: &Path, cancel: &CancelToken) -> Result<(), StampError> {
        self.files.clear();
        self.pending.clear();
        self.pending.push(PathBuf::new());
        while let Some(rel_dir) = self.pending.pop() {
            for entry in std::fs::read_dir(root.join(&rel_dir))? {
                if cancel.is_cancelled() {
                    return Err(StampError::Cancelled);
                }
                let entry = entry?;
                let rel = rel_dir.join(entry.file_name());
                if entry.file_type()?.is_dir() {
                    self.pending.push(rel);
                } else {
                    self.files.push(rel);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
pub(super) mod internals {
    use crate::execution::cache::resource::{FileId, FsPathId};

    impl FsPathId {
        /// A file identity without a filesystem behind it, so a digest that
        /// folds a path can be pinned to a constant — no test controls a
        /// real file's mtime.
        pub(crate) fn file(len: u64, mtime_ns: i128) -> Self {
            FsPathId::File(FileId { len, mtime_ns })
        }
    }
}

#[cfg(test)]
mod tests;
