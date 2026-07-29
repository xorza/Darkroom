//! Per-run filesystem identity collection.
//!
//! Filesystem metadata walks run on Tokio's blocking pool. One
//! [`ResourceStamper`] serves the producer-first digest pass and the late bound-path
//! restamps alike, so each path is observed once per run and digest folding itself
//! performs no I/O.

use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};

use common::CancelToken;
use hashbrown::{HashMap, HashSet};

use crate::DynamicValue;
use crate::execution::cache::digest::{
    DOMAIN, Digest, DigestHasher, InputTag, hash_data_type, hash_static,
};
use crate::execution::cache::slot::RuntimeSlot;
use crate::execution::program::index::{NodeColumn, NodeIdx, OutputAddr};
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

#[derive(Clone, Copy, Debug)]
enum FsPathId {
    File(FileId),
    /// Every file beneath the root, folded in relative-path order.
    Directory(Digest),
}

impl FsPathId {
    fn hash(&self, hasher: &mut DigestHasher) {
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

/// Why a path has no identity.
///
/// Nothing here is an [`FsPathId`] variant, absence included. An error
/// dressed as a value is exactly what let "I could not see this" fold
/// into a digest as if it were an identity — and a stable one, so the
/// node kept reusing a cached result while what it could not see changed
/// underneath it. A path that is not there is the same answer reached by
/// another road: a node whose declared input does not exist has nothing
/// to be a function of, so it fails at its own turn with the path in the
/// message, rather than being handed an identity and invoked to discover
/// as much for itself.
#[derive(Debug, thiserror::Error)]
pub(crate) enum StampError {
    /// The path would not read: one that is not there, a directory that
    /// would not list, an entry that would not stat, a file with no
    /// modification time.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// The run was cancelled mid-walk.
    #[error("the run was cancelled")]
    Cancelled,
}

/// Every path this run has identified.
///
/// One stamper serves the whole run. Its memo stays here for the run's
/// life; only the [`StampJob`] below ever crosses a thread boundary.
#[derive(Debug, Default)]
pub(super) struct ResourceStamper {
    /// What each path was, identified once per run — what the digest fold
    /// reads.
    fs_paths: HashMap<String, FsPathId>,
    job: StampJob,
}

/// One stamping pass, and every buffer it works in.
///
/// Owned whole, because a blocking-pool task must own what it touches:
/// the job moves to the pool, runs, and moves back — so the walk needs no
/// borrow of the stamper, and the pass reuses its queue and scratch
/// instead of allocating them again. Nothing of a directory survives its
/// walk but the 32-byte digest.
#[derive(Debug, Default)]
struct StampJob {
    /// Paths queued for this pass, deduplicated — a path fifty nodes read
    /// is stamped once.
    requests: HashSet<String>,
    /// What the pass identified, drained into the stamper's memo when the
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
    /// Stamp every queued path, draining the queue. The first path with no
    /// determinate identity stops the pass and takes its error with it —
    /// a node whose inputs cannot be identified has no sound cache key,
    /// and continuing would mean silently never caching it again.
    fn run(&mut self, cancel: &CancelToken) -> Result<(), StampError> {
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

impl ResourceStamper {
    /// Queue whichever of `paths` this run has not stamped already.
    fn request_fs_paths(&mut self, paths: &[String]) {
        for path in paths {
            if !self.fs_paths.contains_key(path) {
                self.job.requests.insert(path.clone());
            }
        }
    }

    /// Clear the run's memo and queue — a fresh run identifies afresh.
    pub(super) fn reset(&mut self) {
        self.fs_paths.clear();
        self.job.requests.clear();
    }

    /// Identify every path `nodes` reads, in one off-thread pass.
    ///
    /// The whole protocol — queue, then walk — so no caller has to know
    /// the order or that the queue empties itself. Adds to the run's memo;
    /// [`Self::reset`] is what starts a fresh one.
    ///
    /// Takes the slot column rather than the whole cache: the cache owns
    /// this stamper, so the two are one `&mut` at every caller and only
    /// these slots are read here.
    /// Queueing finishes before the future exists, so neither `nodes` nor
    /// the program and slot borrows it reads are captured across the
    /// await — a future holding them would not be `Send`, and the worker
    /// spawns this one.
    pub(super) fn identify<'a>(
        &'a mut self,
        program: &ExecutionProgram,
        slots: &NodeColumn<RuntimeSlot>,
        nodes: impl IntoIterator<Item = NodeIdx>,
        cancel: CancelToken,
    ) -> impl Future<Output = Result<(), StampError>> + 'a {
        for node_idx in nodes {
            self.request_node_paths(program, slots, node_idx);
        }
        self.prepare(cancel)
    }

    /// Queue the paths `node_idx` reads, on top of whatever is already
    /// queued — [`Self::prepare`] is what empties the queue.
    fn request_node_paths(
        &mut self,
        program: &ExecutionProgram,
        slots: &NodeColumn<RuntimeSlot>,
        node_idx: NodeIdx,
    ) {
        let e_node = &program[node_idx];
        if e_node.behavior != FuncBehavior::Pure {
            return;
        }
        for input in &program.inputs[e_node.inputs] {
            let paths = match &input.binding {
                ExecutionBinding::Const(value) => value.as_fs_paths(),
                // Any resident value, not only a current one: this runs
                // before the run's digests are stamped, so the digest
                // fold's stricter accessor would see almost nothing here.
                // Over-requesting costs one walk; under-requesting costs a
                // node its pruning.
                ExecutionBinding::Bind(address) if input.stamps_fs_path => slots[address.node_idx]
                    .output_values()
                    .and_then(|values| values.get(address.port_idx as usize))
                    .and_then(DynamicValue::as_fs_paths),
                _ => None,
            };
            if let Some(paths) = paths {
                self.request_fs_paths(paths);
            }
        }
    }

    /// Run the queued pass on the blocking pool, leaving the queue empty.
    ///
    /// Empty on the failing path too: the walk drains as it goes, and the
    /// drain finishes whatever the first unreadable path left behind.
    ///
    /// The job goes out and comes back — it owns its queue and scratch, so
    /// nothing here is borrowed across the boundary and the stamper's memo
    /// never moves at all. It returns on the failing path too, which is
    /// what keeps a run that hits one unreadable path from re-walking
    /// every directory it had already identified.
    async fn prepare(&mut self, cancel: CancelToken) -> Result<(), StampError> {
        if self.job.requests.is_empty() {
            return Ok(());
        }
        let mut job = std::mem::take(&mut self.job);
        let (job, resolved) = tokio::task::spawn_blocking(move || {
            let resolved = job.run(&cancel);
            (job, resolved)
        })
        .await
        .expect("resource stamping task panicked");
        self.job = job;
        self.adopt_stamped();
        resolved
    }

    /// Take what the pass identified into the run's memo, leaving the
    /// job's buffer empty for the next one.
    fn adopt_stamped(&mut self) {
        self.fs_paths.extend(self.job.stamped.drain(..));
    }

    /// A node's **content digest** — the one content key it's cached under, folding its identity
    /// (func id + version + output types) plus its structural inputs. The single digest the whole
    /// cache keys on: RAM reuse ([`RuntimeCache::is_resident_hit`]), disk load/store, and downstream
    /// folding all read the node's stamped `current_digest`. Computed producer-first
    /// (topological), so a `Bind` producer's `current_digest` is already stamped when read.
    ///
    /// - An **`Impure`** node has no digest (`None`) — it varies per run, so it never caches and
    ///   always recomputes; a `Bind` producer with a `None` digest taints this node to `None`.
    /// - Otherwise fold every input structurally: a `Const`'s value + prepared `FsPath`
    ///   file/dir content, or a `Bind` producer's stamped `current_digest` — plus, for a
    ///   resource-typed input, the live identity of the referent behind the *delivered* value
    ///   ([`ResourceStamper::hash_bound_fs_path`]). That last fold needs the producer's value:
    ///   unreadable ⇒ `None`, and the run loop re-stamps such a node at reach time, once its
    ///   producers settled.
    ///
    /// A method on the stamper because every external identity it folds comes from there;
    /// the encoding itself stays in this module, beside the helpers and the [`DOMAIN`] that
    /// versions it.
    pub(super) fn node_digest(
        &self,
        program: &ExecutionProgram,
        node_idx: NodeIdx,
        slots: &NodeColumn<RuntimeSlot>,
    ) -> Option<Digest> {
        let e_node = &program[node_idx];

        // Only a `Pure` node is content-cacheable; an `Impure` node varies per run, so it has no
        // digest and always recomputes.
        if e_node.behavior != FuncBehavior::Pure {
            return None;
        }

        let mut hasher = DigestHasher::new();
        hasher
            .write_bytes(DOMAIN)
            .write_pod(e_node.func_id.as_u128())
            .write_pod(e_node.version);

        let outputs = &program.outputs[e_node.outputs];
        hasher.write_pod(outputs.len() as u64);
        for output in outputs {
            hash_data_type(&mut hasher, &output.data_type);
        }

        for input in &program.inputs[e_node.inputs] {
            match &input.binding {
                ExecutionBinding::None => {
                    hasher.write_input_tag(InputTag::Unbound);
                }
                ExecutionBinding::Const(value) => {
                    hasher.write_input_tag(InputTag::Const);
                    hash_static(&mut hasher, value);
                    self.hash_fs_paths(&mut hasher, value.as_fs_paths())?;
                }
                ExecutionBinding::Bind(addr) => {
                    // The producer was visited first (topological order), so its `current_digest`
                    // is set; a `None` taints this node.
                    let producer = slots[addr.node_idx].current_digest?;
                    // Producer digest then port index, both fixed-width, so two
                    // consumers reading different ports of one node fold apart.
                    hasher
                        .write_input_tag(InputTag::Bind)
                        .write_digest(&producer)
                        .write_pod(addr.port_idx);
                    // A resource-typed input dereferences the delivered reference, so the
                    // external state behind the *runtime value* is part of this node's key —
                    // the Bind-side counterpart of the `Const` arm's fold. Needs the
                    // producer's value; unreadable (pre-run) ⇒ `None`, re-stamped at reach
                    // time by the run loop.
                    if input.stamps_fs_path {
                        self.hash_bound_fs_path(&mut hasher, slots, addr)?;
                    }
                }
            }
        }
        Some(hasher.finish())
    }

    /// Fold the referent identity behind a **Bind-delivered** resource input: read the
    /// delivered value off the producer's resident slot and fold its prepared file/directory
    /// identity, so a wired path re-keys its consumer exactly like a const path does. The
    /// producer's value must exist first: an unreadable value (producer not resident) is
    /// `None`, tainting the node's digest — the pre-run sweep stamps it "uncacheable, must
    /// run", and the run loop then re-stamps at reach time, when the producers have settled
    /// and any disk-backed path producer was hydrated (`executor.rs`). A mis-typed delivered
    /// value folds a distinct marker instead.
    pub(super) fn hash_bound_fs_path(
        &self,
        hasher: &mut DigestHasher,
        slots: &NodeColumn<RuntimeSlot>,
        addr: &OutputAddr,
    ) -> Option<()> {
        // `current_output_values`, so a value produced under an older digest
        // cannot deliver a reference into this key.
        let delivered = slots[addr.node_idx]
            .current_output_values()?
            .get(addr.port_idx as usize)?;
        match delivered.as_fs_paths() {
            Some(paths) => {
                hasher.write_input_tag(InputTag::BoundPaths);
                self.hash_fs_paths(hasher, Some(paths))?;
            }
            // A mis-typed wire — a resource input handed something that is
            // not a path — keys on its marker alone, and stays cacheable.
            None => {
                hasher.write_input_tag(InputTag::BoundMistyped);
            }
        }
        Some(())
    }

    /// Fold the identities behind the paths a value names.
    ///
    /// The two `Option`s mean opposite things. A value naming *no* paths
    /// folds nothing and succeeds — that is a plain const, not a
    /// filesystem read. Returning `None` is the failure: a path this run
    /// never stamped, leaving its node without a sound cache key.
    pub(super) fn hash_fs_paths(
        &self,
        hasher: &mut DigestHasher,
        paths: Option<&[String]>,
    ) -> Option<()> {
        let Some(paths) = paths else {
            return Some(());
        };
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

    use crate::execution::cache::resource::{FileId, FsPathId, ResourceStamper, StampError};
    use crate::execution::cache::slot::RuntimeSlot;
    use crate::execution::program::ExecutionProgram;
    use crate::execution::program::index::{NodeColumn, NodeIdx};

    impl ResourceStamper {
        /// [`ResourceStamper::identify`] on this thread — the same pass,
        /// without the blocking pool a test has no runtime to reach.
        pub(crate) fn identify_blocking(
            &mut self,
            program: &ExecutionProgram,
            slots: &NodeColumn<RuntimeSlot>,
            node_idx: NodeIdx,
        ) {
            self.request_node_paths(program, slots, node_idx);
            let _ = self.stamp_queued(&CancelToken::never());
        }

        pub(super) fn stamp_queued(&mut self, cancel: &CancelToken) -> Result<(), StampError> {
            let resolved = self.job.run(cancel);
            self.adopt_stamped();
            resolved
        }

        /// Plant a file identity for `path` without touching a
        /// filesystem, so a digest that folds a path can be pinned to a
        /// constant — no test controls a real file's mtime.
        pub(crate) fn stamp_file(&mut self, path: &str, len: u64, mtime_ns: i128) {
            self.fs_paths
                .insert(path.to_string(), FsPathId::File(FileId { len, mtime_ns }));
        }
    }
}

#[cfg(test)]
mod tests;
