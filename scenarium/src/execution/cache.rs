//! The per-node cross-run runtime cache: output values + content digests + node
//! state, keyed by `NodeId` and index-aligned to the program's `e_nodes`. Owned
//! by the [`ExecutionEngine`](crate::execution::ExecutionEngine); written by the
//! executor's run loop, read by the planner's availability check
//! ([`Cache::is_available`]), and reconciled against the node set at each `update`.
//! Per-run results (errors, timings) are *not* here — they belong to a single run
//! and live on the run, not the cache.

use common::{KeyIndexKey, KeyIndexVec};

use crate::common::shared_any_state::SharedAnyState;
use crate::data::DynamicValue;
use crate::execution::digest::{Digest, DigestEngine};
use crate::execution::program::{ExecutionNode, ExecutionProgram};
use crate::graph::NodeId;
use crate::prelude::AnyState;

/// One node's cross-run runtime state, index-aligned to the program's `e_nodes`:
/// the value cache (`output_values` plus the `output_digest` they were produced
/// under) and the node's persistent `state`/`event_state`. Carries its own `id`
/// so the cache reconciles by key on `update` (surviving node reorder/trim).
#[derive(Default, Debug)]
pub(crate) struct RuntimeSlot {
    pub(crate) id: NodeId,
    pub(crate) state: AnyState,
    pub(crate) event_state: SharedAnyState,
    pub(crate) output_values: Option<Vec<DynamicValue>>,
    /// The node's current content digest (`None` when not reproducible), recomputed
    /// by the engine at `update`. The cache hit is `output_digest == current_digest`;
    /// the run stamps `output_digest` from this on success.
    pub(crate) current_digest: Option<Digest>,
    /// The content digest `output_values` were produced under (`None` until run, or
    /// for a non-reproducible node). Compared to `current_digest` for the cache
    /// check — a changed input no longer matches, so a flipped-back parameter can't
    /// serve a stale value — and the disk key for a `persist` node.
    pub(crate) output_digest: Option<Digest>,
    /// Set at `update` when a decodable blob exists on disk for the *current*
    /// digest, *without* loading it — the planner treats such a node as cached and
    /// prunes its cone, but the bytes are only deserialized on demand (the
    /// execution frontier or an inspection read), so a disk-cached value behind
    /// another disk-cached value never enters RAM. See [`Cache::is_available`].
    pub(crate) disk_available: bool,
}

impl KeyIndexKey<NodeId> for RuntimeSlot {
    fn key(&self) -> &NodeId {
        &self.id
    }
}

impl RuntimeSlot {
    fn reset_state(&mut self) {
        self.state = AnyState::default();
        self.event_state = SharedAnyState::default();
        self.output_values = None;
        self.output_digest = None;
        self.disk_available = false;
    }

    /// Drop the cached output, leaving the persistent `state`/`event_state`
    /// intact. Run for each scheduled node before the run loop so the loop's
    /// in-place reuse of the output `Vec` can't leak a prior run's value through
    /// a port the lambda leaves unwritten this time.
    pub(crate) fn clear_output(&mut self) {
        self.output_values = None;
        self.output_digest = None;
    }
}

/// The per-node cross-run cache. `slots` is index-aligned to `program.e_nodes`
/// via [`Self::reconcile`]; the executor mutates a slot's outputs/state in its
/// run loop, while the planner reads only [`Self::is_available`].
#[derive(Default, Debug)]
pub(crate) struct Cache {
    pub(crate) slots: KeyIndexVec<NodeId, RuntimeSlot>,
}

