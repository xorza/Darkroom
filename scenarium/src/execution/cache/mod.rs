//! The cross-run runtime cache: the per-node RAM slots (output values + content digests +
//! node state, keyed by `NodeId` and index-aligned to the program's `e_nodes`) **plus** the
//! [`DiskStore`] backing them, and the caching policy over the two — reuse detection, on-demand
//! hydration, persistence, and RAM eviction. Owned by the
//! [`ExecutionEngine`](crate::execution::ExecutionEngine); the executor's run loop drives it a
//! node at a time. The [`DiskStore`] is pure blob I/O and knows nothing of the cache; this type
//! reads a node's digest/value-state off its slot and the blob off disk, and pushes the result
//! back — so RAM eviction (demote-or-drop) lives here, on the cache that owns both stores.
//! Per-run results (errors, timings) are *not* here — they belong to a single run, not the cache.

use std::collections::HashSet;
use std::future::Future;
use std::sync::Arc;

use common::{KeyIndexKey, KeyIndexVec};

use crate::data::{DynamicValue, RamUsage};
use crate::execution::NodeColumn;
use crate::execution::digest::{Digest, node_digest};
use crate::execution::disk_store::{DiskStore, Target};
use crate::execution::program::{ExecutionBinding, ExecutionNode, ExecutionProgram, NodeIdx};
use crate::execution::resolve::Disposition;
use crate::graph::NodeId;
use crate::node::special::SpecialNode;
use crate::runtime::any_state::AnyState;
use crate::runtime::context::ContextManager;
use crate::runtime::shared_any_state::SharedAnyState;

/// One node's cached output as an explicit three-state machine. The states are mutually
/// exclusive, so the bad combinations — "resident *and* flagged on disk", "value present
/// but no digest tracked", a stale resident value masking a fresh disk blob — can't be
/// built. The node's *identity* digest is a separate axis
/// ([`RuntimeSlot::current_digest`]); this models only the value.
#[derive(Default, Debug)]
pub(crate) enum ValueState {
    /// No cached output — never produced, evicted, or cleared for re-execution.
    #[default]
    Empty,
    /// Values resident in RAM. `produced_under` is the digest they were computed
    /// under — `None` for an impure node, which holds a value but is never a hit.
    Resident {
        values: Vec<DynamicValue>,
        produced_under: Option<Digest>,
    },
    /// Not in RAM, but a decodable blob exists on disk for the slot's *current* digest —
    /// flagged during the run by [`mark_on_disk_if_present`](RuntimeCache::mark_on_disk_if_present)
    /// (or demoted here from a resident value by `evict_unused`) without loading,
    /// deserialized on demand (a running consumer's `collect_inputs`, or an inspection).
    /// Lets a disk-cached value behind another disk-cached value never enter RAM.
    OnDisk,
}

/// One node's cross-run runtime state, index-aligned to the program's `e_nodes`:
/// the [`value`](RuntimeSlot::value) cache and the node's persistent
/// `state`/`event_state`. Carries its own `id` so the cache reconciles by key on
/// `update` (surviving node reorder/trim).
#[derive(Default, Debug)]
pub(crate) struct RuntimeSlot {
    pub(crate) id: NodeId,
    pub(crate) state: AnyState,
    pub(crate) event_state: SharedAnyState,
    /// The node's current content digest — its content-addressed key (`None` when not
    /// reproducible), computed and stamped by the executor as it reaches the node during
    /// the run ([`digest::node_digest`]). A resident value hits iff its
    /// `produced_under` equals this — so a flipped-back input can't serve a stale value.
    pub(crate) current_digest: Option<Digest>,
    pub(crate) value: ValueState,
}

/// The two slot fields the run loop hands a lambda — its persistent `state` and the
/// fresh output buffer — split-borrowed from one slot so both can be written at once.
/// Produced by [`RuntimeSlot::invoke_slot`].
pub(crate) struct InvokeSlot<'a> {
    pub(crate) state: &'a mut AnyState,
    pub(crate) outputs: &'a mut Vec<DynamicValue>,
}

impl KeyIndexKey<NodeId> for RuntimeSlot {
    fn key(&self) -> &NodeId {
        &self.id
    }
}

