//! The cross-run runtime cache: the per-node RAM slots (output values + content digests +
//! node state, keyed by `ExecutionNodeId`) **plus** the
//! [`DiskStore`] backing them, and the caching policy over the two — reuse detection, frontier
//! hydration, persistence, and RAM eviction. Owned by the
//! [`ExecutionEngine`](crate::execution::ExecutionEngine); the executor's run loop drives it a
//! node at a time. The [`DiskStore`] is pure blob I/O and knows nothing of the cache; this type
//! reads a node's digest/value-state off its slot and the blob off disk, and pushes the result
//! back — so RAM eviction lives here, on the cache that owns both stores.
//! Per-run results (errors, timings) are *not* here — they belong to a single run, not the cache.

use std::collections::HashSet;
use std::future::Future;
use std::sync::Arc;

use hashbrown::HashMap;

use crate::execution::digest::{Digest, node_digest};
use crate::execution::disk_store::DiskStore;
use crate::execution::identity::ExecutionNodeId;
use crate::execution::program::ExecutionProgram;
use crate::execution::resource::RunResourceStamps;
use crate::execution::stats::NodeRamUsage;
use crate::node::lambda::OutputDemand;
use crate::runtime::any_state::AnyState;
use crate::runtime::context::ContextManager;
use crate::runtime::shared_any_state::SharedAnyState;
use crate::{DynamicValue, RamUsage};

#[derive(Debug)]
pub(crate) struct OutputSnapshot {
    pub(crate) values: Vec<DynamicValue>,
}

impl OutputSnapshot {
    pub(crate) fn new(values: Vec<DynamicValue>) -> Self {
        Self { values }
    }

    fn empty(output_count: usize) -> Self {
        Self::new(vec![DynamicValue::Unbound; output_count])
    }

    fn reset(&mut self, output_count: usize) {
        self.values.clear();
        self.values.resize(output_count, DynamicValue::Unbound);
    }

    fn covers_demand(&self, demand: &[OutputDemand]) -> bool {
        debug_assert_eq!(
            self.values.len(),
            demand.len(),
            "cached output values must match output demand arity"
        );
        self.values
            .iter()
            .zip(demand)
            .all(|(value, demand)| !matches!(value, DynamicValue::Unbound) || demand.is_skip())
    }
}

/// Whether one node's cached output is resident. Disk availability is discovered on demand
/// from the node's digest rather than mirrored in runtime state.
#[derive(Default, Debug)]
pub(crate) enum ValueState {
    /// No cached output — never produced, evicted, or cleared for re-execution.
    #[default]
    Empty,
    /// Values resident in RAM. `produced_under` is the digest they were computed
    /// under — `None` for an impure node, which holds a value but is never a hit.
    Resident {
        snapshot: OutputSnapshot,
        produced_under: Option<Digest>,
    },
}

