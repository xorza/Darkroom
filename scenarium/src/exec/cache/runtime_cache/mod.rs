//! The cross-run runtime cache: the per-node RAM slots (output values + content digests,
//! index-aligned to the installed program) plus the [`DiskStore`] backing them, and the caching
//! policy over the two — reuse detection (the header-only
//! [`probe_reuse`](RuntimeCache::probe_reuse)), frontier hydration (the decode, deferred to the
//! node's turn in the run loop), persistence, and RAM eviction. Owned by the
//! [`ExecEngine`](crate::exec::engine::ExecEngine). The [`DiskStore`] is pure blob I/O and knows
//! nothing of the cache, so RAM eviction lives here, on the cache that owns both stores. Per-run
//! results (errors, timings) are not here.

pub(crate) mod cache_flush_report;
pub(crate) mod error;

use std::cmp::Reverse;
use std::iter;
use std::ops::Index;
use std::sync::Arc;

use common::CancelToken;
use hashbrown::hash_map::Entry;
use hashbrown::{HashMap, HashSet};

use crate::containers::col::Col;
use crate::data::dynamic_value::DynamicValue;
use crate::data::ram_usage::RamUsage;
use crate::exec::cache::digest::{DOMAIN, Digest, DigestHasher, InputTag};
use crate::exec::cache::disk_store::blob_target::BlobTarget;
use crate::exec::cache::disk_store::error::{PruneError, StoreResult};
use crate::exec::cache::disk_store::store_outcome::StoreOutcome;
use crate::exec::cache::disk_store::{DiskStore, StorePolicy};
use crate::exec::cache::path_identities::PathIdentities;
use crate::exec::cache::runtime_cache::cache_flush_report::CacheFlushReport;
use crate::exec::cache::runtime_cache::error::{CacheFlushUnsupported, CacheNodeFailure};
use crate::exec::cache::runtime_slot::{OutputRead, RuntimeSlot, Stamp};
use crate::exec::cache::stamp_job::error::StampError;
use crate::exec::compile::compiled_graph::{CompiledGraph, ExecBinding};
use crate::exec::compile::consumer_cone::ConsumerCone;
use crate::exec::idx::{NodeIdx, OutputAddr};
use crate::exec::schedule::{Delivery, RunSchedule};
use crate::graph::func::FuncBehavior;
use crate::graph::func::lambda::OutputDemand;
use crate::graph::node::Keep;
use crate::graph::program_id::ProgramId;

/// The per-node cross-run cache plus its disk backing. `slots` is a [`Col`] aligned to the
/// installed program; [`reconcile`](Self::reconcile) re-pairs it with a new index order by stable
/// id.
///
/// The program is not held here — it arrives with each call, since a `NodeIdx` means nothing
/// without it. [`ExecEngine`](crate::exec::engine::ExecEngine) owns both halves and is the only
/// caller of [`reconcile`](Self::reconcile), so the pairing is established by the owner rather
/// than validated by every method. `disk_store` is kept across graph updates; only `slots` is
/// reconciled or cleared.
#[derive(Default, Debug)]
pub(crate) struct RuntimeCache {
    /// Private: its length is the alignment invariant [`reconcile`](Self::reconcile)
    /// establishes, so nothing outside may push, drain, or resize it.
    slots: Col<NodeIdx, RuntimeSlot>,
    /// Private so that replacing it goes through [`set_disk_store`](Self::set_disk_store).
    disk_store: DiskStore,
    /// What each path this run reads was, identified once; the digest fold reads it beside the
    /// slots.
    paths: PathIdentities,
    /// Buffers for an eviction's downstream walk, held to keep their allocations.
    cone: ConsumerCone,
    /// Shared values [`measure_resident_ram`](Self::measure_resident_ram) has already counted, by
    /// pointer identity. Scratch, kept only so the allocation survives.
    ram_seen: HashSet<usize>,
    /// The per-node breakdown of the same walk, read through [`node_ram`](Self::node_ram). An
    /// answer, not scratch, and undeduplicated: a node reports everything it holds whether or not
    /// a neighbour holds it too.
    node_ram: Col<NodeIdx, RamUsage>,
    /// The first node of a run under each digest, while [`mark_leaders`](Self::mark_leaders)
    /// walks it. Scratch, kept only so the allocation survives; it holds one entry per node at
    /// most.
    leaders: HashMap<Digest, NodeIdx>,
    /// Each node's earlier node of this run under its digest, which it may take its values from:
    /// what [`mark_leaders`](Self::mark_leaders) answers and the sweep reads, aligned like the
    /// slots.
    twin_leaders: Col<NodeIdx, Option<NodeIdx>>,
    /// The values [`relieve`](Self::relieve) may drop, in the order it drops them. Scratch, kept
    /// only so the allocation survives; it holds one entry per node at most.
    droppable: Vec<Droppable>,
}

