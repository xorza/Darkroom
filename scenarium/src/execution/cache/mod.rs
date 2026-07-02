//! The per-node cross-run runtime cache: output values + content digests + node
//! state, keyed by `NodeId` and index-aligned to the program's `e_nodes`. Owned
//! by the [`ExecutionEngine`](crate::execution::ExecutionEngine); the executor's run
//! loop computes each node's digest, writes its outputs, and reads its reuse state
//! ([`Cache::is_resident_hit`]). Reconciled against the node set at each `update`.
//! Per-run results (errors, timings) are *not* here — they belong to a single run
//! and live on the run, not the cache.

use common::{KeyIndexKey, KeyIndexVec};

use crate::common::shared_any_state::SharedAnyState;
use crate::data::DynamicValue;
use crate::execution::digest::Digest;
use crate::execution::program::{ExecutionNode, NodeIdx};
use crate::graph::NodeId;
use crate::prelude::AnyState;

/// One node's cached output as an explicit three-state machine. The states are mutually
/// exclusive, so the bad combinations — "resident *and* flagged on disk", "value present
/// but no digest tracked", a stale resident value masking a fresh disk blob — can't be
/// built. The node's *identity* digest is a separate axis
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
    /// Not in RAM, but a decodable blob exists on disk for the slot's *current* digest —
    /// flagged during the run by [`OutputCache::mark_on_disk_if_present`](crate::execution::output_cache::OutputCache::mark_on_disk_if_present)
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
    /// Drop the cached output, leaving the persistent `state`/`event_state` intact.
    /// The run loop calls this on the failure paths — a node that errored or was
    /// skipped for an errored dependency — so a stale prior value isn't left resident
    /// as if it were this run's result. (Successful runs reuse the buffer in place;
    /// see [`invoke_slot`](Self::invoke_slot).)
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
            ValueCache::Resident { values, .. } => {
                values.resize(output_count, DynamicValue::Unbound);
            }
            _ => {
                self.value = ValueCache::Resident {
                    values: vec![DynamicValue::Unbound; output_count],
                    produced_under: None,
                };
            }
        }
        let ValueCache::Resident { values, .. } = &mut self.value else {
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
        if let ValueCache::Resident { produced_under, .. } = &mut self.value {
            *produced_under = digest;
        }
    }
}

/// The per-node cross-run cache. `slots` is index-aligned to `program.e_nodes`
/// via [`Self::reconcile`]; the executor computes each node's digest, mutates its
/// outputs/state, and reads reuse state ([`Self::is_resident_hit`]) in its run loop.
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

    /// Whether the slot currently holds a *usable* cached value for its digest — resident
    /// under the current digest, or a disk blob already flagged [`ValueCache::OnDisk`] this
    /// run. Unlike [`is_resident_hit`](Self::is_resident_hit) (RAM only), this also counts a
    /// stat'd-but-unloaded disk blob, so a node the executor's cut pruned (its consumers all
    /// reused, so it never ran) is still reported as *cached* when its value can be served —
    /// while a pruned memory-only node with no value reports `false`.
    pub(crate) fn has_available_value(&self, idx: NodeIdx) -> bool {
        self.is_resident_hit(idx) || matches!(self.slots[idx].value, ValueCache::OnDisk)
    }

    /// Install a disk-loaded output into a slot under `digest` (the node's current
    /// digest), turning a later reuse check into a plain RAM hit.
    pub(crate) fn hydrate(&mut self, idx: NodeIdx, values: Vec<DynamicValue>, digest: Digest) {
        self.slots[idx].value = ValueCache::Resident {
            values,
            produced_under: Some(digest),
        };
    }
}

#[cfg(test)]
mod tests;
