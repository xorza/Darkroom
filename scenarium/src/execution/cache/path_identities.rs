//! [`PathIdentities`]: what each filesystem path a run reads *was*, identified
//! once per run and folded by the digest.

use std::mem;

use common::CancelToken;
use hashbrown::HashMap;
use tokio::task;

use crate::execution::cache::digest::{DigestHasher, InputTag};
use crate::execution::cache::stamp_job::error::StampError;
use crate::execution::cache::stamp_job::{FsPathId, StampJob};

/// The run's memo of path identities, the previous run's keys, and the
/// off-thread walk that fills the memo.
///
/// Held by the [`RuntimeCache`](crate::execution::cache::runtime_cache::RuntimeCache),
/// which decides which paths a node reads — a const's, or a value a producer
/// left in its slot — and folds what this hands back.
#[derive(Debug, Default)]
pub(super) struct PathIdentities {
    /// What each path this run reads was, identified once.
    current: HashMap<String, FsPathId>,
    /// The previous run's memo, kept for its keys: a path read again moves its
    /// string from here to the queue, so a run over the paths of the last
    /// allocates none. What no run asked for again is dropped a run later, so
    /// the two hold at most two runs' paths.
    previous: HashMap<String, FsPathId>,
    /// The off-thread walk that fills `current`: queue, then pass. It owns
    /// only what crosses to the blocking pool.
    job: StampJob,
}

impl PathIdentities {
    pub(super) fn clear(&mut self) {
        self.current.clear();
        self.previous.clear();
        self.job.clear_queue();
    }

    /// Start a run's memo afresh, keeping the last run's keys for reuse.
    pub(super) fn begin_run(&mut self) {
        mem::swap(&mut self.current, &mut self.previous);
        self.current.clear();
        self.job.clear_queue();
    }

    /// Queue `path` for the next walk, unless this run already identified it,
    /// already queued it, or it is [unset](is_unset_path) — which has nothing
    /// to walk, and queueing it would fail the whole pass on a `metadata("")`,
    /// costing every other node in the batch its pre-run identity.
    pub(super) fn request(&mut self, path: &str) {
        if is_unset_path(path) || self.current.contains_key(path) || self.job.is_requested(path) {
            return;
        }
        let key = self
            .previous
            .remove_entry(path)
            .map_or_else(|| path.to_owned(), |(key, _)| key);
        self.job.request(key);
    }

    /// Run the queued pass on the blocking pool, leaving the queue empty and
    /// what it identified in the memo.
    ///
    /// The job goes out and comes back — it owns its queue and scratch, so
    /// nothing is borrowed across the boundary and the memo never moves at all.
    /// The memo takes what the pass stamped whatever its verdict, and the pass
    /// walks past a path that will not read ([`StampJob::run`]) — so one
    /// unreadable path costs its own node's digest and none of the identities
    /// the same pass gathered for the rest of the run.
    pub(super) async fn walk_queued(&mut self, cancel: CancelToken) -> Result<(), StampError> {
        if !self.job.is_queued() {
            return Ok(());
        }
        let mut job = mem::take(&mut self.job);
        let (job, resolved) = task::spawn_blocking(move || {
            let resolved = job.run(&cancel);
            (job, resolved)
        })
        .await
        .expect("resource stamping task panicked");
        self.job = job;
        self.current.extend(self.job.drain_stamped());
        resolved
    }

    /// Fold the identities behind the paths a value names.
    ///
    /// `None` is the failure: a path this run never stamped, leaving its node
    /// without a sound cache key. A value naming *no* paths does not call at
    /// all — that is a plain const, not a filesystem read.
    ///
    /// An [unset](is_unset_path) slot is neither. It names nothing to stat, so
    /// it folds a marker and leaves the node cacheable — "no file chosen yet"
    /// is a state the author passes through, not a resource that failed. The
    /// marker stands in the slot rather than being skipped: on the Bind side
    /// the authored strings are not folded alongside, so skipping would let
    /// `["", p]` and `[p, ""]` key alike.
    pub(super) fn hash(&self, hasher: &mut DigestHasher, paths: &[String]) -> Option<()> {
        hasher.write_pod(paths.len() as u64);
        for path in paths {
            if is_unset_path(path) {
                hasher.write_input_tag(InputTag::UnsetPath);
                continue;
            }
            self.current.get(path)?.hash(hasher);
        }
        Some(())
    }
}

/// Whether a path string names nothing — the state a path port sits in before
/// a file is chosen, whose placeholder is
/// [`DataType::default_value`](crate::data::data_type::DataType::default_value)'s
/// empty string.
///
/// Blank after trimming, not merely empty: whitespace is what a typed-in path
/// collects, and no such string names a file anyone meant. The path itself is
/// never trimmed — spaces are legal at either end of a filename, so rewriting
/// one would read a file other than the authored one.
fn is_unset_path(path: &str) -> bool {
    path.trim().is_empty()
}

#[cfg(test)]
pub(crate) mod internals {
    use common::CancelToken;

    use crate::execution::cache::path_identities::PathIdentities;
    use crate::execution::cache::stamp_job::FsPathId;
    use crate::execution::cache::stamp_job::error::StampError;

    impl PathIdentities {
        /// [`PathIdentities::walk_queued`] on this thread, without the
        /// blocking pool a test has no runtime to reach.
        pub(crate) fn walk_queued_blocking(
            &mut self,
            cancel: &CancelToken,
        ) -> Result<(), StampError> {
            let resolved = self.job.run(cancel);
            self.current.extend(self.job.drain_stamped());
            resolved
        }

        /// The key the run's memo holds `path` under, for a test about whose
        /// allocation it is.
        pub(crate) fn key(&self, path: &str) -> Option<&String> {
            self.current.get_key_value(path).map(|(key, _)| key)
        }

        /// Re-key `path` with at least `capacity` bytes reserved — a key no
        /// copy of the path would have, so a test can tell it apart.
        pub(crate) fn widen_key(&mut self, path: &str, capacity: usize) {
            let (mut key, identity) = self.current.remove_entry(path).unwrap();
            key.reserve_exact(capacity);
            self.current.insert(key, identity);
        }

        /// Plant a file identity without touching a filesystem, so a digest
        /// folding a path can be pinned to a constant.
        pub(crate) fn plant_file(&mut self, path: &str, len: u64, mtime_ns: i128) {
            self.current
                .insert(path.to_string(), FsPathId::file(len, mtime_ns));
        }
    }
}