/// One resident value memory pressure may drop: when it was last used, and the RAM it holds.
#[derive(Clone, Copy, Debug)]
struct Droppable {
    node_idx: NodeIdx,
    used_in: u64,
    bytes: usize,
}

/// Where a reuse would come from, if one is possible at all — the front half of
/// [`probe_reuse`](RuntimeCache::probe_reuse) and [`hydrate_reuse`](RuntimeCache::hydrate_reuse),
/// so the verdict and the load that acts on it name *one* blob. The blob can still change between
/// the two (header checked at the verdict, body read later, no lock), which is what
/// [`RunError::CacheLoadFailed`](crate::exec::error::RunError) exists for.
#[derive(Debug)]
enum ReuseSource {
    /// Already in RAM under the node's current digest, covering the demand.
    Resident,
    /// Not in RAM; this blob is the only candidate.
    Blob(BlobTarget),
}

/// What [`hydrate_reuse`](RuntimeCache::hydrate_reuse) left behind — whether the
/// node's consumers can read a value without it running.
///
/// A named pair rather than a `bool` because the callers read `Missed` oppositely: at a `Reuse`
/// node the cut already pruned its producers, so it fails the node; at a re-stamped `Run` node it
/// is an ordinary miss and the lambda runs.
#[derive(Debug)]
pub(crate) enum ReuseOutcome {
    /// A value is resident under the node's current digest and ready to read —
    /// it was already there, or a blob decoded into the slot.
    Served,
    /// Nothing to serve: no usable source, or a blob that stopped decoding
    /// since the probe.
    Missed,
}

/// One node reached by the run loop with no digest yet — what
/// [`restamp_and_hydrate`](RuntimeCache::restamp_and_hydrate) re-keys and,
/// on a hit, serves.
#[derive(Debug)]
pub(crate) struct Restamp<'a> {
    pub(crate) program: &'a CompiledGraph,
    /// The run's schedule, which decides what each input delivers.
    pub(crate) schedule: &'a RunSchedule,
    pub(crate) node_idx: NodeIdx,
    pub(crate) demand: &'a [OutputDemand],
    pub(crate) cancel: CancelToken,
}

impl Index<NodeIdx> for RuntimeCache {
    type Output = RuntimeSlot;

    fn index(&self, node_idx: NodeIdx) -> &RuntimeSlot {
        &self.slots[node_idx]
    }
}

impl RuntimeCache {
    /// The slot of `node_idx`, to change through its own methods.
    pub(crate) fn slot_mut(&mut self, node_idx: NodeIdx) -> &mut RuntimeSlot {
        &mut self.slots[node_idx]
    }

    /// The span of the slot column — the program node count it is aligned to.
    pub(crate) const fn slot_count(&self) -> usize {
        self.slots.len()
    }

    /// Attach the store this cache persists to and serves from. Must be called between runs:
    /// every reuse verdict already given was answered from the previous store. A write that
    /// failed is owed again, as the new store may take it.
    pub(crate) fn set_disk_store(&mut self, disk_store: DiskStore) {
        self.disk_store = disk_store;
        for slot in self.slots.iter_mut() {
            slot.reconsider_blob();
        }
    }

    pub(crate) fn clear(&mut self) {
        self.slots.clear();
        self.paths.clear();
    }

    /// Drop `seeds` and everything downstream of them from RAM and disk.
    ///
    /// The cone, not the seeds alone: a surviving consumer reuses and prunes its producers, so
    /// evicting one node in isolation would change nothing a later run does. A seed absent from
    /// the program is skipped.
    pub(crate) async fn evict(
        &mut self,
        program: &CompiledGraph,
        seeds: impl IntoIterator<Item = ProgramId>,
    ) -> Vec<CacheNodeFailure> {
        let downstream = self.cone.of(
            program,
            seeds
                .into_iter()
                .filter_map(|program_id| program.node(program_id)),
        );
        let removals = self
            .disk_store
            .remove_nodes(
                downstream
                    .into_iter()
                    .map(|node_idx| program.program_ids[node_idx])
                    .collect(),
            )
            .await;
        let mut failures = Vec::new();
        for (node_idx, removal) in downstream.into_iter().zip(removals) {
            match removal {
                // The removal just made the belief that a blob backed the value false.
                Ok(()) => self.slots[node_idx].clear_output(),
                Err(error) => failures.push(CacheNodeFailure {
                    program_id: program.program_ids[node_idx],
                    cause: error.into(),
                }),
            }
        }
        failures
    }

    /// Remove the blobs no node of `program` names, and what writes left behind; see
    /// [`DiskStore::prune`].
    pub(crate) async fn prune_store(&self, program: &CompiledGraph) -> Vec<PruneError> {
        self.disk_store
            .prune(program.program_ids.iter().copied())
            .await
    }

