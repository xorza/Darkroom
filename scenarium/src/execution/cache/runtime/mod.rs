//! The cross-run runtime cache: the per-node RAM slots (output values + content digests +
//! node state, index-aligned to the installed program) **plus** the
//! [`DiskStore`] backing them, and the caching policy over the two — reuse detection (the
//! header-only [`probe_reuse`](RuntimeCache::probe_reuse)), frontier hydration (the decode,
//! deferred to the node's turn in the run loop), persistence, and RAM eviction. Owned by the
//! [`ExecutionEngine`](crate::execution::engine::ExecutionEngine); the executor's run loop drives
//! it a node at a time. The [`DiskStore`] is pure blob I/O and knows nothing of the cache; this type
//! reads a node's digest/value-state off its slot and the blob off disk, and pushes the result
//! back — so RAM eviction lives here, on the cache that owns both stores.
//! Per-run results (errors, timings) are *not* here — they belong to a single run, not the cache.

use std::collections::HashSet;
use std::future::Future;
use std::sync::Arc;

use hashbrown::HashMap;

use crate::execution::cache::slot::{RuntimeSlot, StateOwner, ValueState};
use crate::execution::digest::node_digest;
use crate::execution::disk_store::{BlobTarget, DiskStore, StorePolicy};
use crate::execution::identity::ExecutionNodeId;
use crate::execution::outcome::NodeRamUsage;
use crate::execution::program::ExecutionProgram;
use crate::execution::program::index::{NodeColumn, NodeIdx, OutputAddr};
use crate::execution::resource::RunResourceStamps;
use crate::node::definition::FuncBehavior;
use crate::node::lambda::OutputDemand;
use crate::runtime::context::ContextStore;
use crate::{DynamicValue, RamUsage};

