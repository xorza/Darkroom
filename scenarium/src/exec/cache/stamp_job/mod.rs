//! Per-run filesystem identity collection.
//!
//! Filesystem metadata walks run on Tokio's blocking pool. One job serves the
//! producer-first digest pass and the late bound-path restamps alike, so each
//! path is observed once per run and digest folding itself performs no I/O.

pub(crate) mod error;

use std::mem;
use std::ops::Range;
use std::path::{MAIN_SEPARATOR_STR, Path, PathBuf};

use common::{CancelToken, FileIdentity, fs};
use hashbrown::HashSet;

use crate::exec::cache::digest::{Digest, DigestHasher, InputTag};
use crate::exec::cache::stamp_job::error::StampError;

#[derive(Clone, Copy, Debug)]
pub(super) enum FsPathId {
    File(FileIdentity),
    /// Every file beneath the root, folded in relative-path order.
    Dir(Digest),
}

impl FsPathId {
    pub(super) fn hash(&self, hasher: &mut DigestHasher) {
        match self {
            Self::File(file) => {
                hasher
                    .write_input_tag(InputTag::FilePath)
                    .write_file_identity(file);
            }
            Self::Dir(digest) => {
                hasher
                    .write_input_tag(InputTag::DirPath)
                    .write_digest(digest);
            }
        }
    }
}

/// One stamping pass, and every buffer it works in.
///
/// Owned whole, because a blocking-pool task must own what it touches: the job moves to the pool,
/// runs, and moves back, so the walk borrows nothing of the cache and the pass reuses its queue
/// and scratch. Nothing of a directory survives its walk but the 32-byte digest. The walk only;
/// what a path *was* is remembered by the
/// [`PathIdentities`](crate::exec::cache::path_identities::PathIdentities) this job belongs to.
#[derive(Debug, Default)]
pub(super) struct StampJob {
    /// Paths queued for this pass, deduplicated.
    requests: HashSet<String>,
    /// What the pass identified, drained into the cache's memo when the
    /// job comes home.
    stamped: Vec<(String, FsPathId)>,
    /// The relative paths of the directory being walked, back to back, as **raw bytes**:
    /// `to_string_lossy` would map every non-UTF-8 name onto the same replacement text, so a
    /// rename between two such names would leave the directory's identity — and a pure node's
    /// cache key — unchanged.
    names: Vec<u8>,
    /// Files anywhere beneath the directory being walked.
    files: Vec<WalkedFile>,
    /// Directories seen but not yet listed, relative to the root.
    pending: Vec<PathBuf>,
}

/// One file a directory walk met: its relative path in [`StampJob::names`], and its identity.
#[derive(Debug, Clone)]
struct WalkedFile {
    name: Range<usize>,
    identity: FileIdentity,
}

impl StampJob {
    /// Queue `path` for the next pass. The caller owns the key, so a path
    /// it identified before is queued without a copy.
    pub(super) fn request(&mut self, path: String) {
        debug_assert!(!self.requests.contains(&path), "{path} is already queued");
        self.requests.insert(path);
    }

    /// Whether `path` waits in the queue.
    pub(super) fn is_requested(&self, path: &str) -> bool {
        self.requests.contains(path)
    }

    /// Whether [`run`](Self::run) has anything to do — keeps an all-const run off the blocking
    /// pool entirely.
    pub(super) fn is_queued(&self) -> bool {
        !self.requests.is_empty()
    }

    /// Drop the queue without walking it, for a cache that is starting a fresh
    /// run or being emptied. The capacity stays.
    pub(super) fn clear_queue(&mut self) {
        self.requests.clear();
    }