    /// Measure the RAM held by resident values: the global total, returned, and the per-node
    /// breakdown, left in [`node_ram`](Self::node_ram). The total deduplicates shared custom
    /// values by pointer identity, while each node reports the full size of every value it
    /// holds. `Empty` slots read as zero. The breakdown is dense, aligned like the slots.
    pub(crate) fn measure_resident_ram(&mut self) -> RamUsage {
        self.ram_seen.clear();
        self.node_ram.reset(self.slots.len(), RamUsage::default());
        let mut total = RamUsage::default();
        for (node_idx, slot) in self.slots.iter_indexed() {
            let Some(values) = slot.output_values() else {
                continue;
            };
            let mut node_usage = RamUsage::default();
            for value in values {
                let usage = value.ram_usage();
                node_usage += usage;
                let counts_toward_total = match value {
                    DynamicValue::Custom(arc) => {
                        self.ram_seen.insert(Arc::as_ptr(arc).cast::<()>() as usize)
                    }
                    _ => true,
                };
                if counts_toward_total {
                    total += usage;
                }
            }
            self.node_ram[node_idx] = node_usage;
        }
        total
    }

    /// The per-node breakdown the last
    /// [`measure_resident_ram`](Self::measure_resident_ram) left behind, spanning the
    /// program the slots are aligned to. Empty until the first measurement.
    pub(crate) const fn node_ram(&self) -> &Col<NodeIdx, RamUsage> {
        &self.node_ram
    }

    /// Drop values kept while memory allows ([`Keep::Auto`])
    /// until they held at least `deficit` bytes of CPU memory: the least recently used first, and
    /// among those used in one run the largest first, so the fewest values go. As ComfyUI's
    /// default cache drops what it holds once free RAM falls under a headroom
    /// ([startup flags](https://docs.comfy.org/development/comfyui-server/startup-flags)) and Nuke's
    /// buffer cache the oldest first. A value kept always is never dropped, and one kept never is
    /// already gone, as is one `droppable` refuses, given the node and its slot: a value a later
    /// step of the running run still serves or reads. Returns the bytes the dropped values held.
    ///
    /// A value another slot shares counts in full here though dropping it frees nothing while the
    /// other holds it: the drop can fall short, and the next reading of memory drops more.
    pub(crate) fn relieve(
        &mut self,
        program: &CompiledGraph,
        deficit: u64,
        droppable: impl Fn(NodeIdx, &RuntimeSlot) -> bool,
    ) -> u64 {
        self.droppable.clear();
        for (node_idx, e_node) in program.e_nodes.iter_indexed() {
            let slot = &self.slots[node_idx];
            if e_node.cache.ram != Keep::Auto || !droppable(node_idx, slot) {
                continue;
            }
            let Some(values) = slot.output_values() else {
                continue;
            };
            let bytes = values.iter().map(|value| value.ram_usage().cpu).sum();
            if bytes > 0 {
                self.droppable.push(Droppable {
                    node_idx,
                    used_in: slot.used_in(),
                    bytes,
                });
            }
        }
        self.droppable
            .sort_unstable_by_key(|item| (item.used_in, Reverse(item.bytes), item.node_idx));
        let mut freed = 0_u64;
        for item in &self.droppable {
            if freed >= deficit {
                break;
            }
            self.slots[item.node_idx].clear_output();
            freed += u64::try_from(item.bytes).expect("a value's size fits in u64");
        }
        freed
    }

    /// Realign the slots from the program they currently belong to onto a newly installed one
    /// ([`CompiledGraph::realign`]): re-pair them by stable id, default new nodes, trim removed
    /// ones, and apply the installed program's RAM-retention policy immediately.
    ///
    /// `prev` is the program the slots currently belong to — the empty one before anything was
    /// installed. [`ExecEngine::install`](crate::exec::engine::ExecEngine) holds both at the swap.
    pub(crate) fn reconcile(&mut self, prev: &CompiledGraph, program: &CompiledGraph) {
        program.realign(
            prev,
            &mut self.slots,
            |_, mut slot| {
                // An unwritable verdict was the previous program's codecs', and a failed write
                // is tried again by the program the host just installed.
                slot.reconsider_blob();
                slot
            },
            |_| RuntimeSlot::default(),
        );
        self.release_dead_outputs(program);
    }

    pub(crate) fn is_resident_cur(&self, node_idx: NodeIdx) -> bool {
        self.slots[node_idx].cur_snapshot().is_some()
    }

    /// Current *and* holding every output this run demands.
    fn is_resident_hit(&self, node_idx: NodeIdx, demand: &[OutputDemand]) -> bool {
        self.slots[node_idx]
            .cur_snapshot()
            .is_some_and(|snapshot| snapshot.covers_demand(demand))
    }