/// One node's cross-run runtime state: the [`value`](RuntimeSlot::value) cache and
/// the node's persistent `state`/`event_state`.
#[derive(Default, Debug)]
pub(crate) struct RuntimeSlot {
    pub(crate) state: AnyState,
    pub(crate) event_state: SharedAnyState,
    /// The node's current content digest — its cache-validity key (`None` when not
    /// reproducible), stamped producer-first by the resolver and refreshed at execution
    /// reach only for a late bound-resource identity ([`digest::node_digest`]). A resident
    /// value hits iff its
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
            ValueState::Resident { snapshot, .. } => Some(&snapshot.values),
            _ => None,
        }
    }

    /// Reject stale resident references so they cannot enter a new resource-backed digest.
    pub(crate) fn current_output_values(&self) -> Option<&[DynamicValue]> {
        match &self.value {
            ValueState::Resident {
                snapshot,
                produced_under,
            } if self.current_digest.is_some() && *produced_under == self.current_digest => {
                Some(&snapshot.values)
            }
            _ => None,
        }
    }

    /// Prepare the slot for a lambda invocation and hand back *disjoint* mutable
    /// borrows of `state` and the output buffer — the lambda writes both at once,
    /// which a single whole-slot borrow couldn't allow. A resident buffer is reused
    /// **in place**, cleared to `Unbound`, and `resize`d to the current arity. Clearing
    /// prevents a skipped output from retaining a value produced by an earlier run.
    /// `produced_under` stays as-is until [`stamp_produced`](Self::stamp_produced)
    /// updates it on success.
    pub(crate) fn invoke_slot(&mut self, output_count: usize) -> InvokeSlot<'_> {
        match &mut self.value {
            ValueState::Resident { snapshot, .. } => snapshot.reset(output_count),
            _ => {
                self.value = ValueState::Resident {
                    snapshot: OutputSnapshot::empty(output_count),
                    produced_under: None,
                };
            }
        }
        let ValueState::Resident { snapshot, .. } = &mut self.value else {
            unreachable!("set to Resident just above");
        };
        InvokeSlot {
            state: &mut self.state,
            outputs: &mut snapshot.values,
        }
    }

    pub(crate) fn unbound_demanded_outputs(&self, demand: &[OutputDemand]) -> Vec<usize> {
        let ValueState::Resident { snapshot, .. } = &self.value else {
            panic!("a node's output must be resident immediately after invocation");
        };
        debug_assert_eq!(
            snapshot.values.len(),
            demand.len(),
            "node output values must match output demand arity"
        );
        demand
            .iter()
            .zip(&snapshot.values)
            .enumerate()
            .filter_map(|(output, (demand, value))| {
                (!demand.is_skip() && matches!(value, DynamicValue::Unbound)).then_some(output)
            })
            .collect()
    }

    /// Stamp the resident value with the node's current content digest on a successful
    /// run: `produced_under` turns it into a cache hit for the next run (RAM) and the
    /// key its disk blob is stored under.
    pub(crate) fn stamp_produced(&mut self) {
        let digest = self.current_digest;
        let ValueState::Resident { produced_under, .. } = &mut self.value else {
            panic!("a node's output must be resident when it is stamped produced");
        };
        *produced_under = digest;
    }
}

/// The per-node cross-run cache plus its disk backing. `slots` is keyed like
/// `program.e_nodes`; the resolver stamps each node's digest and decides cache reuse,
/// while the executor mutates outputs/state and consumes that decision. `disk_store`
/// persists outputs and serves them back; it is kept across graph updates while only `slots`
/// is reconciled or cleared.
#[derive(Default, Debug)]
pub(crate) struct RuntimeCache {
    pub(crate) slots: HashMap<ExecutionNodeId, RuntimeSlot>,
    pub(crate) disk_store: DiskStore,
}

#[derive(Debug)]
pub(crate) struct CacheRamStats {
    pub(crate) total: RamUsage,
    pub(crate) by_node: Vec<NodeRamUsage>,
}

impl RuntimeCache {
    pub(crate) fn clear(&mut self) {
        self.slots.clear();
    }

    /// The total and per-node RAM held by resident values. The global total deduplicates
    /// shared custom values by pointer identity, while each node reports the full size of
    /// every value it holds. `Empty` slots and zero-byte nodes are omitted.
    pub(crate) fn resident_ram_stats(&self) -> CacheRamStats {
        let mut seen: HashSet<*const ()> = HashSet::new();
        let mut total = RamUsage::default();
        let mut by_node = Vec::new();
        for (e_node_id, slot) in &self.slots {
            let ValueState::Resident { snapshot, .. } = &slot.value else {
                continue;
            };
            let mut node_usage = RamUsage::default();
            for value in &snapshot.values {
                let usage = value.ram_usage();
                node_usage += usage;
                let counts_toward_total = match value {
                    DynamicValue::Custom(arc) => seen.insert(Arc::as_ptr(arc) as *const ()),
                    _ => true,
                };
                if counts_toward_total {
                    total += usage;
                }
            }
            if node_usage.total() > 0 {
                by_node.push(NodeRamUsage {
                    e_node_id: *e_node_id,
                    usage: node_usage,
                });
            }
        }
        CacheRamStats { total, by_node }
    }