impl RuntimeSlot {
    /// Drop the cached output, leaving the persistent `state`/`event_state` intact.
    /// The run loop calls this on the failure paths — a node that errored or was
    /// skipped for an errored dependency — so a stale prior value isn't left resident
    /// as if it were this run's result. (Successful runs reuse the buffer in place;
    /// see [`invoke_slot`](Self::invoke_slot).)
    pub(crate) fn clear_output(&mut self) {
        self.value = ValueState::Empty;
    }

    /// The resident output values, or `None` when the slot isn't `Resident`.
    pub(crate) fn output_values(&self) -> Option<&Vec<DynamicValue>> {
        match &self.value {
            ValueState::Resident { values, .. } => Some(values),
            _ => None,
        }
    }

    /// Prepare the slot for a lambda invocation and hand back *disjoint* mutable
    /// borrows of `state` and the output buffer — the lambda writes both at once,
    /// which a single whole-slot borrow couldn't allow. A resident buffer is reused
    /// **in place** (its prior values kept — a re-running lambda overwrites all its
    /// outputs; a future skip can reuse them), `resize`d to the current arity so a
    /// func-version change that altered output count can't leave a stale-length
    /// buffer. `produced_under` stays as-is until [`stamp_produced`](Self::stamp_produced)
    /// updates it on success.
    pub(crate) fn invoke_slot(&mut self, output_count: usize) -> InvokeSlot<'_> {
        match &mut self.value {
            ValueState::Resident { values, .. } => {
                values.resize(output_count, DynamicValue::Unbound);
            }
            _ => {
                self.value = ValueState::Resident {
                    values: vec![DynamicValue::Unbound; output_count],
                    produced_under: None,
                };
            }
        }
        let ValueState::Resident { values, .. } = &mut self.value else {
            unreachable!("set to Resident just above");
        };
        InvokeSlot {
            state: &mut self.state,
            outputs: values,
        }
    }

    /// Stamp the resident value with the node's current content digest on a successful
    /// run: `produced_under` turns it into a cache hit for the next run (RAM) and the
    /// key its disk blob is stored under. No-op if not `Resident`.
    pub(crate) fn stamp_produced(&mut self) {
        let digest = self.current_digest;
        if let ValueState::Resident { produced_under, .. } = &mut self.value {
            *produced_under = digest;
        }
    }
}

/// The per-node cross-run cache plus its disk backing. `slots` is index-aligned to
/// `program.e_nodes` via [`Self::reconcile`]; the executor computes each node's digest,
/// mutates its outputs/state, and reads reuse state ([`Self::is_resident_hit`]) in its run
/// loop. `disk_store` persists outputs and serves them back — set via
/// [`ExecutionEngine::set_disk_store`](crate::execution::ExecutionEngine::set_disk_store) and
/// kept across graph updates (only `slots` is reconciled/cleared).
#[derive(Default, Debug)]
pub(crate) struct RuntimeCache {
    pub(crate) slots: KeyIndexVec<NodeId, RuntimeSlot>,
    pub(crate) disk_store: DiskStore,
}

impl RuntimeCache {
    pub(crate) fn clear(&mut self) {
        self.slots.clear();
    }

    /// The RAM held by every *resident* value across all slots, split into system
    /// RAM vs GPU VRAM. `OnDisk`/`Empty` slots hold nothing and count zero. A
    /// `Custom` value shared (`Arc`) by more than one slot is counted once — its
    /// bytes exist once — deduped by pointer identity.
    pub(crate) fn resident_ram_usage(&self) -> RamUsage {
        let mut seen: HashSet<*const ()> = HashSet::new();
        let mut total = RamUsage::default();
        for slot in self.slots.iter() {
            let ValueState::Resident { values, .. } = &slot.value else {
                continue;
            };
            for value in values {
                if let DynamicValue::Custom(arc) = value
                    && !seen.insert(Arc::as_ptr(arc) as *const ())
                {
                    // A second slot holds the same Arc — its bytes are already counted.
                    continue;
                }
                total += value.ram_usage();
            }
        }
        total
    }