/// The per-node cross-run cache plus its disk backing. `slots` is a
/// [`NodeColumn`] aligned to the installed program, so every run-loop access is
/// an array read; cross-install survival happens at [`reconcile`](Self::reconcile),
/// which re-pairs the slots with the new index order by stable id. The resolver stamps
/// each node's digest and decides cache reuse,
/// while the executor mutates outputs/state and consumes that decision. `disk_store`
/// persists outputs and serves them back; it is kept across graph updates while only `slots`
/// is reconciled or cleared.
#[derive(Default, Debug)]
pub(crate) struct RuntimeCache {
    pub(crate) slots: NodeColumn<RuntimeSlot>,
    /// The ids `slots` is currently paired with, so the cache can interpret its
    /// own state: reconciling to a new program needs no memory of the previous
    /// one, and [`validate_installed`](crate::execution::compile::CompiledGraph::validate_installed)
    /// can compare alignment element-wise instead of by length.
    pub(crate) e_node_ids: NodeColumn<ExecutionNodeId>,
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
        self.e_node_ids.clear();
    }

    /// Append the slot for the next dense index, paired with the id it belongs
    /// to. The only way slots enter the column, so the two stay aligned.
    fn push_slot(&mut self, e_node_id: ExecutionNodeId, slot: RuntimeSlot) {
        self.e_node_ids.push(e_node_id);
        self.slots.push(slot);
    }

    pub(crate) async fn evict(
        &mut self,
        program: &ExecutionProgram,
        e_node_ids: &[ExecutionNodeId],
    ) -> Vec<CacheEvictionFailure> {
        let mut failures = Vec::new();
        for e_node_id in e_node_ids {
            let node_idx = *program
                .e_node_index
                .get(e_node_id)
                .expect("an eviction target belongs to the installed program");
            match self.disk_store.remove_node(*e_node_id).await {
                Ok(()) => self.slots[node_idx].clear_output(),
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
        for (e_node_id, slot) in self.e_node_ids.iter().zip(self.slots.iter()) {
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

    /// Realign the slots to a newly installed program: re-pair the slots the
    /// cache is currently holding with the new index order by stable id —
    /// dropping persistent state whose owning implementation (func id +
    /// version) changed — default new nodes, trim removed ones, and apply the
    /// installed program's RAM-retention policy immediately. The one place ids
    /// are hashed for slot access; every per-run access is an index read.
    pub(crate) fn reconcile(&mut self, program: &ExecutionProgram) {
        let mut retained: HashMap<ExecutionNodeId, RuntimeSlot> =
            self.e_node_ids.drain().zip(self.slots.drain()).collect();
        for (e_node_id, e_node) in program.e_node_ids.iter().zip(program.e_nodes.iter()) {
            let owner = StateOwner {
                func_id: e_node.func_id,
                version: e_node.version,
            };
            let slot = match retained.remove(e_node_id) {
                Some(mut slot) => {
                    slot.reown(owner);
                    slot
                }
                None => RuntimeSlot {
                    owner,
                    ..Default::default()
                },
            };
            self.push_slot(*e_node_id, slot);
        }
        self.release_dead_outputs(program);
    }

    /// Whether `e_node_id` holds a *resident* output valid for its current digest:
    /// the value is in RAM and was produced under this digest. A `None` current
    /// digest (impure cone) never hits, and a value produced under a *different*
    /// digest (a changed input) misses too. The executor's input read and the
    /// disk-store rely on this being the true "bytes are here" predicate.
    pub(crate) fn is_resident_current(&self, node_idx: NodeIdx) -> bool {
        match (
            &self.slots[node_idx].value,
            self.slots[node_idx].current_digest,
        ) {
            (ValueState::Resident { produced_under, .. }, Some(d)) => *produced_under == Some(d),
            _ => false,
        }
    }

    fn is_resident_hit(&self, node_idx: NodeIdx, demand: &[OutputDemand]) -> bool {
        match (
            &self.slots[node_idx].value,
            self.slots[node_idx].current_digest,
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

    /// Read a producer output for a consumer: a clone of the value, or — with
    /// `take` — the value itself, moved out of the slot (leaving `Unbound`). The move is the
    /// executor's last-read fast path for a non-RAM producer: the RAM copy would be released
    /// right after anyway, and handing over the slot's copy leaves the consumer as the sole
    /// `Arc` holder so [`DynamicValue::into_custom`] can reuse the allocation in place.
    /// `None` when the slot holds no resident values.
    pub(crate) fn read_output_port(
        &mut self,
        program: &ExecutionProgram,
        address: OutputAddr,
        take: bool,
    ) -> Option<DynamicValue> {
        let arity = program[address.node_idx].outputs.len as usize;
        let ValueState::Resident { snapshot, .. } = &mut self.slots[address.node_idx].value else {
            return None;
        };
        debug_assert_eq!(snapshot.values.len(), arity);
        Some(if take {
            std::mem::take(&mut snapshot.values[address.port_idx as usize])
        } else {
            snapshot.values[address.port_idx as usize].clone()
        })
    }

    /// Clear a single output value of a resident slot (to `Unbound`), keeping its siblings — the
    /// mid-run per-output release for a non-RAM producer whose one output just went spent while
    /// others are still owed to other consumers.
    pub(crate) fn clear_output_port(&mut self, address: OutputAddr) {
        let ValueState::Resident { snapshot, .. } = &mut self.slots[address.node_idx].value else {
            panic!("an output can only be released from a resident slot");
        };
        debug_assert!(
            (address.port_idx as usize) < snapshot.values.len(),
            "output port must be in range"
        );
        snapshot.values[address.port_idx as usize] = DynamicValue::Unbound;
    }

    /// Stamp `e_node_id`'s structural content digest into its slot. The producer-first resolver
    /// pass calls this before exact output demand is known; cache coverage is probed later by
    /// [`probe_reuse`](Self::probe_reuse).
    pub(crate) fn stamp_digest(
        &mut self,
        program: &ExecutionProgram,
        resource_stamps: &RunResourceStamps,
        node_idx: NodeIdx,
    ) {
        let digest = node_digest(program, node_idx, self, resource_stamps);
        self.slots[node_idx].current_digest = digest;
    }

    /// Blobs are named by stable id, so they survive installs that shift indices.
    fn blob_target(&self, program: &ExecutionProgram, node_idx: NodeIdx) -> Option<BlobTarget> {
        self.disk_store.blob_target(
            program.e_node_ids[node_idx],
            &program[node_idx],
            self.slots[node_idx].current_digest,
        )
    }

    /// Whether an unchanged output can satisfy this run's exact demand — the verdict alone,
    /// **without loading anything**, so the resolver can cut a producer cone without paying
    /// for a decode. A `None` digest (an impure cone, or a bound path not yet readable)
    /// never reuses.
    ///
    /// RAM reuse trusts residency ([`is_resident_hit`](Self::is_resident_hit)): a resident
    /// digest-valid value is served, because a content digest attests the value produced
    /// under it — however the value came to be resident (mode retention or a preview pin).
    /// Disk reuse stays gated on `persists_to_disk` (`Disk`/`Both`, enforced in
    /// [`DiskStore::blob_target`]) and is answered from the blob header alone.
    ///
    /// Takes `&mut self` without mutating anything: the slots hold `Send`-but-not-`Sync`
    /// node state, so a *shared* cache borrow held across this await would make the whole
    /// worker future non-`Send`.
    pub(crate) async fn probe_reuse(
        &mut self,
        program: &ExecutionProgram,
        node_idx: NodeIdx,
        demand: &[OutputDemand],
    ) -> bool {
        if self.slots[node_idx].current_digest.is_none() {
            return false;
        }
        if self.is_resident_hit(node_idx, demand) {
            return true;
        }
        let Some(target) = self.blob_target(program, node_idx) else {
            return false;
        };
        self.disk_store.covers_demand(&target, demand).await
    }

    /// [`probe_reuse`](Self::probe_reuse)'s verdict, but leaving the value readable by the
    /// node's consumers: a resident one needs no load, and a blob is decoded into the slot
    /// here. The run loop calls this when it *reaches* a reuse rather than the resolver when
    /// it probes, so frontier decodes interleave with execution instead of accumulating
    /// ahead of the first lambda, and a reused value lives exactly as long as a freshly
    /// computed one — released by the same last-read bookkeeping.
    ///
    /// `false` when nothing loads: no usable blob, or one that stopped decoding since the
    /// probe. The decode path deletes an undecodable blob, so the next run misses cleanly
    /// and recomputes.
    pub(crate) async fn hydrate_reuse(
        &mut self,
        program: &ExecutionProgram,
        node_idx: NodeIdx,
        demand: &[OutputDemand],
        ctx: &mut ContextStore,
    ) -> bool {
        if self.is_resident_hit(node_idx, demand) {
            return true;
        }
        let Some(target) = self.blob_target(program, node_idx) else {
            return false;
        };
        let Some(snapshot) = self.disk_store.read(&target, demand, ctx).await else {
            return false;
        };
        self.slots[node_idx].value = ValueState::Resident {
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
        node_idx: NodeIdx,
        policy: StorePolicy,
        ctx: &'a mut ContextStore,
    ) -> impl Future<Output = ()> + 'a {
        let target = self.blob_target(program, node_idx);
        let resident = self.is_resident_current(node_idx).then(|| {
            let ValueState::Resident { snapshot, .. } = &self.slots[node_idx].value else {
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
        for (node_idx, e_node) in program.e_nodes.iter_indexed() {
            let Some(resident_len) = self.slots[node_idx].output_values().map(Vec::len) else {
                continue;
            };
            // A snapshot holding a different number of values cannot
            // describe this node's outputs, whatever its digest says.
            //
            // The digest is the only other thing keeping a snapshot
            // alive, and it does not have to move: a func that grows an
            // output while keeping its id *and* version reuses the flat
            // node, so `reown` sees no owner change and the stale
            // `produced_under` still equals the stale `current_digest`.
            // Both retention checks passed, and the mismatch surfaced
            // only at install validation — a debug panic, and in release
            // a snapshot indexed by port positions it no longer has.
            let retained = resident_len == e_node.outputs.len as usize
                && e_node.cache.caches_in_ram()
                && e_node.behavior == FuncBehavior::Pure
                && self.is_resident_current(node_idx);
            if !retained {
                self.slots[node_idx].clear_output();
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::execution::cache::runtime::RuntimeCache;
    use crate::execution::cache::slot::{OutputSnapshot, ValueState};
    use crate::execution::digest::Digest;
    use crate::execution::program::index::NodeIdx;

    pub(crate) fn hydrate(
        cache: &mut RuntimeCache,
        node_idx: NodeIdx,
        snapshot: OutputSnapshot,
        digest: Digest,
    ) {
        cache.slots[node_idx].value = ValueState::Resident {
            snapshot,
            produced_under: Some(digest),
        };
    }
}

#[cfg(test)]
mod tests;
