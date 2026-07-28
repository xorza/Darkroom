//! Per-run filesystem identity collection.
//!
//! Filesystem metadata walks run on Tokio's blocking pool. The resulting
//! [`RunResourceStamps`] is shared by the producer-first digest pass and late bound-path
//! restamps, so each path is observed once per run and digest folding itself performs no I/O.

use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};

use common::CancelToken;
use hashbrown::{HashMap, HashSet};

use crate::StaticValue;
use crate::execution::cache::runtime::RuntimeCache;
use crate::execution::digest::DigestHasher;
use crate::execution::plan::ExecutionPlan;
use crate::execution::program::index::NodeIdx;
use crate::execution::program::{ExecutionBinding, ExecutionProgram};
use crate::node::definition::FuncBehavior;

/// Metadata identity of one filesystem entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileId {
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

/// One entry anywhere beneath a stamped directory root.
#[derive(Debug)]
struct DirectoryEntryId {
    /// Path relative to the stamped root, kept as **raw bytes**.
    ///
    /// `to_string_lossy` maps every non-UTF-8 name onto the same
    /// replacement text, so on any filesystem that admits arbitrary byte
    /// names a rename between two such names left the directory's
    /// identity — and so a pure node's cache key — exactly where it was.
    rel: PathBuf,
    /// A path swapping between a file and a directory is a different
    /// thing to read, even where length and mtime happen to agree.
    is_dir: bool,
    file: FileId,
}

#[derive(Debug)]
enum FsPathId {
    File(FileId),
    /// Every entry beneath the root, sorted by relative path.
    Directory(Vec<DirectoryEntryId>),
    Missing,
}

/// Why a path has no identity.
///
/// Both outcomes live here rather than among [`FsPathId`]'s variants. An
/// error dressed as a value is exactly what let "I could not see this"
/// fold into a digest as if it were an identity — and a stable one, so
/// the node kept reusing a cached result while what it could not see
/// changed underneath it.
#[derive(Debug, thiserror::Error)]
pub(crate) enum StampError {
    /// The path would not read: a directory that would not list, an entry
    /// that would not stat, a file with no modification time.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// The run was cancelled mid-walk.
    #[error("the run was cancelled")]
    Cancelled,
}

impl FsPathId {
    fn collect(path: &str, cancel: &CancelToken) -> Result<Self, StampError> {
        if cancel.is_cancelled() {
            return Err(StampError::Cancelled);
        }
        match std::fs::metadata(path) {
            Ok(metadata) if metadata.is_dir() => Self::collect_directory(Path::new(path), cancel),
            Ok(metadata) => Ok(Self::File(FileId::from_metadata(&metadata)?)),
            // A path that is not there is a determinate fact, and a common
            // one — "no such file" is something a node reads reproducibly.
            // Any *other* error means the answer is unknown rather than
            // absent; folding them all into `Missing` let an unreadable
            // path cache exactly as though it had been deleted.
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::Missing),
            Err(error) => Err(error.into()),
        }
    }

    /// Walk **the whole subtree** under `root`, recording every entry
    /// beneath it rather than only its immediate children: a pure
    /// function handed a directory consumes it recursively, so the
    /// recursive contents are what its output is a function of. Stamping
    /// one level deep let `root/sub/file` change freely while `root`'s
    /// fingerprint — and every cache key folding it — stood still.
    ///
    /// **Symlinks are stamped, never followed.** `DirEntry::metadata`
    /// does not traverse them, which both terminates the walk (a link
    /// pointing back up the tree would otherwise never stop) and still
    /// moves the identity when a link is repointed.
    ///
    /// Anything unreadable propagates: a subtree seen only in part must
    /// not hash like one seen whole.
    ///
    /// Iterative, because the depth comes from a user-chosen directory
    /// and a deep tree must not decide the stack size.
    fn collect_directory(root: &Path, cancel: &CancelToken) -> Result<Self, StampError> {
        let mut entries: Vec<DirectoryEntryId> = Vec::new();
        let mut pending = vec![PathBuf::new()];
        while let Some(rel_dir) = pending.pop() {
            for entry in std::fs::read_dir(root.join(&rel_dir))? {
                if cancel.is_cancelled() {
                    return Err(StampError::Cancelled);
                }
                let entry = entry_id(entry, &rel_dir)?;
                if entry.is_dir {
                    pending.push(entry.rel.clone());
                }
                entries.push(entry);
            }
        }
        entries.sort_by(|left, right| left.rel.cmp(&right.rel));
        Ok(Self::Directory(entries))
    }

    fn hash(&self, hasher: &mut DigestHasher) {
        match self {
            Self::File(file) => {
                hasher.write_bytes(&[0]);
                file.hash(hasher);
            }
            Self::Directory(entries) => {
                hasher.write_bytes(&[1]).write_pod(entries.len() as u64);
                for entry in entries {
                    hasher
                        .write_len_prefixed(entry.rel.as_os_str().as_encoded_bytes())
                        .write_pod(entry.is_dir);
                    entry.file.hash(hasher);
                }
            }
            Self::Missing => {
                hasher.write_bytes(&[2]);
            }
        }
    }
}

/// One directory entry's identity, or the first I/O error that stopped it
/// — the whole per-entry sequence in one `?` chain, so the walk has a
/// single failure branch rather than one per call.
fn entry_id(entry: io::Result<std::fs::DirEntry>, rel_dir: &Path) -> io::Result<DirectoryEntryId> {
    let entry = entry?;
    let metadata = entry.metadata()?;
    Ok(DirectoryEntryId {
        rel: rel_dir.join(entry.file_name()),
        is_dir: metadata.is_dir(),
        file: FileId::from_metadata(&metadata)?,
    })
}

