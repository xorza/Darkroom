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
use crate::execution::program::{ExecutionNode, ExecutionProgram, NodeIdx};
use crate::graph::NodeId;
use crate::prelude::AnyState;

/// One node's cached output: an explicit three-state machine replacing what were
/// three loosely-coupled fields (`output_values` / `output_digest` /
/// `disk_available`). The states are mutually exclusive, so the previously-
/// representable bad combinations — "resident *and* flagged on disk", "value present
/// but no digest tracked", a stale resident value masking a fresh disk blob — can't
/// be built. The node's *identity* digest is a separate axis
/// ([`RuntimeSlot::current_digest`]); this models only the value.
#[derive(Default, Debug)]
pub(crate) enum ValueCache {
    /// No cached output — never produced, evicted, or cleared for re-execution.
    #[default]
    Empty,
    /// Values resident in RAM. `produced_under` is the digest they were computed
    /// under — `None` for an impure node, which holds a value but is never a hit.
    Resident {
        values: Vec<DynamicValue>,
        produced_under: Option<Digest>,
    },
    /// Not in RAM, but a decodable blob exists on disk for the slot's *current*
    /// digest — flagged at `update` without loading (see [`Cache::is_available`]),
    /// deserialized on demand (execution frontier / inspection). Lets a disk-cached
    /// value behind another disk-cached value never enter RAM.
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
    /// The node's current content digest (`None` when not reproducible), recomputed
    /// by the engine at `update`. A resident value hits iff its `produced_under`
    /// equals this — so a flipped-back parameter can't serve a stale value; the run
    /// stamps it on success.
    pub(crate) current_digest: Option<Digest>,
    pub(crate) value: ValueCache,
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
    fn reset_state(&mut self) {
        self.state = AnyState::default();
        self.event_state = SharedAnyState::default();
        self.value = ValueCache::Empty;
    }

    /// Drop the cached output, leaving the persistent `state`/`event_state`
    /// intact. Run for each scheduled node before the run loop so the loop's
    /// in-place reuse of the output `Vec` can't leak a prior run's value through
    /// a port the lambda leaves unwritten this time.
    pub(crate) fn clear_output(&mut self) {
        self.value = ValueCache::Empty;
    }

    /// The resident output values, or `None` when the slot isn't `Resident`.
    pub(crate) fn output_values(&self) -> Option<&Vec<DynamicValue>> {
        match &self.value {
            ValueCache::Resident { values, .. } => Some(values),
            _ => None,
        }
    }

    /// Prepare the slot for a lambda invocation: ensure a fresh `Unbound` output
    /// buffer (the pre-run `clear_output` left the slot `Empty`, so this allocates;
    /// `produced_under` stays unset until [`stamp_produced`](Self::stamp_produced) on
    /// success), and hand back *disjoint* mutable borrows of `state` and that buffer —
    /// the lambda writes both at once, which a single whole-slot borrow couldn't allow.
    pub(crate) fn invoke_slot(&mut self, output_count: usize) -> InvokeSlot<'_> {
        if !matches!(self.value, ValueCache::Resident { .. }) {
            self.value = ValueCache::Resident {
                values: vec![DynamicValue::Unbound; output_count],
                produced_under: None,
            };
        }
        let ValueCache::Resident { values, .. } = &mut self.value else {
            unreachable!("set to Resident just above");
        };
        InvokeSlot {
            state: &mut self.state,
            outputs: values,
        }
    }

    /// Stamp the resident value with the node's current digest on a successful run,
    /// turning it into a cache hit for the next plan. No-op if not `Resident`.
    pub(crate) fn stamp_produced(&mut self) {
        let digest = self.current_digest;
        if let ValueCache::Resident { produced_under, .. } = &mut self.value {
            *produced_under = digest;
        }
    }
}

/// The per-node cross-run cache. `slots` is index-aligned to `program.e_nodes`
/// via [`Self::reconcile`]; the executor mutates a slot's outputs/state in its
/// run loop, while the planner reads only [`Self::is_available`].
#[derive(Default, Debug)]
pub(crate) struct Cache {
    pub(crate) slots: KeyIndexVec<NodeId, RuntimeSlot>,
    /// One digest engine kept across updates; its working columns are reused (sized
    /// per recompile) instead of reallocated. Holds no program — that's passed in.
    digest_engine: DigestEngine,
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