    /// [`RuntimeSlot::read_output`] addressed by [`OutputAddr`], with the one check
    /// the slot cannot make itself: that what it holds still has the arity the
    /// installed program declares.
    pub(crate) fn read_output_port(
        &mut self,
        program: &CompiledGraph,
        address: OutputAddr,
        read: OutputRead,
    ) -> Option<DynamicValue> {
        let arity = program[address.node_idx].outputs.len();
        let slot = &mut self.slots[address.node_idx];
        debug_assert!(
            slot.output_values()
                .is_none_or(|values| values.len() == arity),
            "cached output values must match the node's compiled arity"
        );
        slot.read_output(address.port_idx, read)
    }

    /// Producer-first stamp pass over the nodes `schedule` runs ([`RunSchedule::executing`]), so
    /// a consumer folds an already-stamped producer digest. Reuse is not probed here because exact
    /// demand exists only in the resolver's reverse sweep. A Bind-delivered path value that is not
    /// resident yet stamps [`Stamp::Deferred`]; the run loop stamps again once its path producer
    /// settles.
    pub(crate) fn stamp_digests(&mut self, program: &CompiledGraph, schedule: &RunSchedule) {
        for node_idx in schedule.executing() {
            self.stamp_digest(program, schedule, node_idx);
        }
    }

    /// Name each node the run may serve from an earlier node of this run with its digest: equal
    /// digests mean the same computation (see [`digest`](crate::exec::cache::digest)), so the
    /// second need not run. Walked producer-first, so the first under a digest leads. A node with
    /// no key, a sink, whose run is its effect, a node seeded to run, and an event source, which
    /// must run to start its events, neither leads nor follows. The answer is
    /// [`twin_leader`](Self::twin_leader)'s, which the sweep reads.
    ///
    /// Values are shared in RAM within the run alone; a blob stays named by its own node.
    pub(crate) fn mark_leaders(&mut self, program: &CompiledGraph, schedule: &RunSchedule) {
        self.leaders.clear();
        self.twin_leaders.reset(program.e_nodes.len(), None);
        for node_idx in schedule.executing() {
            let flags = schedule.root_flags(node_idx);
            let Stamp::Keyed(digest) = self.slots[node_idx].stamp() else {
                continue;
            };
            if program[node_idx].sink || flags.is_seeded() || flags.is_event_source() {
                continue;
            }
            self.twin_leaders[node_idx] = match self.leaders.entry(digest) {
                Entry::Occupied(leader) => Some(*leader.get()),
                Entry::Vacant(free) => {
                    free.insert(node_idx);
                    None
                }
            };
        }
    }

    /// The earlier node of this run with `node_idx`'s digest, whose values it may take.
    pub(crate) fn twin_leader(&self, node_idx: NodeIdx) -> Option<NodeIdx> {
        self.twin_leaders[node_idx]
    }

    /// Stamp one node's key into its slot; cache coverage is probed later by
    /// [`probe_reuse`](Self::probe_reuse).
    pub(crate) fn stamp_digest(
        &mut self,
        program: &CompiledGraph,
        schedule: &RunSchedule,
        node_idx: NodeIdx,
    ) {
        // Folded whole first, so the read of the slots ends before the slot is borrowed mutably.
        let stamp = self.node_digest(program, schedule, node_idx);
        self.slots[node_idx].set_stamp(stamp);
    }