#[derive(Debug, Default)]
pub(crate) struct RunResourceStamps {
    fs_paths: HashMap<String, FsPathId>,
    /// Paths queued for the next stamping pass, deduplicated.
    ///
    /// Retained rather than built per pass: it and `fs_paths` both travel
    /// to the blocking pool and back, so a run reuses one set and one map
    /// instead of allocating a fresh map per pass and copying it in.
    requests: HashSet<String>,
}

impl RunResourceStamps {
    /// Queue whichever of `paths` this run has not stamped already.
    fn request_fs_paths(&mut self, paths: &[String]) {
        for path in paths {
            if !self.fs_paths.contains_key(path) {
                self.requests.insert(path.clone());
            }
        }
    }

    fn request_node_paths(
        &mut self,
        program: &ExecutionProgram,
        cache: &RuntimeCache,
        node_idx: NodeIdx,
    ) {
        let e_node = &program[node_idx];
        if e_node.behavior != FuncBehavior::Pure {
            return;
        }
        for input in &program.inputs[e_node.inputs] {
            match &input.binding {
                ExecutionBinding::Const(StaticValue::FsPath(path)) => {
                    self.request_fs_paths(std::slice::from_ref(path));
                }
                ExecutionBinding::Const(StaticValue::FsPaths(paths)) => {
                    self.request_fs_paths(paths);
                }
                ExecutionBinding::Bind(address) if input.stamps_fs_path => {
                    let Some(value) = cache.slots[address.node_idx]
                        .output_values()
                        .and_then(|values| values.get(address.port_idx as usize))
                    else {
                        continue;
                    };
                    match value.as_static() {
                        Some(StaticValue::FsPath(path)) => {
                            self.request_fs_paths(std::slice::from_ref(path));
                        }
                        Some(StaticValue::FsPaths(paths)) => {
                            self.request_fs_paths(paths);
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }

    /// Batch every executing node's paths into one off-thread pass.
    ///
    /// **A prefetch, not a decision.** A path this fails to stamp simply
    /// does not land, which leaves its node's digest `None` — and a node
    /// with no digest is re-stamped at its own turn by
    /// [`Self::prepare_node`], where the failure belongs to exactly one
    /// node and is reported as that node's. So nothing is lost here but
    /// the batching, and no failure is decided at a point that cannot say
    /// which node it concerns.
    ///
    /// Request collection finishes before await so the non-`Sync` cache borrow stays synchronous.
    pub(crate) fn prepare_run<'a>(
        &'a mut self,
        program: &ExecutionProgram,
        plan: &ExecutionPlan,
        cache: &RuntimeCache,
        cancel: CancelToken,
    ) -> impl Future<Output = ()> + 'a {
        self.fs_paths.clear();
        self.requests.clear();
        for &node_idx in &plan.process_order {
            if plan.verdicts[node_idx].wants_execute() {
                self.request_node_paths(program, cache, node_idx);
            }
        }
        async move {
            let _ = self.prepare(cancel).await;
        }
    }

    /// Stamp one node's paths, authoritatively.
    ///
    /// Every path here is declared by `node_idx` alone, so a failure is
    /// that node's — the caller fails it and the ordinary errored-upstream
    /// cascade takes its dependents, rather than a run-wide abort blaming
    /// nobody.
    pub(crate) fn prepare_node<'a>(
        &'a mut self,
        program: &ExecutionProgram,
        cache: &RuntimeCache,
        node_idx: NodeIdx,
        cancel: CancelToken,
    ) -> impl Future<Output = Result<(), StampError>> + 'a {
        self.requests.clear();
        self.request_node_paths(program, cache, node_idx);
        self.prepare(cancel)
    }

    async fn prepare(&mut self, cancel: CancelToken) -> Result<(), StampError> {
        if self.requests.is_empty() {
            return Ok(());
        }
        // Both buffers ride to the blocking pool inside `self` and come
        // back, so the run keeps one allocation of each. A failure ends
        // the run, so the error path has no need of them.
        let mut moved = std::mem::take(self);
        let worker_cancel = cancel.clone();
        *self = tokio::task::spawn_blocking(move || moved.resolve(&worker_cancel).map(|()| moved))
            .await
            .expect("resource stamping task panicked")?;
        Ok(())
    }

    /// Stamp every queued path, draining the queue. The first path with no
    /// determinate identity stops the pass and takes its error with it —
    /// a node whose inputs cannot be identified has no sound cache key,
    /// and continuing would mean silently never caching it again.
    fn resolve(&mut self, cancel: &CancelToken) -> Result<(), StampError> {
        for path in self.requests.drain() {
            let identity = FsPathId::collect(&path, cancel)?;
            self.fs_paths.insert(path, identity);
        }
        Ok(())
    }

    pub(crate) fn hash_fs_paths(&self, hasher: &mut DigestHasher, paths: &[String]) -> Option<()> {
        hasher.write_pod(paths.len() as u64);
        for path in paths {
            self.fs_paths.get(path)?.hash(hasher);
        }
        Some(())
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use common::CancelToken;

    use crate::execution::cache::runtime::RuntimeCache;
    use crate::execution::program::ExecutionProgram;
    use crate::execution::program::index::NodeIdx;

    use crate::execution::resource::RunResourceStamps;

    /// The synchronous twin of `RunResourceStamps::prepare_node`, for
    /// tests with no runtime to hand a blocking task to.
    pub(crate) fn prepare_node(
        stamps: &mut RunResourceStamps,
        program: &ExecutionProgram,
        cache: &RuntimeCache,
        node_idx: NodeIdx,
    ) {
        stamps.requests.clear();
        stamps.request_node_paths(program, cache, node_idx);
        stamps
            .resolve(&CancelToken::never())
            .expect("test fixtures stamp readable paths");
    }
}

#[cfg(test)]
mod tests;