    /// Refresh every slot's `current_digest`. Compile-stable (consts / bindings / func
    /// versions / output signatures are fixed between updates), so the engine calls
    /// this once per `update`; the planner and run read the stored value rather than
    /// re-walking the cone each execute. The digest folds the program's resolved
    /// output types (`ExecutionProgram::resolve_output_types`, run earlier this
    /// update), so a redefined output re-keys the cache.
    pub(crate) fn recompute_digests(&mut self, program: &ExecutionProgram) {
        // The output-type pool is resolved by a separate `update` step
        // (`ExecutionProgram::resolve_output_types`) that must run first — the digest
        // folds it. Catch a forgotten/incomplete resolve loudly here rather than as a
        // cryptic out-of-range slice deep in the recursion. O(1), so a release assert.
        assert_eq!(
            program.output_types.len(),
            program.n_outputs,
            "output types must be resolved before digesting"
        );
        self.digest_engine.reset(program);
        for idx in program.node_indices() {
            self.slots[idx].current_digest = self.digest_engine.node_digest(program, idx);
        }
    }

    pub(crate) fn current_digest(&self, idx: NodeIdx) -> Option<Digest> {
        self.slots[idx].current_digest
    }

    pub(crate) fn output_values(&self, idx: NodeIdx) -> Option<&Vec<DynamicValue>> {
        self.slots[idx].output_values()
    }

    /// Whether node `idx` holds a *resident* output valid for its current digest:
    /// the value is in RAM and was produced under this digest. A `None` current
    /// digest (impure cone) never hits, and a value produced under a *different*
    /// digest (a changed input) misses too. The executor's input read and the
    /// disk-store rely on this being the true "bytes are here" predicate.
    pub(crate) fn is_resident_hit(&self, idx: NodeIdx) -> bool {
        match (&self.slots[idx].value, self.slots[idx].current_digest) {
            (ValueCache::Resident { produced_under, .. }, Some(d)) => *produced_under == Some(d),
            _ => false,
        }
    }

    /// Whether node `idx`'s output is *available* for the current digest — resident
    /// in RAM, or sitting on disk as a decodable blob ([`ValueCache::OnDisk`], flagged
    /// at `update` without loading it). This is what the planner prunes on: a node
    /// whose value can be served — from either tier — needn't be executed. The disk
    /// tier is materialized lazily, so a disk-cached cone behind another disk-cached
    /// node is pruned here without ever being read into RAM.
    pub(crate) fn is_available(&self, idx: NodeIdx) -> bool {
        self.is_resident_hit(idx) || matches!(self.slots[idx].value, ValueCache::OnDisk)
    }

    /// Install a disk-loaded output into a slot under `digest` (the node's current
    /// digest), turning the next planner check into a plain RAM hit.
    pub(crate) fn hydrate(&mut self, idx: NodeIdx, values: Vec<DynamicValue>, digest: Digest) {
        self.slots[idx].value = ValueCache::Resident {
            values,
            produced_under: Some(digest),
        };
    }