    /// A node's **content digest**, the one key it is cached under: its identity (func id + port
    /// signature) plus its structural inputs. RAM reuse
    /// ([`is_resident_hit`](Self::is_resident_hit)),
    /// disk load/store, and downstream folding all read the stamp. Computed producer-first, so a
    /// `Wire` producer is already stamped.
    ///
    /// - An **`Impure`** node is [`Stamp::Unkeyed`]: it never caches, and neither does a node
    ///   reading an unkeyed producer.
    /// - Otherwise fold what each input delivers ([`RunSchedule::delivery`]): a `Const`'s value +
    ///   prepared `FsPath` content, a `Wire` producer's digest, or the unbound marker — plus, for
    ///   a resource-typed input, the live identity of the referent behind the *delivered* value
    ///   ([`hash_bound_fs_path`](Self::hash_bound_fs_path)). A path not identified yet, a
    ///   delivered value not resident yet, or a producer whose own key waits on one, is
    ///   [`Stamp::Deferred`], and the run loop stamps the node again at its turn.
    ///
    /// A method on the cache because the slots and the path memo it folds are the cache's; the
    /// *encoding* stays in `digest`, beside the [`DOMAIN`] versioning it.
    pub(crate) fn node_digest(
        &self,
        program: &CompiledGraph,
        schedule: &RunSchedule,
        node_idx: NodeIdx,
    ) -> Stamp {
        let e_node = &program[node_idx];

        // An `Impure` node varies per run, so it has no digest.
        if e_node.behavior != FuncBehavior::Pure {
            return Stamp::Unkeyed;
        }

        let mut deferred = false;
        let mut hasher = DigestHasher::new();
        hasher
            .write_bytes(DOMAIN)
            .write_pod(e_node.func_ref.id().as_u128())
            .write_bytes(e_node.func_ref.signature().bytes());

        for input in &program.inputs[e_node.inputs] {
            // Keyed by what the executor hands the node: a producer the run skips folds as
            // unbound, since folding its stamp would key this node on a past run.
            match schedule.delivery(input) {
                Delivery::Unbound => {
                    hasher.write_input_tag(InputTag::Unbound);
                }
                Delivery::Blocked => {
                    unreachable!("a node with a blocked input does not run, so it is not keyed")
                }
                Delivery::Const(value) => {
                    hasher.write_input_tag(InputTag::Const);
                    hasher.write_static(value);
                    if let Some(paths) = value.as_fs_paths()
                        && self.paths.hash(&mut hasher, paths).is_none()
                    {
                        deferred = true;
                    }
                }
                Delivery::Wire(addr) => {
                    // Visited first (topological order), so it is stamped. An unkeyed producer
                    // leaves this node unkeyed for good, so it is read before any deferral.
                    let producer = match self.slots[addr.node_idx].stamp() {
                        Stamp::Keyed(producer) => producer,
                        Stamp::Unkeyed => return Stamp::Unkeyed,
                        Stamp::Deferred => {
                            deferred = true;
                            continue;
                        }
                    };
                    // Digest then port index, both fixed-width, so consumers reading different
                    // ports of one node fold apart.
                    hasher
                        .write_input_tag(InputTag::Wire)
                        .write_digest(&producer)
                        .write_pod(addr.port_idx);
                    // The external state behind the *runtime value* is part of this node's key —
                    // the Bind-side counterpart of the `Const` arm's fold.
                    if input.fs_path && self.hash_bound_fs_path(&mut hasher, addr).is_none() {
                        deferred = true;
                    }
                }
            }
        }
        if deferred {
            Stamp::Deferred
        } else {
            Stamp::Keyed(hasher.finish())
        }
    }

    /// Fold the referent identity behind a **Bind-delivered** resource input, read off the
    /// producer's resident slot, so a wired path re-keys its consumer like a const path does. An
    /// unreadable value (producer not resident) is `None`, deferring the key; the run loop
    /// stamps again at reach time, once producers settled and any disk-backed path producer was
    /// hydrated. A mis-typed delivered value folds a distinct marker instead.
    fn hash_bound_fs_path(&self, hasher: &mut DigestHasher, addr: OutputAddr) -> Option<()> {
        // The *current* snapshot, so a value produced under an older digest
        // cannot deliver a reference into this key.
        let delivered = self.slots[addr.node_idx]
            .cur_snapshot()?
            .values()
            .get(addr.port_idx as usize)?;
        match delivered.as_fs_paths() {
            Some(paths) => {
                hasher.write_input_tag(InputTag::BoundPaths);
                self.paths.hash(hasher, paths)?;
            }
            // A resource input handed a non-path keys on its marker alone, and stays cacheable.
            None => {
                hasher.write_input_tag(InputTag::BoundMistyped);
            }
        }
        Some(())
    }

    /// Identify every executing node's filesystem paths in one off-thread
    /// pass, before the digests that fold them are stamped.
    ///
    /// A prefetch, not a decision: a path that fails to stamp leaves its node's digest `None`, and
    /// [`Self::restamp_and_hydrate`] re-stamps it at its own turn, where the failure belongs to
    /// exactly one node.
    pub(crate) async fn prepare(
        &mut self,
        program: &CompiledGraph,
        executing: impl IntoIterator<Item = NodeIdx>,
        cancel: CancelToken,
    ) {
        self.paths.begin_run();
        self.identify(program, executing, cancel).await.ok();
    }

    /// Identify every path `nodes` reads, in one off-thread pass. Adds to the run's memo —
    /// [`Self::prepare`] starts a fresh one.
    async fn identify(
        &mut self,
        program: &CompiledGraph,
        nodes: impl IntoIterator<Item = NodeIdx>,
        cancel: CancelToken,
    ) -> Result<(), StampError> {
        for node_idx in nodes {
            self.request_node_paths(program, node_idx);
        }
        self.paths.walk_queued(cancel).await
    }