impl Cache {
    pub(crate) fn clear(&mut self) {
        self.slots.clear();
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

    pub(crate) fn reset_states(&mut self) {
        for slot in self.slots.iter_mut() {
            slot.reset_state();
        }
    }

    /// Refresh every slot's `current_digest` for `program`. Digests are
    /// compile-stable (consts/bindings/func versions are fixed between updates),
    /// so the engine calls this once per `update`; the planner and run then read
    /// the stored value rather than re-walking the cone each execute.
    pub(crate) fn recompute_digests(&mut self, program: &ExecutionProgram) {
        let mut engine = DigestEngine::with_fs(program);
        for idx in 0..program.e_nodes.len() {
            self.slots[idx].current_digest = engine.node_digest(idx);
        }
    }

    pub(crate) fn current_digest(&self, idx: usize) -> Option<Digest> {
        self.slots[idx].current_digest
    }

    pub(crate) fn output_values(&self, idx: usize) -> Option<&Vec<DynamicValue>> {
        self.slots[idx].output_values.as_ref()
    }

    /// Whether node `idx` holds a *resident* output valid for its current digest:
    /// the value is in RAM and was produced under this digest. A `None` current
    /// digest (impure cone) never hits, and a digest the slot was *not* produced
    /// under (a changed input) misses too. The executor's input read and the
    /// disk-store rely on this being the true "bytes are here" predicate.
    pub(crate) fn is_resident_hit(&self, idx: usize) -> bool {
        let slot = &self.slots[idx];
        match slot.current_digest {
            Some(d) => slot.output_values.is_some() && slot.output_digest == Some(d),
            None => false,
        }
    }

    /// Whether node `idx`'s output is *available* for the current digest — resident
    /// in RAM, or sitting on disk as a decodable blob ([`RuntimeSlot::disk_available`],
    /// flagged at `update` without loading it). This is what the planner prunes on:
    /// a node whose value can be served — from either tier — needn't be executed.
    /// The disk tier is materialized lazily, so a disk-cached cone behind another
    /// disk-cached node is pruned here without ever being read into RAM.
    pub(crate) fn is_available(&self, idx: usize) -> bool {
        self.is_resident_hit(idx) || self.slots[idx].disk_available
    }

    /// Install a disk-loaded output into a slot under `digest` (the node's current
    /// digest), turning the next planner check into a plain RAM hit.
    pub(crate) fn hydrate(&mut self, idx: usize, values: Vec<DynamicValue>, digest: Digest) {
        let slot = &mut self.slots[idx];
        slot.output_values = Some(values);
        slot.output_digest = Some(digest);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::StaticValue;

    fn out() -> Vec<DynamicValue> {
        vec![DynamicValue::Static(StaticValue::Int(1))]
    }

    /// `is_resident_hit` is the resident-cache definition: a slot hits iff it has a
    /// current digest, holds values, and those values were produced under that
    /// exact digest. The four cases below are the full truth table.
    #[test]
    fn is_hit_requires_current_digest_values_and_matching_output_digest() {
        let d: Digest = [7u8; 32];
        let other: Digest = [8u8; 32];
        let mut cache = Cache::default();

        // 0: impure cone (no current digest) — never hits, even holding values.
        cache.slots.add(RuntimeSlot {
            id: NodeId::from_u128(1),
            output_values: Some(out()),
            output_digest: Some(d),
            current_digest: None,
            ..Default::default()
        });
        // 1: has a current digest but no cached values.
        cache.slots.add(RuntimeSlot {
            id: NodeId::from_u128(2),
            current_digest: Some(d),
            ..Default::default()
        });
        // 2: values present, but produced under a *different* digest (stale).
        cache.slots.add(RuntimeSlot {
            id: NodeId::from_u128(3),
            current_digest: Some(d),
            output_values: Some(out()),
            output_digest: Some(other),
            ..Default::default()
        });
        // 3: values produced under the current digest — the only hit.
        cache.slots.add(RuntimeSlot {
            id: NodeId::from_u128(4),
            current_digest: Some(d),
            output_values: Some(out()),
            output_digest: Some(d),
            ..Default::default()
        });

        assert!(!cache.is_resident_hit(0), "impure cone never hits");
        assert!(!cache.is_resident_hit(1), "no cached values is a miss");
        assert!(
            !cache.is_resident_hit(2),
            "values under a stale digest is a miss"
        );
        assert!(
            cache.is_resident_hit(3),
            "values under the current digest is a hit"
        );
    }

    /// `is_available` widens the resident hit with the disk tier: a node with no
    /// resident value but a `disk_available` flag is *available* (the planner prunes
    /// it) yet not a resident hit (its bytes aren't in RAM until hydrated). The flag
    /// never resurrects a value the resident check would reject for a stale digest —
    /// it's an independent "a blob exists" bit, gated by the same `update`.
    #[test]
    fn is_available_unions_resident_hit_and_disk_flag() {
        let d: Digest = [7u8; 32];
        let mut cache = Cache::default();

        // 0: nothing resident, but a decodable blob was flagged on disk.
        cache.slots.add(RuntimeSlot {
            id: NodeId::from_u128(1),
            current_digest: Some(d),
            disk_available: true,
            ..Default::default()
        });
        // 1: a plain resident hit, no disk flag.
        cache.slots.add(RuntimeSlot {
            id: NodeId::from_u128(2),
            current_digest: Some(d),
            output_values: Some(out()),
            output_digest: Some(d),
            ..Default::default()
        });
        // 2: neither — a true miss.
        cache.slots.add(RuntimeSlot {
            id: NodeId::from_u128(3),
            current_digest: Some(d),
            ..Default::default()
        });

        assert!(!cache.is_resident_hit(0), "disk-only is not resident");
        assert!(cache.is_available(0), "disk-only is available (prunable)");
        assert!(
            cache.is_resident_hit(1) && cache.is_available(1),
            "resident ⇒ both"
        );
        assert!(!cache.is_available(2), "no value, no blob ⇒ unavailable");
    }

    #[test]
    fn hydrate_turns_a_miss_into_a_hit() {
        let d: Digest = [3u8; 32];
        let mut cache = Cache::default();
        cache.slots.add(RuntimeSlot {
            id: NodeId::from_u128(1),
            current_digest: Some(d),
            ..Default::default()
        });
        assert!(!cache.is_resident_hit(0), "empty slot misses");

        cache.hydrate(0, out(), d);
        assert!(
            cache.is_resident_hit(0),
            "a slot hydrated under its current digest hits"
        );

        // Hydrating under a digest that is no longer current does not hit.
        cache.slots[0].current_digest = Some([9u8; 32]);
        assert!(!cache.is_resident_hit(0), "current digest moved on ⇒ miss");
    }
}