    /// Per-node resident RAM: each slot holding a non-zero resident value paired with
    /// its footprint (system RAM vs GPU VRAM), keyed by the slot's `NodeId`. Unlike
    /// [`resident_ram_usage`](Self::resident_ram_usage) this does **not** dedup shared
    /// `Arc`s — each node reports the size of the value it holds, even when another
    /// node references the same `Arc`. `OnDisk`/`Empty` slots and zero-byte values are
    /// omitted, so only nodes actually holding memory appear.
    pub(crate) fn resident_ram_by_node(&self) -> Vec<(NodeId, RamUsage)> {
        let mut out = Vec::new();
        for slot in self.slots.iter() {
            let ValueState::Resident { values, .. } = &slot.value else {
                continue;
            };
            let mut usage = RamUsage::default();
            for value in values {
                usage += value.ram_usage();
            }
            if usage.total() > 0 {
                out.push((slot.id, usage));
            }
        }
        out
    }

    /// Rebuild `slots` in `e_nodes` order: preserve each surviving node's cache by
    /// id, default new nodes, trim removed ones. Mirrors the `CompactInsert` the
    /// flattener runs over `e_nodes`, keeping `slots[i]` aligned to `e_nodes[i]`.
    pub(crate) fn reconcile(&mut self, e_nodes: &KeyIndexVec<NodeId, ExecutionNode>) {
        let mut compact = self.slots.compact_insert_start();
        for e_node in e_nodes.iter() {
            compact.insert_with(&e_node.id, || RuntimeSlot {
                id: e_node.id,
                ..Default::default()
            });
        }
    }

    /// Whether node `idx` holds a *resident* output valid for its current digest:
    /// the value is in RAM and was produced under this digest. A `None` current
    /// digest (impure cone) never hits, and a value produced under a *different*
    /// digest (a changed input) misses too. The executor's input read and the
    /// disk-store rely on this being the true "bytes are here" predicate.
    pub(crate) fn is_resident_hit(&self, idx: NodeIdx) -> bool {
        match (&self.slots[idx].value, self.slots[idx].current_digest) {
            (ValueState::Resident { produced_under, .. }, Some(d)) => *produced_under == Some(d),
            _ => false,
        }
    }

    /// Whether the slot currently holds a *usable* cached value for its digest — resident
    /// under the current digest, or a disk blob already flagged [`ValueState::OnDisk`] this
    /// run. Unlike [`is_resident_hit`](Self::is_resident_hit) (RAM only), this also counts a
    /// stat'd-but-unloaded disk blob, so a node the executor's cut pruned (its consumers all
    /// reused, so it never ran) is still reported as *cached* when its value can be served —
    /// while a pruned memory-only node with no value reports `false`.
    pub(crate) fn has_available_value(&self, idx: NodeIdx) -> bool {
        self.is_resident_hit(idx) || self.is_on_disk(idx)
    }

    /// Whether slot `idx` is flagged [`ValueState::OnDisk`] — a blob stat'd this run but not yet
    /// loaded into RAM.
    pub(crate) fn is_on_disk(&self, idx: NodeIdx) -> bool {
        matches!(self.slots[idx].value, ValueState::OnDisk)
    }

    /// Install a disk-loaded output into a slot under `digest` (the node's current
    /// digest), turning a later reuse check into a plain RAM hit.
    pub(crate) fn hydrate(&mut self, idx: NodeIdx, values: Vec<DynamicValue>, digest: Digest) {
        self.slots[idx].value = ValueState::Resident {
            values,
            produced_under: Some(digest),
        };
    }

    /// Flag slot `idx` as `OnDisk` — its value lives only in a blob now. Used by the reuse
    /// check when a blob is found and by [`reclaim_slot`](Self::reclaim_slot)'s demote path.
    pub(crate) fn flag_on_disk(&mut self, idx: NodeIdx) {
        self.slots[idx].value = ValueState::OnDisk;
    }

