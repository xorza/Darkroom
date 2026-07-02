//! The unified output cache: persist a node's outputs to a file and hydrate them back
//! so the executor reuses them instead of recomputing. Driven entirely at *execution*
//! time — the planner is structural and knows nothing about the cache. Two policies over
//! the one storage primitive ([`blob`]), differing only in [`OutputCache::target`] (how
//! a node maps to a file) and whether a present blob is rewritten:
//!
//! - **content-addressed** — a `persist` node's outputs at `<disk_root>/<hex(digest)>`,
//!   keyed by its content digest. Reproducible: any upstream change re-keys it (so
//!   it's auto-invalidated), and identical computations dedup across nodes/machines.
//! - **explicit-path** — a [`CachePassthrough`](crate::special::SpecialNode) node's
//!   outputs at the `Const` `FsPath` in `input[1]`. The path *is* the key; the user
//!   manages invalidation (delete the file, or the `bypass` toggle). See
//!   `README.md` Part C.
//!
//! **Lifecycle (all execution-time).** As the executor reaches each node (producer
//! first) and computes its digest:
//! 1. [`mark_on_disk_if_present`](OutputCache::mark_on_disk_if_present): if a decodable
//!    blob exists for that digest, flag the slot [`ValueCache::OnDisk`] and reuse the
//!    node without running it — a cheap `stat`, no read.
//! 2. [`hydrate_slot`](OutputCache::hydrate_slot): a running consumer reading a bound
//!    input pulls the producer's blob into RAM on demand (`collect_inputs`), so a
//!    disk-cached value behind another reused node never enters RAM. A failed read
//!    deletes the bad blob and drops the consumer for this run (it recomputes next
//!    reopen).
//! 3. [`store_node`](OutputCache::store_node): the executor calls this the moment a node
//!    finishes, so its blob is durable before the next node runs (a later failure or
//!    cancel can't lose the earlier nodes' caches).
//! 4. after the run → [`evict_unused`](OutputCache::evict_unused): drop RAM copies the
//!    run's *executed* nodes didn't produce or read but disk can serve again — lossless,
//!    because the per-node stores already put this run's values on disk.
//!
//! [`hydrate_for_inspection`](OutputCache::hydrate_for_inspection) is the off-run
//! path: an editor query reads a node's value, loading its blob on demand.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::context::ContextManager;
use crate::data::DataType;
use crate::elements::cache_passthrough::cache_node_path;
use crate::execution::blob;
use crate::execution::cache::{Cache, ValueCache};
use crate::execution::digest::Digest;
use crate::execution::program::{ExecutionBinding, ExecutionProgram, NodeIdx};
use crate::library::Library;
use crate::special::SpecialNode;

/// Where a node's outputs cache, and how a present blob is treated on store.
#[derive(Debug)]
enum Target {
    /// `<disk_root>/<hex(digest)>` — content-addressed: a present blob is the same
    /// bytes, so the store skips it (avoiding a redundant, possibly costly serialize).
    Addressed(PathBuf),
    /// An explicit path from `input[1]` — the store always (over)writes (a
    /// bypass/miss run rewrites).
    Explicit(PathBuf),
}

impl Target {
    fn path(&self) -> &Path {
        match self {
            Target::Addressed(p) | Target::Explicit(p) => p,
        }
    }
}

/// Persists node outputs to files and hydrates them back. Holds a snapshot of the
/// [`Library`] (its type table is the source of custom-value codecs, used by both
/// policies) and the optional content-addressed store root. Default is an empty
/// library ⇒ no codecs, `persist` nodes memory-only; `CachePassthrough` nodes
/// always use their explicit path.
///
/// The snapshot is fine to hold across library swaps: codecs are registered once
/// at assembly and never change (only subgraphs grow), so a held snapshot keeps
/// every codec it will ever need.
#[derive(Debug, Default)]
pub struct OutputCache {
    library: Arc<Library>,
    /// Root of the content-addressed store; `None` ⇒ `persist` nodes memory-only.
    disk_root: Option<PathBuf>,
}

impl OutputCache {
    /// Build a cache over `library` (its type table supplies the custom-value
    /// codecs) and an optional content-addressed store `disk_root` (`None` ⇒
    /// `persist` nodes are memory-only; `CachePassthrough` nodes always use their
    /// explicit path).
    pub fn new(library: Arc<Library>, disk_root: Option<PathBuf>) -> Self {
        Self { library, disk_root }
    }