    /// Queue the paths `node_idx` reads, skipping any this run already identified.
    fn request_node_paths(&mut self, program: &CompiledGraph, node_idx: NodeIdx) {
        let e_node = &program[node_idx];
        if e_node.behavior != FuncBehavior::Pure {
            return;
        }
        for input in &program.inputs[e_node.inputs] {
            let paths = match &input.binding {
                ExecBinding::Const(value) => value.as_fs_paths(),
                // Any resident value, not only a current one: the run's digests are not stamped
                // yet. Over-requesting costs one walk; under-requesting costs a node its pruning.
                ExecBinding::Wire(address) if input.fs_path => self.slots[address.node_idx]
                    .output_values()
                    .and_then(|values| values.get(address.port_idx as usize))
                    .and_then(DynamicValue::as_fs_paths),
                _ => None,
            };
            let Some(paths) = paths else { continue };
            for path in paths {
                self.paths.request(path);
            }
        }
    }

    /// Identify one node's paths, stamp the digest they complete, and hydrate its value if that
    /// digest now hits — the late second chance at reuse for a node whose digest the pre-run pass
    /// could not fold because a wired path had no value yet.
    ///
    /// Returns what [`Self::hydrate_reuse`] does. Every path is declared by `node_idx` alone, so a
    /// stamping failure is that node's: the caller fails it and the errored-upstream cascade takes
    /// its dependents.
    pub(crate) async fn restamp_and_hydrate(
        &mut self,
        restamp: Restamp<'_>,
    ) -> Result<ReuseOutcome, StampError> {
        let Restamp {
            program,
            schedule,
            node_idx,
            demand,
            cancel,
        } = restamp;
        self.identify(program, iter::once(node_idx), cancel).await?;
        self.stamp_digest(program, schedule, node_idx);
        Ok(self.hydrate_reuse(program, node_idx, demand).await)
    }

    /// The blob a disk-backed node with a digest reads and writes, named by stable id.
    fn blob_target(&self, program: &CompiledGraph, node_idx: NodeIdx) -> Option<BlobTarget> {
        if !program[node_idx].cache.persists_to_disk() {
            return None;
        }
        let digest = self.slots[node_idx].stamp().digest()?;
        self.disk_store
            .blob_target(program.program_ids[node_idx], digest)
    }

    /// Whether an unchanged output can satisfy this run's exact demand — the verdict alone,
    /// **without loading anything**, so the resolver can cut a producer cone without paying for a
    /// decode. A node with no key never reuses.
    ///
    /// RAM reuse trusts residency ([`is_resident_hit`](Self::is_resident_hit)): a content digest
    /// attests the value produced under it, however it came to be resident. Disk reuse is gated on
    /// `persists_to_disk` (enforced in [`blob_target`](Self::blob_target)) and answered from the
    /// blob header alone.
    pub(crate) async fn probe_reuse(
        &self,
        program: &CompiledGraph,
        node_idx: NodeIdx,
        demand: &[OutputDemand],
    ) -> bool {
        match self.reuse_source(program, node_idx, demand) {
            None => false,
            Some(ReuseSource::Resident) => true,
            Some(ReuseSource::Blob(target)) => {
                target
                    .covers_demand(Arc::clone(&program.codecs), demand.into())
                    .await
            }
        }
    }

    /// Whether this node could be served without running it, and from where; shared by the
    /// verdict and the load.
    fn reuse_source(
        &self,
        program: &CompiledGraph,
        node_idx: NodeIdx,
        demand: &[OutputDemand],
    ) -> Option<ReuseSource> {
        if self.is_resident_hit(node_idx, demand) {
            return Some(ReuseSource::Resident);
        }
        self.blob_target(program, node_idx).map(ReuseSource::Blob)
    }

    /// [`probe_reuse`](Self::probe_reuse)'s verdict, but leaving the value readable by the node's
    /// consumers: a resident one needs no load, and a blob is decoded into the slot. The run loop
    /// calls this when it *reaches* a reuse, so frontier decodes interleave with execution and a
    /// reused value is released by the same last-read bookkeeping as a computed one.
    ///
    /// [`ReuseOutcome::Missed`] when nothing loads: no usable blob, or one that stopped decoding
    /// since the probe. The decode path deletes an undecodable blob, so the next run recomputes.
    pub(crate) async fn hydrate_reuse(
        &mut self,
        program: &CompiledGraph,
        node_idx: NodeIdx,
        demand: &[OutputDemand],
    ) -> ReuseOutcome {
        let target = match self.reuse_source(program, node_idx, demand) {
            None => return ReuseOutcome::Missed,
            Some(ReuseSource::Resident) => return ReuseOutcome::Served,
            Some(ReuseSource::Blob(target)) => target,
        };
        let digest = target.digest;
        let Some(snapshot) = target
            .read(Arc::clone(&program.codecs), demand.into())
            .await
        else {
            return ReuseOutcome::Missed;
        };
        self.slots[node_idx].load_from_blob(snapshot, digest);
        ReuseOutcome::Served
    }