    /// Read producer `idx`'s output `port` for a consumer: a clone of the value, or — with
    /// `take` — the value itself, moved out of the slot (leaving `Unbound`). The move is the
    /// executor's last-read fast path for a non-RAM producer: the RAM copy would be released
    /// right after anyway, and handing over the slot's copy leaves the consumer as the sole
    /// `Arc` holder so [`DynamicValue::into_custom`] can reuse the allocation in place.
    /// `None` when the slot holds no resident values (a failed hydrate).
    pub(crate) fn read_output_port(
        &mut self,
        program: &ExecutionProgram,
        idx: NodeIdx,
        port: usize,
        take: bool,
    ) -> Option<DynamicValue> {
        let arity = program.e_nodes[idx].outputs.len as usize;
        let ValueState::Resident { values, .. } = &mut self.slots[idx].value else {
            return None;
        };
        assert_eq!(values.len(), arity);
        Some(if take {
            std::mem::take(&mut values[port])
        } else {
            values[port].clone()
        })
    }

    /// Clear a single output value of a resident slot (to `Unbound`), keeping its siblings — the
    /// mid-run per-output release for a non-RAM producer whose one output just went spent while
    /// others are still owed to other consumers. No-op if the slot isn't resident or `port` is
    /// out of range.
    pub(crate) fn clear_output_port(&mut self, idx: NodeIdx, port: usize) {
        if let ValueState::Resident { values, .. } = &mut self.slots[idx].value
            && let Some(slot) = values.get_mut(port)
        {
            *slot = DynamicValue::Unbound;
        }
    }

    // === Caching policy (over the RAM slots + the owned `disk_store`) ===

    /// Stamp node `idx`'s content digest into its slot and return whether an unchanged output
    /// can be reused this run — resident in RAM ([`is_resident_hit`](Self::is_resident_hit)) or
    /// a blob on disk flagged now ([`mark_on_disk_if_present`](Self::mark_on_disk_if_present)).
    /// The single place the digest is folded and the reuse verdict formed — called once per
    /// node per run, by the pre-run [`Resolver`](crate::execution::resolve::Resolver) sweep;
    /// the run loop reads the resolver's verdict rather than re-deriving (a digest folds live
    /// filesystem state and could drift mid-run). A `None` digest (an impure cone) never
    /// reuses.
    ///
    /// RAM reuse trusts residency ([`is_resident_hit`](Self::is_resident_hit)): a resident
    /// digest-valid value is served, because a content digest attests the value produced
    /// under it — however the value came to be resident (mode retention, a preview pin, or
    /// an inspection hydrating a blob). The one exception is the `CachePassthrough`
    /// (file-cache) node: its digest is its path key and attests nothing — the file is the
    /// real cache and deleting it the documented invalidation — so its residency is never
    /// trusted; the disk check (which stats the actual file) is its only reuse path. Disk
    /// reuse stays gated on `persists_to_disk` (`Disk`/`Both`, enforced in
    /// [`DiskStore::blob_path`]); the file-cache node's explicit path is checked directly
    /// in `blob_path`.
    pub(crate) fn stamp_and_check_reuse(
        &mut self,
        program: &ExecutionProgram,
        idx: NodeIdx,
    ) -> bool {
        let digest = node_digest(program, idx, self);
        self.slots[idx].current_digest = digest;
        let ram_hit = self.is_resident_hit(idx) && !is_file_cache(program, idx);
        digest.is_some() && (ram_hit || self.mark_on_disk_if_present(program, idx))
    }

    /// The per-node "reuse from disk?" check, run once a node's digest is computed: if a
    /// decodable blob exists on disk for that digest, flag the slot `OnDisk` (dropping any stale
    /// resident value produced under a superseded digest) and return `true` — the node is served
    /// without running. The bytes stay on disk; they load lazily only when a running consumer
    /// reads the value ([`hydrate_slot`](Self::hydrate_slot)), so a disk-cached value behind
    /// another never enters RAM. A bypassed cache-passthrough always recomputes.
    pub(crate) fn mark_on_disk_if_present(
        &mut self,
        program: &ExecutionProgram,
        idx: NodeIdx,
    ) -> bool {
        if is_bypassed(program, idx) {
            return false;
        }
        let Some(target) = self
            .disk_store
            .blob_path(program, idx, self.slots[idx].current_digest)
        else {
            return false;
        };
        if self
            .disk_store
            .outputs_decodable(program.node_output_types(&program.e_nodes[idx]))
            && target.path().exists()
        {
            self.flag_on_disk(idx);
            true
        } else {
            false
        }
    }