    /// The file node `idx` caches to, or `None` when it doesn't: a
    /// `CachePassthrough` with a `Const` path → explicit; a `persist` node with a
    /// disk root and a content digest → content-addressed; anything else → none.
    fn target(&self, program: &ExecutionProgram, idx: NodeIdx, cache: &Cache) -> Option<Target> {
        let e_node = &program.e_nodes[idx];
        if matches!(e_node.special, Some(SpecialNode::CachePassthrough { .. })) {
            return cache_node_path(program.node_inputs(e_node))
                .map(|p| Target::Explicit(PathBuf::from(p)));
        }
        if e_node.persist {
            let digest = cache.slots[idx].current_digest?;
            let mut buf = [0u8; 64];
            return Some(Target::Addressed(
                self.disk_root.as_ref()?.join(hex(&digest, &mut buf)),
            ));
        }
        None
    }

    /// The executor's per-node "reuse from disk?" check, run once a node's digest is
    /// computed: if a decodable blob exists on disk for that digest, flag the slot
    /// [`ValueCache::OnDisk`] (dropping any stale resident value produced under a
    /// superseded digest) and return `true` — the node is served without running. The
    /// bytes stay on disk; they load lazily only when a running consumer reads the value
    /// ([`Self::hydrate_slot`], driven from `collect_inputs`), so a disk-cached value
    /// behind another never enters RAM. A bypassed cache-passthrough always recomputes.
    /// Reads the node's resolved output types off the program's `output_types` pool, so
    /// only this cache's codec table is needed.
    pub(crate) fn mark_on_disk_if_present(
        &self,
        program: &ExecutionProgram,
        idx: NodeIdx,
        cache: &mut Cache,
    ) -> bool {
        if is_bypassed(program, idx) {
            return false;
        }
        let Some(target) = self.target(program, idx, cache) else {
            return false;
        };
        if self.outputs_decodable(program.node_output_types(&program.e_nodes[idx]))
            && target.path().exists()
        {
            cache.slots[idx].value = ValueCache::OnDisk;
            true
        } else {
            false
        }
    }

    /// Materialize node `idx` plus the producers feeding its inputs — the values an
    /// inspection of `idx` reads (its own outputs and its inputs' resolved values).
    /// Lets a disk-cached node no run touched still show its value when the editor
    /// selects it, without the run having eagerly loaded every blob.
    pub(crate) fn hydrate_for_inspection(
        &self,
        program: &ExecutionProgram,
        cache: &mut Cache,
        idx: NodeIdx,
    ) {
        self.hydrate_slot(program, cache, idx);
        let span = program.e_nodes[idx].inputs;
        for input in &program.inputs[span.range()] {
            if let ExecutionBinding::Bind(addr) = &input.binding {
                self.hydrate_slot(program, cache, addr.target_idx);
            }
        }
    }

    /// Deserialize node `idx`'s disk blob into its slot. Returns whether the slot is
    /// resident afterward: `true` if already resident or the read succeeded. On a read
    /// failure (codec gone, corrupt, an incompatible blob format, or deleted) the file is
    /// **deleted** and the `OnDisk` flag cleared, so the demanding consumer is dropped
    /// this run and the *next* reopen recomputes the node + rewrites a fresh blob —
    /// without the delete, `store_node`'s skip-if-exists would keep the broken file
    /// forever. Returns `false`. A blob can't be of the *wrong type* for a matching
    /// digest: the output signature is folded into the digest ([`digest::node_digest`]),
    /// so a redefined output re-keys rather than colliding.
    pub(crate) fn hydrate_slot(
        &self,
        program: &ExecutionProgram,
        cache: &mut Cache,
        idx: NodeIdx,
    ) -> bool {
        // A fresh value already in RAM needs no load. A *stale* resident value can't
        // reach here: `mark_on_disk_if_present` demotes "stale + blob on disk" to
        // `OnDisk` (dropping the stale value) before a consumer would read it.
        if cache.is_resident_hit(idx) {
            return true;
        }
        if !matches!(cache.slots[idx].value, ValueCache::OnDisk) {
            return false;
        }
        // The slot claimed an on-disk blob. Load it; on success it's resident.
        if let Some(digest) = cache.slots[idx].current_digest
            && let Some(target) = self.target(program, idx, cache)
        {
            if let Some(values) = blob::read(target.path(), &self.library) {
                cache.hydrate(idx, values, digest);
                return true;
            }
            // The blob didn't load — corrupt, an incompatible format, or vanished. Delete
            // it so the recompute that follows writes a fresh one (otherwise the broken
            // file lingers and `store_node` skips it as "already on disk", forever).
            let _ = std::fs::remove_file(target.path());
        }
        cache.slots[idx].clear_output();
        false
    }