    /// Write the blob a value served from RAM owes.
    ///
    /// A value kept in RAM and on disk is served from RAM without the disk being consulted, so nothing else
    /// would notice that the write establishing its durability never landed (a missing codec, a
    /// cache mode turned on after the value was resident). Only `invoke_node` stores, and a node
    /// served from RAM does not invoke.
    ///
    /// The debt check is a field read, so a blob this engine already wrote costs no I/O: an event
    /// loop executes on every tick.
    pub(crate) async fn settle_blob_debt(
        &mut self,
        program: &CompiledGraph,
        node_idx: NodeIdx,
    ) -> StoreResult {
        if !self.slots[node_idx].owes_blob() {
            return Ok(StoreOutcome::NothingOwed);
        }
        // `PreserveCovering`, not `KnownMiss`: the reuse verdict was about RAM, so a broader blob
        // already on disk must survive. It also re-establishes the belief when the debt was only a
        // gap in this engine's knowledge.
        self.store_node(program, node_idx, StorePolicy::PreserveCovering)
            .await
    }

    /// Write `node_idx`'s freshly-computed outputs to disk the moment it finishes, so a long run's
    /// earlier caches are durable even if a later node errors or the run is cancelled.
    /// [`StorePolicy::KnownMiss`] publishes directly after resolution proved reuse impossible;
    /// [`StorePolicy::PreserveCovering`] first protects a broader blob when a maintenance flush
    /// has no such verdict.
    ///
    /// Only writes a value current under the node's key: a value produced under a superseded
    /// digest must not overwrite the new digest's blob.
    ///
    /// [`StoreOutcome::NothingOwed`] for a node with nothing to write: not disk-backed, holding no
    /// snapshot current under its key, or whose last write failed, which waits for an install, a
    /// new store or a flush. The slot is told the answer here rather than by each caller: a value
    /// that stays resident is served from RAM on every later run, so this is the last moment
    /// anything would notice a write that did not land.
    pub(crate) async fn store_node(
        &mut self,
        program: &CompiledGraph,
        node_idx: NodeIdx,
        policy: StorePolicy,
    ) -> StoreResult {
        if self.slots[node_idx].write_failed() {
            return Ok(StoreOutcome::NothingOwed);
        }
        let (Some(target), Some(snapshot)) = (
            self.blob_target(program, node_idx),
            self.slots[node_idx].cur_snapshot(),
        ) else {
            return Ok(StoreOutcome::NothingOwed);
        };
        let outcome = target
            .store(
                Arc::clone(&program.codecs),
                snapshot.values().to_vec(),
                policy,
            )
            .await;
        self.slots[node_idx].note_store(&outcome);
        outcome
    }

    /// Persist the resident values of `seeds` — [`evict`](Self::evict)'s
    /// counterpart, for a node the host just made disk-backed while its value
    /// was already in RAM.
    ///
    /// The named nodes alone, never their consumer cone: a blob changes nothing a consumer does,
    /// whereas an eviction must take the cone so a surviving consumer does not reuse and prune
    /// what was freed.
    pub(crate) async fn flush(
        &mut self,
        program: &CompiledGraph,
        seeds: impl IntoIterator<Item = ProgramId>,
    ) -> CacheFlushReport {
        self.flush_each(
            program,
            seeds
                .into_iter()
                .filter_map(|program_id| program.node(program_id)),
        )
        .await
    }

    /// [`flush`](Self::flush) over the whole installed program — what a newly
    /// attached [`DiskStore`] owes every value computed while it was
    /// memory-only.
    pub(crate) async fn flush_all(&mut self, program: &CompiledGraph) -> CacheFlushReport {
        self.flush_each(
            program,
            program.e_nodes.iter_indexed().map(|(node_idx, _)| node_idx),
        )
        .await
    }

    /// Store whichever of `nodes` are disk-backed and hold a value current under their stamped
    /// digest — the shared body of the two flushes. Both tests are
    /// [`store_node`](Self::store_node)'s own, so a node that is neither costs one
    /// [`blob_target`](Self::blob_target) that answers `None`.
    ///
    /// [`StorePolicy::PreserveCovering`] because a flush carries no reuse verdict: a blob that
    /// already covers the snapshot must survive untouched.
    async fn flush_each(
        &mut self,
        program: &CompiledGraph,
        nodes: impl Iterator<Item = NodeIdx>,
    ) -> CacheFlushReport {
        let mut report = CacheFlushReport::default();
        for node_idx in nodes {
            // Asked for, so a write that failed is tried again.
            self.slots[node_idx].reconsider_blob();
            let outcome = self
                .store_node(program, node_idx, StorePolicy::PreserveCovering)
                .await;
            let program_id = program.program_ids[node_idx];
            match outcome {
                // A node with nothing to write holds no value to persist; that is not a shortfall.
                Ok(
                    StoreOutcome::Published
                    | StoreOutcome::AlreadyCovered
                    | StoreOutcome::NothingOwed,
                ) => {}
                Ok(StoreOutcome::Unsupported { type_id }) => {
                    report.unsupported.push(CacheFlushUnsupported {
                        program_id,
                        type_id,
                    });
                }
                Err(error) => report.failures.push(CacheNodeFailure {
                    program_id,
                    cause: error.into(),
                }),
            }
        }
        report
    }

