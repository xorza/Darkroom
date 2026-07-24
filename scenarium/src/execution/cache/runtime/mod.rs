//! The cross-run runtime cache: the per-node RAM slots (output values + content digests +
//! node state, keyed by `ExecutionNodeId`) **plus** the
//! [`DiskStore`] backing them, and the caching policy over the two — reuse detection, frontier
//! hydration, persistence, and RAM eviction. Owned by the
//! [`ExecutionEngine`](crate::execution::engine::ExecutionEngine); the executor's run loop drives
//! it a node at a time. The [`DiskStore`] is pure blob I/O and knows nothing of the cache; this type
//! reads a node's digest/value-state off its slot and the blob off disk, and pushes the result
//! back — so RAM eviction lives here, on the cache that owns both stores.
//! Per-run results (errors, timings) are *not* here — they belong to a single run, not the cache.

use std::collections::HashSet;
use std::future::Future;
use std::sync::Arc;

use hashbrown::HashMap;

#[cfg(test)]
use crate::execution::cache::slot::OutputSnapshot;
use crate::execution::cache::slot::{RuntimeSlot, ValueState};
#[cfg(test)]
use crate::execution::digest::Digest;
use crate::execution::digest::node_digest;
use crate::execution::disk_store::{DiskStore, StorePolicy};
use crate::execution::identity::ExecutionNodeId;
use crate::execution::outcome::NodeRamUsage;
use crate::execution::program::ExecutionProgram;
use crate::execution::resource::RunResourceStamps;
use crate::node::definition::FuncBehavior;
use crate::node::lambda::OutputDemand;
use crate::runtime::context::ContextManager;
use crate::{DynamicValue, RamUsage};

#[cfg(test)]
mod tests;

/// The per-node cross-run cache plus its disk backing. `slots` is keyed like
/// `program.e_nodes`; the resolver stamps each node's digest and decides cache reuse,
/// while the executor mutates outputs/state and consumes that decision. `disk_store`
/// persists outputs and serves them back; it is kept across graph updates while only `slots`
/// is reconciled or cleared.
#[derive(Default, Debug)]
pub(crate) struct RuntimeCache {
    pub(crate) slots: HashMap<ExecutionNodeId, RuntimeSlot>,
    pub(crate) disk_store: DiskStore,
    ram_seen: HashSet<usize>,
}

#[derive(Debug)]
pub(crate) struct CacheEvictionFailure {
    pub(crate) e_node_id: ExecutionNodeId,
    pub(crate) message: String,
}

impl RuntimeCache {
    pub(crate) fn clear(&mut self) {
        self.slots.clear();
    }

    pub(crate) async fn evict(
        &mut self,
        e_node_ids: &[ExecutionNodeId],
    ) -> Vec<CacheEvictionFailure> {
        let mut failures = Vec::new();
        for e_node_id in e_node_ids {
            match self.disk_store.remove_node(*e_node_id).await {
                Ok(()) => {
                    self.slots
                        .get_mut(e_node_id)
                        .expect("an eviction target belongs to the installed program")
                        .clear_output();
                }
                Err(error) => failures.push(CacheEvictionFailure {
                    e_node_id: *e_node_id,
                    message: error.to_string(),
                }),
            }
        }
        failures
    }

    /// The total and per-node RAM held by resident values. The global total deduplicates
    /// shared custom values by pointer identity, while each node reports the full size of
    /// every value it holds. `Empty` slots and zero-byte nodes are omitted.
    pub(crate) fn resident_ram_stats(&mut self, by_node: &mut Vec<NodeRamUsage>) -> RamUsage {
        self.ram_seen.clear();
        by_node.clear();
        let mut total = RamUsage::default();
        for (e_node_id, slot) in &self.slots {
            let ValueState::Resident { snapshot, .. } = &slot.value else {
                continue;
            };
            let mut node_usage = RamUsage::default();
            for value in &snapshot.values {
                let usage = value.ram_usage();
                node_usage += usage;
                let counts_toward_total = match value {
                    DynamicValue::Custom(arc) => {
                        self.ram_seen.insert(Arc::as_ptr(arc) as *const () as usize)
                    }
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
        total
    }

    /// Preserve surviving slots by id, default new nodes, trim removed ones, and apply the
    /// installed program's RAM-retention policy immediately.
    pub(crate) fn reconcile(&mut self, program: &ExecutionProgram) {
        self.slots
            .retain(|e_node_id, _| program.e_nodes.contains_key(e_node_id));
        for e_node_id in program.e_nodes.keys().copied() {
            self.slots.entry(e_node_id).or_default();
        }
        self.release_dead_outputs(program);
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
    /// even if a later node errors or the run is cancelled. [`StorePolicy::KnownMiss`] publishes
    /// directly after resolution proved reuse impossible; [`StorePolicy::PreserveCovering`]
    /// first protects a broader blob when a maintenance flush has no such verdict. The target
    /// and output slice are snapshotted **synchronously**; the borrow across the store await is
    /// just the value slice (`Sync`), never the whole cache.
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
        policy: StorePolicy,
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
            disk.store(&target, snapshot, policy, ctx).await;
        }
    }

    /// Release resident values that cannot be a future RAM hit under the installed program.
    /// Called both when a program is installed and after each run, so cache-mode downgrades,
    /// impure outputs, and superseded snapshots do not wait for another execution to free RAM.
    pub(crate) fn release_dead_outputs(&mut self, program: &ExecutionProgram) {
        for e_node_id in program.e_nodes.keys() {
            let e_node = &program.e_nodes[e_node_id];
            let retained = e_node.cache.caches_in_ram()
                && e_node.behavior == FuncBehavior::Pure
                && self.is_resident_current(*e_node_id);
            if retained || self.slots[e_node_id].output_values().is_none() {
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