    /// Preserve surviving slots by id, default new nodes, and trim removed ones.
    pub(crate) fn reconcile(&mut self, program: &ExecutionProgram) {
        self.slots
            .retain(|e_node_id, _| program.e_nodes.contains_key(e_node_id));
        for e_node_id in program.e_nodes.keys().copied() {
            self.slots.entry(e_node_id).or_default();
        }
    }

    /// Whether `e_node_id` holds a *resident* output valid for its current digest:
    /// the value is in RAM and was produced under this digest. A `None` current
    /// digest (impure cone) never hits, and a value produced under a *different*
    /// digest (a changed input) misses too. The executor's input read and the
    /// disk-store rely on this being the true "bytes are here" predicate.
    pub(crate) fn is_resident_current(&self, e_node_id: ExecutionNodeId) -> bool {
        match (
            &self.slots[&e_node_id].value,
            self.slots[&e_node_id].current_digest,
        ) {
            (ValueState::Resident { produced_under, .. }, Some(d)) => *produced_under == Some(d),
            _ => false,
        }
    }

    pub(crate) fn is_resident_hit(
        &self,
        e_node_id: ExecutionNodeId,
        demand: &[OutputDemand],
    ) -> bool {
        match (
            &self.slots[&e_node_id].value,
            self.slots[&e_node_id].current_digest,
        ) {
            (
                ValueState::Resident {
                    produced_under,
                    snapshot,
                    ..
                },
                Some(d),
            ) => *produced_under == Some(d) && snapshot.covers_demand(demand),
            _ => false,
        }
    }

    /// Read producer `e_node_id`'s output `port` for a consumer: a clone of the value, or — with
    /// `take` — the value itself, moved out of the slot (leaving `Unbound`). The move is the
    /// executor's last-read fast path for a non-RAM producer: the RAM copy would be released
    /// right after anyway, and handing over the slot's copy leaves the consumer as the sole
    /// `Arc` holder so [`DynamicValue::into_custom`] can reuse the allocation in place.
    /// `None` when the slot holds no resident values.
    pub(crate) fn read_output_port(
        &mut self,
        program: &ExecutionProgram,
        e_node_id: ExecutionNodeId,
        port: usize,
        take: bool,
    ) -> Option<DynamicValue> {
        let arity = program.e_nodes[&e_node_id].outputs.len as usize;
        let ValueState::Resident { snapshot, .. } =
            &mut self.slots.get_mut(&e_node_id).unwrap().value
        else {
            return None;
        };
        debug_assert_eq!(snapshot.values.len(), arity);
        Some(if take {
            std::mem::take(&mut snapshot.values[port])
        } else {
            snapshot.values[port].clone()
        })
    }

    /// Clear a single output value of a resident slot (to `Unbound`), keeping its siblings — the
    /// mid-run per-output release for a non-RAM producer whose one output just went spent while
    /// others are still owed to other consumers.
    pub(crate) fn clear_output_port(&mut self, e_node_id: ExecutionNodeId, port: usize) {
        let ValueState::Resident { snapshot, .. } =
            &mut self.slots.get_mut(&e_node_id).unwrap().value
        else {
            panic!("an output can only be released from a resident slot");
        };
        debug_assert!(port < snapshot.values.len(), "output port must be in range");
        snapshot.values[port] = DynamicValue::Unbound;
    }

    /// Stamp `e_node_id`'s structural content digest into its slot. The producer-first resolver
    /// pass calls this before exact output demand is known; cache coverage is probed later by
    /// [`check_reuse`](Self::check_reuse).
    pub(crate) fn stamp_digest(
        &mut self,
        program: &ExecutionProgram,
        resource_stamps: &RunResourceStamps,
        e_node_id: ExecutionNodeId,
    ) {
        let digest = node_digest(program, e_node_id, self, resource_stamps);
        self.slots.get_mut(&e_node_id).unwrap().current_digest = digest;
    }

