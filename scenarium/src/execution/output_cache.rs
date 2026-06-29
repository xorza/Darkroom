//! The unified output cache: persist a node's outputs to a file and hydrate them
//! back so the planner prunes the node's recompute. Loading is two-staged — a cheap
//! `update`-time availability flag ([`OutputCache::mark_available`], a `stat` + codec
//! check, no bytes) that lets the planner prune, then an on-demand read of only the
//! values a run or an inspection touches ([`OutputCache::hydrate_frontier`]) — so a
//! disk-cached value behind another never enters RAM. Two policies over the one
//! storage primitive ([`blob`]), sharing the availability/load path and one store loop:
//!
//! - **content-addressed** — a `persist` node's outputs at `<disk_root>/<hex(digest)>`,
//!   keyed by its content digest. Reproducible: any upstream change re-keys it (so
//!   it's auto-invalidated), and identical computations dedup across nodes/machines.
//! - **explicit-path** — a [`CachePassthrough`](crate::special::SpecialNode) node's
//!   outputs at the `Const` `FsPath` in `input[1]`. The path *is* the key; the user
//!   manages invalidation (delete the file, or the `bypass` toggle). See
//!   `README.md` Part C.
//!
//! The two differ only in [`OutputCache::target`] (how a node maps to a file) and
//! whether a present blob is rewritten — everything else is shared.
//!
//! **Lifecycle (the call order is the contract).** The engine drives these in a
//! fixed sequence; each step depends on the previous:
//! 1. `update` → [`mark_available`](OutputCache::mark_available): flag on-disk blobs
//!    (no read), so the next plan can prune them.
//! 2. `execute`, per attempt → plan, then
//!    [`hydrate_frontier`](OutputCache::hydrate_frontier): read the frontier the
//!    schedule consumes. A failed read clears that flag and returns `false`, so the
//!    engine re-plans (the failed node then recomputes) — this is why hydration runs
//!    *after* planning and the two loop until it succeeds.
//! 3. run → [`store_node`](OutputCache::store_node): the executor calls this for each
//!    node the moment it finishes, so its blob is durable before the next node runs (a
//!    later failure or cancel can't lose the earlier nodes' caches).
//! 4. after the run → [`evict_unused`](OutputCache::evict_unused): drop RAM copies the
//!    run didn't touch but disk can serve again — lossless, because the per-node stores
//!    already put this run's values on disk.
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
use crate::execution::plan::ExecutionPlan;
use crate::execution::program::{ExecutionBinding, ExecutionProgram, NodeColumn, NodeIdx};
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
    /// Reused "executed or read as frontier" bitset for [`Self::evict_unused`],
    /// reset per run so eviction doesn't allocate a node-sized `Vec` each time.
    protected: NodeColumn<bool>,
}