    /// After a run, demote resident values the run neither executed nor read as a
    /// frontier input back to disk-only — reclaiming RAM that merely duplicates a
    /// disk blob. `keep[idx]` marks the nodes to retain — the run's executed nodes plus
    /// the producers they read (see [`Executor::protected_after_run`](crate::execution::executor::Executor::protected_after_run));
    /// any other resident value is a prior run's leftover, pruned behind a cache this
    /// run, so nothing read it. If its blob is still on disk it's dropped from RAM and
    /// re-marked [`ValueCache::OnDisk`], so a later run or an inspection reloads it.
    /// Lossless: a value with no blob (Memory-only, impure) is kept, so eviction never
    /// forces a recompute.
    pub(crate) fn evict_unused(
        &self,
        program: &ExecutionProgram,
        cache: &mut Cache,
        keep: &[bool],
    ) {
        for idx in program.node_indices() {
            if keep[idx.idx()] || cache.slots[idx].output_values().is_none() {
                continue;
            }
            // Reloadable iff a blob for the current digest is on disk — only then is
            // dropping the RAM copy lossless.
            let reloadable = self
                .target(program, idx, cache)
                .is_some_and(|target| target.path().exists());
            if reloadable {
                cache.slots[idx].value = ValueCache::OnDisk;
            }
        }
    }

    /// Whether every output of node `idx` could be decoded back from a blob: each
    /// `Custom` output type has a codec in this cache's library. `types` are the node's
    /// resolved output types off the program pool, so this predicts (without reading)
    /// whether `blob::read` would succeed — `mark_on_disk_if_present` never flags a node
    /// whose later on-demand load would fail and trip the executor's "value present"
    /// invariant. An unresolved type (`Null`) imposes no constraint.
    fn outputs_decodable(&self, types: &[DataType]) -> bool {
        types.iter().all(|ty| match ty {
            DataType::Custom(type_id) => self.library.codec(type_id).is_some(),
            _ => true,
        })
    }

    /// Write node `idx`'s freshly-computed outputs to its file the moment it finishes
    /// (the executor calls this right after a successful invoke), so a long run's
    /// earlier caches are durable even if a later node errors or the run is cancelled.
    /// The outputs are **borrowed**, not cloned — they may be large, and the write
    /// future captures only the value slice (which is `Sync`), never the whole
    /// (non-`Sync`) [`Cache`], so the borrow can safely cross the serialize await. A
    /// content-addressed blob already on disk is skipped (same digest ⇒ same bytes); an
    /// explicit path is always (over)written. Best-effort: a non-cacheable node or a
    /// value with no codec is skipped, any failure logged — caching never fails a run.
    ///
    /// Only writes a value that matches the node's *current* digest
    /// ([`Cache::is_resident_hit`]): a resident value produced under a now-superseded
    /// digest (an input changed since it ran) must not be written to the new digest's
    /// content-addressed path, which would serve stale bytes on a later run. In the run
    /// loop the just-stamped value is always a current hit; this guards the deferred
    /// [`store_resident_caches`](crate::execution::ExecutionEngine::store_resident_caches)
    /// flush, which runs after a recompile.
    pub(crate) fn store_node<'a>(
        &'a self,
        program: &ExecutionProgram,
        idx: NodeIdx,
        cache: &'a Cache,
        ctx: &'a mut ContextManager,
    ) -> impl Future<Output = ()> + 'a {
        let target = self.target(program, idx, cache);
        let outputs = cache
            .is_resident_hit(idx)
            .then(|| cache.slots[idx].output_values())
            .flatten();
        async move {
            let (Some(target), Some(outputs)) = (target, outputs) else {
                return;
            };
            // A content-addressed blob already on disk is the same bytes — skip.
            if matches!(&target, Target::Addressed(path) if path.exists()) {
                return;
            }
            if let Err(e) = blob::write(target.path(), outputs, &self.library, ctx).await {
                tracing::warn!(path = %target.path().display(), error = %e, "failed to write output cache");
            }
        }
    }
}

/// True for a bypassed cache-passthrough node — recompute + overwrite, so never
/// hydrate it.
fn is_bypassed(program: &ExecutionProgram, idx: NodeIdx) -> bool {
    matches!(
        program.e_nodes[idx].special,
        Some(SpecialNode::CachePassthrough { bypass: true })
    )
}

/// Lowercase hex of a digest into a caller-owned stack buffer (no heap) — the
/// 64-char content-addressed blob filename. The returned `&str` borrows `buf`, so the
/// caller keeps it alive until the path is built. Avoids a per-call `String`; the
/// remaining `PathBuf` for the `stat`/read is inherent and dwarfed by that syscall.
fn hex<'a>(digest: &Digest, buf: &'a mut [u8; 64]) -> &'a str {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for (i, &byte) in digest.0.iter().enumerate() {
        buf[2 * i] = HEX[(byte >> 4) as usize];
        buf[2 * i + 1] = HEX[(byte & 0x0f) as usize];
    }
    std::str::from_utf8(buf).expect("hex digits are ASCII")
}

#[cfg(test)]
mod tests;