    /// Whether an unchanged output can satisfy this run's exact demand — already resident in
    /// RAM ([`is_resident_hit`](Self::is_resident_hit)) or successfully hydrated from disk. A
    /// `None` digest (an impure cone, or a bound resource not yet readable) never reuses.
    ///
    /// RAM reuse trusts residency ([`is_resident_hit`](Self::is_resident_hit)): a resident
    /// digest-valid value is served, because a content digest attests the value produced
    /// under it — however the value came to be resident (mode retention or a preview pin).
    /// Disk reuse stays gated on `persists_to_disk`
    /// (`Disk`/`Both`, enforced in [`DiskStore::blob_target`]).
    pub(crate) async fn check_reuse(
        &mut self,
        program: &ExecutionProgram,
        e_node_id: ExecutionNodeId,
        demand: &[OutputDemand],
    ) -> bool {
        if self.slots[&e_node_id].current_digest.is_none() {
            return false;
        }
        if self.is_resident_hit(e_node_id, demand) {
            return true;
        }
        let Some(target) = self.disk_store.blob_target(
            e_node_id,
            &program.e_nodes[&e_node_id],
            self.slots[&e_node_id].current_digest,
        ) else {
            return false;
        };
        let Some(snapshot) = self.disk_store.read(&target, demand).await else {
            return false;
        };
        self.slots.get_mut(&e_node_id).unwrap().value = ValueState::Resident {
            snapshot,
            produced_under: Some(target.digest),
        };
        true
    }

    /// Write `e_node_id`'s freshly-computed outputs to disk the moment it finishes (the executor
    /// calls this right after a successful invoke), so a long run's earlier caches are durable
    /// even if a later node errors or the run is cancelled. The target and output slice are
    /// snapshotted **synchronously**; only [`DiskStore::store`]'s write awaits, so the borrow
    /// across the await is just the value slice (`Sync`), never the whole cache.
    ///
    /// Only writes a value that matches the node's *current* digest
    /// ([`is_resident_hit`](Self::is_resident_hit)): a resident value produced under a superseded
    /// digest must not be stamped with — and overwrite — the new digest's blob. In the run loop
    /// the just-stamped value is always a current hit; this guards maintenance flushes when a
    /// disk store is attached.
    pub(crate) fn store_node<'a>(
        &'a self,
        program: &ExecutionProgram,
        e_node_id: ExecutionNodeId,
        ctx: &'a mut ContextManager,
    ) -> impl Future<Output = ()> + 'a {
        let target = self.disk_store.blob_target(
            e_node_id,
            &program.e_nodes[&e_node_id],
            self.slots[&e_node_id].current_digest,
        );
        let resident = self.is_resident_current(e_node_id).then(|| {
            let ValueState::Resident { snapshot, .. } = &self.slots[&e_node_id].value else {
                unreachable!("a current resident slot must contain resident values")
            };
            snapshot
        });
        let disk = &self.disk_store;
        async move {
            let (Some(target), Some(snapshot)) = (target, resident) else {
                return;
            };
            disk.store(&target, snapshot, ctx).await;
        }
    }

    /// After a run, release any non-RAM outputs that were not spent during execution.
    pub(crate) fn release_non_ram_outputs(&mut self, program: &ExecutionProgram) {
        for e_node_id in program.e_nodes.keys() {
            if program.e_nodes[e_node_id].cache.caches_in_ram()
                || self.slots[e_node_id].output_values().is_none()
            {
                continue;
            }
            self.slots.get_mut(e_node_id).unwrap().clear_output();
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    pub(crate) fn hydrate(
        cache: &mut RuntimeCache,
        e_node_id: ExecutionNodeId,
        snapshot: OutputSnapshot,
        digest: Digest,
    ) {
        cache.slots.get_mut(&e_node_id).unwrap().value = ValueState::Resident {
            snapshot,
            produced_under: Some(digest),
        };
    }
}

#[cfg(test)]
mod tests;