impl OutputCache {
    /// Build a cache over `library` (its type table supplies the custom-value
    /// codecs) and an optional content-addressed store `disk_root` (`None` ⇒
    /// `persist` nodes are memory-only; `CachePassthrough` nodes always use their
    /// explicit path).
    pub fn new(library: Arc<Library>, disk_root: Option<PathBuf>) -> Self {
        Self {
            library,
            disk_root,
            protected: NodeColumn::default(),
        }
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
            let digest = cache.current_digest(idx)?;
            let mut buf = [0u8; 64];
            return Some(Target::Addressed(
                self.disk_root.as_ref()?.join(hex(&digest, &mut buf)),
            ));
        }
        None
    }

    /// `update` pass: set each cacheable node whose blob is present on disk and
    /// decodable for the current digest to [`ValueCache::OnDisk`] — **without reading
    /// it**. The planner prunes such a node's cone like any cache hit, but the bytes
    /// are deserialized only on demand (the execution frontier via
    /// [`Self::hydrate_frontier`], or an inspection read), so a disk-cached value
    /// sitting behind another disk-cached value never enters RAM. A bypassed cache
    /// node, a resident hit, a node with no current digest, or one whose outputs lack
    /// a registered codec is left alone (it recomputes). Reads the node's resolved
    /// output types off the program's `output_types` pool, so no library is needed
    /// here for resolution — only this cache's own codec table.
    ///
    /// Marking `OnDisk` *drops* any stale resident value the slot held: a value
    /// produced under a now-superseded digest can't mask the fresh blob at hydrate
    /// time (the bug the old three-field shape allowed).
    pub(crate) fn mark_available(&self, program: &ExecutionProgram, cache: &mut Cache) {
        for idx in program.node_indices() {
            // Re-evaluate the on-disk claim from scratch each update: drop a stale one
            // (the blob has since gone), but never a resident value.
            if matches!(cache.slots[idx].value, ValueCache::OnDisk) {
                cache.slots[idx].clear_output();
            }
            if is_bypassed(program, idx) || cache.is_resident_hit(idx) {
                continue;
            }
            if cache.current_digest(idx).is_none() {
                continue;
            }
            let Some(target) = self.target(program, idx, cache) else {
                continue;
            };
            if self.outputs_decodable(program.node_output_types(&program.e_nodes[idx]))
                && target.path().exists()
            {
                cache.slots[idx].value = ValueCache::OnDisk;
            }
        }
    }

    /// Read into RAM every disk-cached value an executing node will actually
    /// consume: walk `execute_order`, and for each `Bind` input whose producer the
    /// planner serves from cache, deserialize its blob now. Producers *behind* a
    /// pruned producer are never referenced here, so a disk-cached chain loads only
    /// its frontier.
    ///
    /// Returns whether every such producer is now resident. A blob that fails to
    /// load (corrupt, deleted, undecodable) has its `disk_available` cleared by
    /// [`Self::hydrate_slot`] and makes this return `false`, so the caller re-plans:
    /// the cleared node is then scheduled to recompute rather than pruned behind a
    /// value that isn't there (which would trip the executor's "value present"
    /// invariant). Each failure clears one flag, so the re-plan loop converges.
    pub(crate) fn hydrate_frontier(
        &self,
        program: &ExecutionProgram,
        plan: &ExecutionPlan,
        cache: &mut Cache,
    ) -> bool {
        let mut all_loaded = true;
        for &e_idx in &plan.execute_order {
            let span = program.e_nodes[e_idx].inputs;
            for input in &program.inputs[span.range()] {
                if let ExecutionBinding::Bind(addr) = &input.binding {
                    // Only a producer the planner serves from cache needs loading; a
                    // producer that will execute fills its own slot during the run.
                    if plan.verdicts[addr.target_idx].is_cached()
                        && !self.hydrate_slot(program, cache, addr.target_idx)
                    {
                        all_loaded = false;
                    }
                }
            }
        }
        all_loaded
    }

    /// Materialize node `idx` plus the producers feeding its inputs — the values an
    /// inspection of `idx` reads (its own outputs and its inputs' resolved values).
    /// Lets a disk-cached node no run touched still show its value when the editor
    /// selects it, without `mark_available` having eagerly loaded every blob.
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
    /// failure (codec gone, corrupt or deleted blob) the stale `disk_available` flag is
    /// cleared, so a re-plan recomputes the node instead of pruning it behind a value
    /// that can't be loaded — and `false` is returned. A blob can't be of the *wrong
    /// type* for a matching digest: the output signature is folded into the digest
    /// ([`Cache::recompute_digests`](crate::execution::cache::Cache::recompute_digests)),
    /// so a redefined output re-keys rather than colliding.
    fn hydrate_slot(&self, program: &ExecutionProgram, cache: &mut Cache, idx: NodeIdx) -> bool {
        // A fresh value already in RAM needs no load. A *stale* resident value can't
        // reach here: `mark_available` demotes "stale + blob on disk" to `OnDisk`, and
        // "stale, no blob" isn't pruned (so it's never a cached frontier producer).
        if cache.is_resident_hit(idx) {
            return true;
        }
        if !matches!(cache.slots[idx].value, ValueCache::OnDisk) {
            return false;
        }
        // The slot claimed an on-disk blob, so any path that fails to load it now is a
        // stale claim: drop it (`false` ⇒ the caller re-plans to recompute the node).
        if let Some(digest) = cache.current_digest(idx)
            && let Some(target) = self.target(program, idx, cache)
            && let Some(values) = blob::read(target.path(), &self.library)
        {
            cache.hydrate(idx, values, digest);
            return true;
        }
        cache.slots[idx].clear_output();
        false
    }

    /// After a run, demote resident values the run neither executed nor read as a
    /// frontier input back to disk-only — reclaiming RAM that merely duplicates a
    /// disk blob. Such a value is a prior run's leftover, pruned behind a cache this
    /// run, so nothing read it; if its blob is still on disk it's dropped from RAM and
    /// re-marked [`ValueCache::OnDisk`], so a later run or an inspection reloads it.
    /// Lossless: a value with no blob (Memory-only, impure) is kept, so eviction never
    /// forces a recompute.
    pub(crate) fn evict_unused(
        &mut self,
        program: &ExecutionProgram,
        plan: &ExecutionPlan,
        cache: &mut Cache,
    ) {
        // Protected = nodes this run executed, plus the cached producers it read as
        // frontier inputs. Any other resident value is an untouched prior-run leftover.
        // The bitset is a reused scratch column, reset to the node count each run.
        self.protected.reset(program.e_nodes.len(), false);
        for &e_idx in &plan.execute_order {
            self.protected[e_idx] = true;
            let span = program.e_nodes[e_idx].inputs;
            for input in &program.inputs[span.range()] {
                if let ExecutionBinding::Bind(addr) = &input.binding {
                    self.protected[addr.target_idx] = true;
                }
            }
        }
        for idx in program.node_indices() {
            if self.protected[idx] || cache.slots[idx].output_values().is_none() {
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
    /// `Custom` output type has a codec in this cache's library. `types` are the
    /// node's resolved output types off its slot, so this predicts (without reading)
    /// whether `blob::read` would succeed — `mark_available` never flags a node whose
    /// later frontier load would fail and trip the executor's "value present"
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
    pub(crate) fn store_node<'a>(
        &'a self,
        program: &ExecutionProgram,
        idx: NodeIdx,
        cache: &'a Cache,
        ctx: &'a mut ContextManager,
    ) -> impl Future<Output = ()> + 'a {
        let target = self.target(program, idx, cache);
        let outputs = cache.output_values(idx);
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
mod tests {
    use super::*;

    #[test]
    fn hex_is_64_lowercase_chars() {
        let mut digest = [0u8; 32];
        digest[0] = 0xab;
        digest[31] = 0x0f;
        let mut buf = [0u8; 64];
        let h = hex(&Digest(digest), &mut buf);
        assert_eq!(h.len(), 64);
        assert!(h.starts_with("ab"));
        assert!(h.ends_with("0f"));
        assert!(
            h.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }
}