    /// Deserialize node `idx`'s disk blob into its slot. Returns whether the slot is resident
    /// afterward: `true` if already resident or the read succeeded. On a read failure (codec
    /// gone, corrupt, an incompatible format, a wrong output count, or deleted) the slot is
    /// cleared, so the demanding consumer is dropped this run and the *next* reopen recomputes
    /// the node. A broken **content-addressed** blob is also deleted — without that,
    /// [`DiskStore::store`]'s skip-if-exists would keep the broken file forever. An
    /// **explicit-path** blob is the user's own file and is never deleted here: `store`
    /// overwrites it on the recompute anyway.
    pub(crate) async fn hydrate_slot(&mut self, program: &ExecutionProgram, idx: NodeIdx) -> bool {
        // A fresh value already in RAM needs no load. A *stale* resident value can't reach here:
        // `mark_on_disk_if_present` demotes "stale + blob on disk" to `OnDisk` (dropping the
        // stale value) before a consumer would read it.
        if self.is_resident_hit(idx) {
            return true;
        }
        if !self.is_on_disk(idx) {
            return false;
        }
        // The slot claimed an on-disk blob. Load it; on success it's resident.
        if let Some(digest) = self.slots[idx].current_digest
            && let Some(target) = self.disk_store.blob_path(program, idx, Some(digest))
        {
            // A content-addressed blob folds the output signature into its digest, so its
            // count always matches — but an explicit-path (file-cache) blob is keyed on the
            // path alone, and a user-pointed file can decode to any output count. Reject a
            // mismatch here as a miss rather than letting a consumer's arity assert panic.
            let arity = program.e_nodes[idx].outputs.len as usize;
            match self.disk_store.read(target.path()).await {
                Some(values) if values.len() == arity => {
                    self.hydrate(idx, values, digest);
                    return true;
                }
                Some(values) => {
                    tracing::warn!(
                        path = %target.path().display(),
                        expected = arity,
                        got = values.len(),
                        "cached outputs have the wrong count; ignoring blob"
                    );
                }
                None => {}
            }
            if matches!(target, Target::Addressed(_)) {
                self.disk_store.delete(target.path());
            }
        }
        self.slots[idx].clear_output();
        false
    }

    /// Materialize node `idx` plus the producers feeding its inputs — the values an inspection
    /// of `idx` reads (its own outputs and its inputs' resolved values). Lets a disk-cached node
    /// no run touched still show its value when the editor selects it, without the run having
    /// eagerly loaded every blob.
    ///
    /// Hydrated values simply stay resident (until a later run's eviction demotes them):
    /// a content digest attests the value produced under it, so the reuse check serving a
    /// hydrated leftover is correct — and the file-cache node, whose path-key digest
    /// attests nothing, is the one node whose residency
    /// [`stamp_and_check_reuse`](Self::stamp_and_check_reuse) never trusts.
    pub(crate) async fn hydrate_for_inspection(
        &mut self,
        program: &ExecutionProgram,
        idx: NodeIdx,
    ) {
        self.hydrate_slot(program, idx).await;
        let span = program.e_nodes[idx].inputs;
        for pool_idx in span.range() {
            if let ExecutionBinding::Bind(addr) = &program.inputs[pool_idx].binding {
                self.hydrate_slot(program, addr.target_idx).await;
            }
        }
    }