    /// Hand over what the last pass identified, leaving the buffer empty for the next one.
    pub(super) fn drain_stamped(&mut self) -> impl Iterator<Item = (String, FsPathId)> + '_ {
        self.stamped.drain(..)
    }

    /// Stamp every queued path, draining the queue, and report the first path
    /// that had no determinate identity.
    ///
    /// **An unreadable path costs its own node, not the pass.** The pass is shared by every node
    /// of the run, so the walk carries on past a failure and the paths queued behind it still
    /// land. The failing node is left without a digest, and the run loop re-stamps it at its own
    /// turn, where the error is reported as that node's.
    ///
    /// A failure is reported, never remembered: a path that would not read during the pre-run
    /// pass may read at the node's turn, once an upstream producer has written it.
    ///
    /// Cancellation is the one thing that stops the pass where it stands: every path left would
    /// raise it again.
    pub(super) fn run(&mut self, cancel: &CancelToken) -> Result<(), StampError> {
        // The queue steps out so the walk can borrow the rest of the job
        // while it drains, and steps back in empty, with its capacity.
        let mut requests = mem::take(&mut self.requests);
        let mut failure = None;
        for path in requests.drain() {
            match self.stamp(&path, cancel) {
                Ok(identity) => self.stamped.push((path, identity)),
                Err(StampError::Cancelled) => {
                    failure = Some(StampError::Cancelled);
                    break;
                }
                // The first failure is reported; one message can only carry one.
                Err(error) => {
                    if failure.is_none() {
                        failure = Some(error);
                    }
                }
            }
        }
        self.requests = requests;
        failure.map_or(Ok(()), Err)
    }

    fn stamp(&mut self, path: &str, cancel: &CancelToken) -> Result<FsPathId, StampError> {
        if cancel.is_cancelled() {
            return Err(StampError::Cancelled);
        }
        // Follows a symlinked root, unlike the walk below: the path a node
        // was handed names what it means to read.
        let path = Path::new(path);
        let metadata = fs::metadata(path).map_err(StampError::io(path))?;
        if metadata.is_dir() {
            self.stamp_dir(path, cancel)
        } else {
            let file = FileIdentity::from_metadata(&metadata).map_err(StampError::io(path))?;
            Ok(FsPathId::File(file))
        }
    }

    /// Walk the subtree, then fold it in an order the filesystem does not choose.
    ///
    /// **Only files are folded.** A directory has no content of its own, and everything it holds
    /// surfaces as a file appearing, vanishing, or changing its relative path. The empty
    /// directory does not move the identity; nothing a node reads is a function of it.
    fn stamp_dir(&mut self, root: &Path, cancel: &CancelToken) -> Result<FsPathId, StampError> {
        self.collect_files(root, cancel)?;
        // `read_dir` order is not a property of the directory. Unstable: relative paths are
        // unique.
        let names = &self.names;
        let name = |file: &WalkedFile| &names[file.name.clone()];
        self.files
            .sort_unstable_by(|left, right| name(left).cmp(name(right)));

        let mut hasher = DigestHasher::new();
        hasher.write_pod(self.files.len() as u64);
        for file in &self.files {
            hasher
                .write_len_prefixed(name(file))
                .write_file_identity(&file.identity);
        }
        Ok(FsPathId::Dir(hasher.finish()))
    }

    /// Walk **the whole subtree** under `root`, identifying every file beneath it from the
    /// listing: a pure function handed a directory consumes it recursively, so `root/sub/file`
    /// changing has to move `root`'s fingerprint. One pass, so a file is identified as it was
    /// listed, and no file can vanish between a listing and its identity.
    ///
    /// **Symlinks are listed, never followed.** `DirEntry::file_type` and its metadata do not
    /// traverse them, so a link to a directory is a leaf: the walk terminates (a link pointing
    /// back up the tree would otherwise never stop) and repointing the link still moves the
    /// identity. The type usually arrives with the directory entry, leaving one `stat` per file
    /// and none per directory.
    ///
    /// Iterative, because a user-chosen directory's depth must not decide the stack size.
    fn collect_files(&mut self, root: &Path, cancel: &CancelToken) -> Result<(), StampError> {
        self.names.clear();
        self.files.clear();
        self.pending.clear();
        self.pending.push(PathBuf::new());
        while let Some(rel_dir) = self.pending.pop() {
            let dir = root.join(&rel_dir);
            for entry in fs::read_dir(&dir).map_err(StampError::io(&dir))? {
                if cancel.is_cancelled() {
                    return Err(StampError::Cancelled);
                }
                let entry = entry.map_err(StampError::io(&dir))?;
                // The path is joined for a failure alone, never per file.
                let failed = |source| StampError::Io {
                    path: entry.path(),
                    source,
                };
                let file_name = entry.file_name();
                if entry.file_type().map_err(failed)?.is_dir() {
                    self.pending.push(rel_dir.join(file_name));
                    continue;
                }
                let metadata = fs::entry_metadata(&entry).map_err(failed)?;
                let identity = FileIdentity::from_metadata(&metadata).map_err(failed)?;
                let start = self.names.len();
                if !rel_dir.as_os_str().is_empty() {
                    self.names
                        .extend_from_slice(rel_dir.as_os_str().as_encoded_bytes());
                    self.names.extend_from_slice(MAIN_SEPARATOR_STR.as_bytes());
                }
                self.names.extend_from_slice(file_name.as_encoded_bytes());
                self.files.push(WalkedFile {
                    name: start..self.names.len(),
                    identity,
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
pub(super) mod internals {
    use common::FileIdentity;

    use crate::exec::cache::stamp_job::FsPathId;

    impl FsPathId {
        /// A file identity without a filesystem behind it — no test controls a real file's mtime.
        pub(crate) fn file(len: u64, mtime_ns: i128) -> Self {
            Self::File(FileIdentity {
                len,
                mtime_ns,
                file_id: None,
                changed_ns: None,
            })
        }
    }
}

#[cfg(test)]
mod tests;