    /// Drop resident outputs of the non-reproducible cone (`current_digest` is
    /// `None` — impure nodes and anything tainted by an impure producer). Such a
    /// value can never be a resident hit (see [`Self::is_resident_hit`]), so a copy
    /// left from a prior run is dead weight — and that cone re-executes this run
    /// anyway. Called at the *start* of a run, so the last run's outputs stay
    /// resident while idle (the inspector can still read them) and are dropped only
    /// once a new run begins. Reproducible resident hits and disk-backed values keep
    /// a `Some` digest, so they're untouched.
    pub(crate) fn evict_non_reproducible(&mut self) {
        for slot in self.slots.iter_mut() {
            if slot.current_digest.is_none() {
                slot.clear_output();
            }
        }
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
        let d = Digest([7u8; 32]);
        let other = Digest([8u8; 32]);
        let mut cache = Cache::default();

        // 0: impure cone (no current digest) — never hits, even holding values.
        cache.slots.add(RuntimeSlot {
            id: NodeId::from_u128(1),
            value: ValueCache::Resident {
                values: out(),
                produced_under: Some(d),
            },
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
            value: ValueCache::Resident {
                values: out(),
                produced_under: Some(other),
            },
            ..Default::default()
        });
        // 3: values produced under the current digest — the only hit.
        cache.slots.add(RuntimeSlot {
            id: NodeId::from_u128(4),
            current_digest: Some(d),
            value: ValueCache::Resident {
                values: out(),
                produced_under: Some(d),
            },
            ..Default::default()
        });

        assert!(!cache.is_resident_hit(NodeIdx(0)), "impure cone never hits");
        assert!(
            !cache.is_resident_hit(NodeIdx(1)),
            "no cached values is a miss"
        );
        assert!(
            !cache.is_resident_hit(NodeIdx(2)),
            "values under a stale digest is a miss"
        );
        assert!(
            cache.is_resident_hit(NodeIdx(3)),
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
        let d = Digest([7u8; 32]);
        let mut cache = Cache::default();

        // 0: nothing resident, but a decodable blob was flagged on disk.
        cache.slots.add(RuntimeSlot {
            id: NodeId::from_u128(1),
            current_digest: Some(d),
            value: ValueCache::OnDisk,
            ..Default::default()
        });
        // 1: a plain resident hit, no disk flag.
        cache.slots.add(RuntimeSlot {
            id: NodeId::from_u128(2),
            current_digest: Some(d),
            value: ValueCache::Resident {
                values: out(),
                produced_under: Some(d),
            },
            ..Default::default()
        });
        // 2: neither — a true miss.
        cache.slots.add(RuntimeSlot {
            id: NodeId::from_u128(3),
            current_digest: Some(d),
            ..Default::default()
        });

        assert!(
            !cache.is_resident_hit(NodeIdx(0)),
            "disk-only is not resident"
        );
        assert!(
            cache.is_available(NodeIdx(0)),
            "disk-only is available (prunable)"
        );
        assert!(
            cache.is_resident_hit(NodeIdx(1)) && cache.is_available(NodeIdx(1)),
            "resident ⇒ both"
        );
        assert!(
            !cache.is_available(NodeIdx(2)),
            "no value, no blob ⇒ unavailable"
        );
    }

    #[test]
    fn hydrate_turns_a_miss_into_a_hit() {
        let d = Digest([3u8; 32]);
        let mut cache = Cache::default();
        cache.slots.add(RuntimeSlot {
            id: NodeId::from_u128(1),
            current_digest: Some(d),
            ..Default::default()
        });
        assert!(!cache.is_resident_hit(NodeIdx(0)), "empty slot misses");

        cache.hydrate(NodeIdx(0), out(), d);
        assert!(
            cache.is_resident_hit(NodeIdx(0)),
            "a slot hydrated under its current digest hits"
        );

        // Hydrating under a digest that is no longer current does not hit.
        cache.slots[0].current_digest = Some(Digest([9u8; 32]));
        assert!(
            !cache.is_resident_hit(NodeIdx(0)),
            "current digest moved on ⇒ miss"
        );
    }

    /// `evict_non_reproducible` drops resident outputs of the non-reproducible cone
    /// (`current_digest == None`) and leaves every reproducible slot — resident hits
    /// and disk-backed values, both `Some`-digest — untouched.
    #[test]
    fn evict_non_reproducible_clears_only_none_digest_residents() {
        let d = Digest([7u8; 32]);
        let mut cache = Cache::default();

        // 0: impure/tainted (no digest) holding a value — evicted.
        cache.slots.add(RuntimeSlot {
            id: NodeId::from_u128(1),
            current_digest: None,
            value: ValueCache::Resident {
                values: out(),
                produced_under: None,
            },
            ..Default::default()
        });
        // 1: reproducible resident hit — kept.
        cache.slots.add(RuntimeSlot {
            id: NodeId::from_u128(2),
            current_digest: Some(d),
            value: ValueCache::Resident {
                values: out(),
                produced_under: Some(d),
            },
            ..Default::default()
        });
        // 2: disk-backed value (has a digest) — kept.
        cache.slots.add(RuntimeSlot {
            id: NodeId::from_u128(3),
            current_digest: Some(d),
            value: ValueCache::OnDisk,
            ..Default::default()
        });

        cache.evict_non_reproducible();

        assert!(
            matches!(cache.slots[NodeIdx(0)].value, ValueCache::Empty),
            "the None-digest resident output is dropped"
        );
        assert!(
            cache.is_resident_hit(NodeIdx(1)),
            "a reproducible resident hit survives"
        );
        assert!(
            matches!(cache.slots[NodeIdx(2)].value, ValueCache::OnDisk),
            "a disk-backed value survives"
        );
    }
}