    /// Release resident values that cannot be a future RAM hit under the installed program. Called
    /// on install and after each run, so cache-mode downgrades, impure outputs, and superseded
    /// snapshots do not wait for another execution to free RAM.
    pub(crate) fn release_dead_outputs(&mut self, program: &CompiledGraph) {
        for (node_idx, e_node) in program.e_nodes.iter_indexed() {
            let Some(resident_len) = self.slots[node_idx].output_values().map(<[_]>::len) else {
                continue;
            };
            // A snapshot of a different length cannot describe this node's outputs, whatever its
            // digest says: a func that grows an output keeps its id, so the stale
            // `produced_under` still equals the stale stamp.
            let retained = resident_len == e_node.outputs.len()
                && e_node.cache.caches_in_ram()
                && e_node.behavior == FuncBehavior::Pure
                && self.is_resident_cur(node_idx);
            if !retained {
                self.slots[node_idx].clear_output();
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use common::CancelToken;

    use crate::data::dynamic_value::DynamicValue;

    use crate::exec::cache::digest::Digest;
    use crate::exec::cache::disk_store::DiskStore;
    use crate::exec::cache::runtime_cache::RuntimeCache;
    use crate::exec::cache::runtime_slot::OutputSnapshot;
    use crate::exec::cache::stamp_job::error::StampError;
    use crate::exec::compile::compiled_graph::CompiledGraph;
    use crate::exec::idx::NodeIdx;

    impl RuntimeCache {
        /// A first install of `program`; tests that reinstall name the outgoing one through
        /// [`RuntimeCache::reconcile`].
        pub(crate) fn install_for_test(&mut self, program: &CompiledGraph) {
            self.reconcile(&CompiledGraph::default(), program);
        }

        /// The attached store, for tests that read its I/O counters or corrupt a blob.
        pub(crate) fn disk_store(&self) -> &DiskStore {
            &self.disk_store
        }

        /// [`RuntimeCache::identify`] on this thread, without the blocking pool a test has no
        /// runtime to reach.
        pub(crate) fn prepare_node_blocking(&mut self, program: &CompiledGraph, node_idx: NodeIdx) {
            self.prepare_nodes_blocking(program, [node_idx]).ok();
        }

        /// [`RuntimeCache::prepare`]'s batching on this thread; hands back what production
        /// discards, so a test can say whether the shared pass survived.
        pub(crate) fn prepare_nodes_blocking(
            &mut self,
            program: &CompiledGraph,
            nodes: impl IntoIterator<Item = NodeIdx>,
        ) -> Result<(), StampError> {
            for node_idx in nodes {
                self.request_node_paths(program, node_idx);
            }
            self.paths.walk_queued_blocking(&CancelToken::never())
        }

        /// The key the run's memo holds `path` under.
        pub(crate) fn path_key(&self, path: &str) -> Option<&String> {
            self.paths.key(path)
        }

        /// Re-key `path` with at least `capacity` bytes reserved, so a test can tell the key apart.
        pub(crate) fn widen_path_key(&mut self, path: &str, capacity: usize) {
            self.paths.widen_key(path, capacity);
        }

        /// Plant a file identity without touching a filesystem.
        pub(crate) fn stamp_file(&mut self, path: &str, len: u64, mtime_ns: i128) {
            self.paths.plant_file(path, len, mtime_ns);
        }

        /// Make `values` a hit for `node_idx`: resident under the digest this run stamped.
        pub(crate) fn prime_hit(&mut self, node_idx: NodeIdx, values: Vec<DynamicValue>) {
            let digest = self.slots[node_idx]
                .stamp()
                .digest()
                .expect("a primed node is reproducible, so this run stamped it a digest");
            self.slots[node_idx].load_output(OutputSnapshot::new(values), Some(digest));
        }

        /// Plant a whole snapshot under the digest it is to count as produced by.
        pub(crate) fn hydrate(
            &mut self,
            node_idx: NodeIdx,
            snapshot: OutputSnapshot,
            digest: Digest,
        ) {
            self.slots[node_idx].load_output(snapshot, Some(digest));
        }
    }
}

#[cfg(test)]
mod tests;