    /// Write node `idx`'s freshly-computed outputs to disk the moment it finishes (the executor
    /// calls this right after a successful invoke), so a long run's earlier caches are durable
    /// even if a later node errors or the run is cancelled. The target and output slice are
    /// snapshotted **synchronously**; only [`DiskStore::store`]'s write awaits, so the borrow
    /// across the await is just the value slice (`Sync`), never the whole cache.
    ///
    /// Only writes a value that matches the node's *current* digest
    /// ([`is_resident_hit`](Self::is_resident_hit)): a resident value produced under a superseded
    /// digest must not be written to the new digest's content-addressed path. In the run loop the
    /// just-stamped value is always a current hit; this guards the deferred
    /// [`store_resident_caches`](crate::execution::ExecutionEngine::store_resident_caches) flush.
    pub(crate) fn store_node<'a>(
        &'a self,
        program: &ExecutionProgram,
        idx: NodeIdx,
        ctx: &'a mut ContextManager,
    ) -> impl Future<Output = ()> + 'a {
        let target = self
            .disk_store
            .blob_path(program, idx, self.slots[idx].current_digest);
        let outputs = self
            .is_resident_hit(idx)
            .then(|| self.slots[idx].output_values())
            .flatten();
        let disk = &self.disk_store;
        async move {
            let (Some(target), Some(outputs)) = (target, outputs) else {
                return;
            };
            disk.store(&target, outputs, ctx).await;
        }
    }

    /// Reclaim node `idx`'s spent RAM value: demote to `OnDisk` if a blob for its current digest
    /// can serve it again (lossless, reloads on demand), else drop a **non-RAM** value with no
    /// such blob (`None`, or a `Disk` value whose blob is missing) so it recomputes when next
    /// needed; a `Ram`/`Both` value with no blob is left resident — its mode promised to hold it.
    /// The single place the demote-or-drop decision lives, shared by the mid-run release (the
    /// executor, once a non-RAM node's every output is read) and the end-of-run
    /// [`evict_unused`](Self::evict_unused) sweep. The *caller* decides eligibility.
    pub(crate) fn reclaim_slot(&mut self, program: &ExecutionProgram, idx: NodeIdx) {
        let reloadable = self
            .disk_store
            .blob_path(program, idx, self.slots[idx].current_digest)
            .is_some_and(|target| target.path().exists());
        if reloadable {
            self.flag_on_disk(idx);
        } else if !program.e_nodes[idx].cache.caches_in_ram() {
            self.slots[idx].clear_output();
        }
    }

    /// After a run, reclaim RAM the run's retention policy doesn't call for holding. A
    /// non-retained value whose consumers all ran was already released mid-run the moment its
    /// last consumer read it (the executor's [`reclaim_slot`](Self::reclaim_slot) call); this
    /// end-of-run sweep covers the rest — prior-run leftovers this run never touched (e.g. a
    /// cached value that fell *behind* the frontier when a downstream node became a disk hit),
    /// and a non-retained value some consumer didn't reach (so its outputs never all went
    /// spent).
    ///
    /// Both columns are per-run state reused here, not recomputed: `disposition` is the
    /// resolver's — [`needed`](Disposition::needed) marks the active frontier, a node some
    /// running node will read — and `retain` is the executor's retention policy (RAM-caching
    /// mode, or a pinned preview root whose readable output was the point of its run). A
    /// retained node on the frontier stays resident for the next run's RAM hit; every other
    /// resident value goes through [`reclaim_slot`](Self::reclaim_slot), which demotes a
    /// reloadable one to disk and drops a non-RAM one (a non-reloadable `Ram`/`Both` leftover
    /// is kept, its mode's promise).
    pub(crate) fn evict_unused(
        &mut self,
        program: &ExecutionProgram,
        disposition: &NodeColumn<Disposition>,
        retain: &NodeColumn<bool>,
    ) {
        for idx in program.node_indices() {
            if self.slots[idx].output_values().is_none() {
                continue;
            }
            // A retained node on the active frontier stays hot for the next run's RAM hit.
            // (A pinned root is always on the frontier — roots seed the disposition walk.)
            if retain[idx] && disposition[idx].needed() {
                continue;
            }
            self.reclaim_slot(program, idx);
        }
    }
}

/// True for a bypassed cache-passthrough node — recompute + overwrite, so never hydrate it.
fn is_bypassed(program: &ExecutionProgram, idx: NodeIdx) -> bool {
    matches!(
        program.e_nodes[idx].special,
        Some(SpecialNode::CachePassthrough { bypass: true })
    )
}

/// True for any cache-passthrough (file-cache) node, bypassed or not. Its digest is a path
/// key that attests nothing about the value, so the reuse check never trusts its residency.
fn is_file_cache(program: &ExecutionProgram, idx: NodeIdx) -> bool {
    matches!(
        program.e_nodes[idx].special,
        Some(SpecialNode::CachePassthrough { .. })
    )
}

#[cfg(test)]
mod tests;
