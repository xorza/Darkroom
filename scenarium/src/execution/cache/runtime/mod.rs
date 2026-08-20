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

pub(crate) mod cache_flush_report;
pub(crate) mod error;

use std::collections::HashSet;
use std::ops::{Index, IndexMut};
use std::sync::Arc;

use common::CancelToken;
use hashbrown::HashMap;

use crate::containers::column::Column;
use crate::execution::cache::digest::{DOMAIN, Digest, DigestHasher, InputTag};
use crate::execution::cache::disk_store::error::StoreResult;
use crate::execution::cache::disk_store::store_outcome::StoreOutcome;
use crate::execution::cache::disk_store::{BlobTarget, DiskStore, StorePolicy};
use crate::execution::cache::resource::error::StampError;
use crate::execution::cache::resource::{FsPathId, StampJob};
use crate::execution::cache::runtime::cache_flush_report::CacheFlushReport;
use crate::execution::cache::runtime::error::{
    CacheFlushUnsupported, CacheNodeError, CacheNodeFailure,
};
use crate::execution::cache::slot::RuntimeSlot;
use crate::execution::compile::compiled_graph::{CompiledGraph, ExecutionBinding};
use crate::execution::compile::consumer_cone::ConsumerCone;
use crate::execution::identity::{NodeIdx, OutputAddr};
use crate::graph::func::FuncBehavior;
use crate::graph::func::lambda::OutputDemand;
use crate::graph::identity::NodeId;
use crate::runtime::context::ContextStore;
use crate::{DynamicValue, RamUsage};

/// The per-node cross-run cache plus its disk backing. `slots` is a
/// [`Column`] aligned to the installed program, so every run-loop access is
/// an array read; cross-install survival happens at [`reconcile`](Self::reconcile),
/// which re-pairs the slots with the new index order by stable id.
///
/// **The program is not held here — it arrives with each call.** A `NodeIdx`
/// means nothing without the program it indexes, so every method that reads one
/// takes that program alongside it. The pairing is established by
/// [`ExecutionEngine`](crate::execution::engine::ExecutionEngine), which owns both
/// halves and is the only caller of [`reconcile`](Self::reconcile) — so there is
/// no run in which the cache and the program it is asked about can be different
/// installs. That makes the pairing something the owner establishes, rather
/// than an invariant every method here would have to validate.
///
/// The resolver stamps each node's digest and decides cache reuse, while the
/// executor mutates outputs/state and consumes that decision. `disk_store`
/// persists outputs and serves them back; it is kept across graph updates while
/// only `slots` is reconciled or cleared.
#[derive(Default, Debug)]
pub(crate) struct RuntimeCache {
    /// Private: the *column* is the cache's own — its length is the alignment
    /// invariant [`reconcile`](Self::reconcile) establishes, so nothing outside
    /// may push, drain, or resize it. Individual slots are reached by
    /// [`Index<NodeIdx>`], the same way a node is reached on
    /// [`CompiledGraph`].
    slots: Column<NodeIdx, RuntimeSlot>,
    /// Private for the same reason as the slots: the worker replaces it
    /// wholesale between runs, and going through
    /// [`set_disk_store`](Self::set_disk_store) is what makes that a thing the
    /// cache is told about rather than one done to it.
    disk_store: DiskStore,
    /// What each path this run reads *was*, identified once. Held beside the
    /// slots rather than inside the walker that fills it, because the digest
    /// fold reads both together — a path's identity, and the producer slot
    /// that delivered the path.
    fs_paths: HashMap<String, FsPathId>,
    /// The off-thread walk that fills `fs_paths`: queue, then pass. It owns
    /// only what crosses to the blocking pool.
    stamp_job: StampJob,
    /// The buffers an eviction's downstream walk runs in. Held rather than
    /// built per call: what it derives is a pure function of the program and is
    /// refilled every time, but the allocations behind it need not be.
    cone: ConsumerCone,
    /// Which shared values [`measure_resident_ram`](Self::measure_resident_ram) has
    /// already counted toward its global total, by pointer identity. Scratch in the
    /// strict sense — meaningless the moment that walk ends, and held only so the
    /// allocation survives to the next measurement.
    ram_seen: HashSet<usize>,
    /// The per-node breakdown the same walk produces. An *answer*, not scratch: it
    /// outlives the call and is read through [`node_ram`](Self::node_ram) when the run is
    /// reduced to status rows.
    ///
    /// A separate field from `ram_seen` because the two are different things that merely
    /// share a walk — one keyed by pointer and dead at the end of it, one keyed by node
    /// and deliberately undeduplicated, since a node reports everything it holds whether
    /// or not a neighbour holds it too. Here rather than in a caller's buffer because its
    /// length is `slots`': a caller could only ever have supplied the memory, never the
    /// shape.
    node_ram: Column<NodeIdx, RamUsage>,
}

/// Where a reuse would come from, if one is possible at all — the front half
/// of [`probe_reuse`](RuntimeCache::probe_reuse) and
/// [`hydrate_reuse`](RuntimeCache::hydrate_reuse), which ask the same question
/// and differ only in what they do with the answer.
///
/// Naming the answer is what stops the verdict and the load that acts on it
/// from deriving their own: both now name *one* blob. What it cannot rule out
/// is the blob changing between the two — the header is checked when the
/// verdict is given and the body read later, with no lock in between, which is
/// what [`RunError::CacheLoadFailed`](crate::execution::error::RunError) exists
/// for.
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
/// A named pair rather than a `bool` because the two callers read the negative
/// answer as opposite things. The run loop reaching a `Reuse` node has already
/// had the cut prune its producers, so a `Missed` there is unrecoverable and
/// fails the node; the same `Missed` at a re-stamped `Run` node is an ordinary
/// cache miss, and the lambda simply runs. Neither reading belongs in the
/// cache, so this says only what happened.
#[derive(Debug)]
pub(crate) enum ReuseOutcome {
    /// A value is resident under the node's current digest and ready to read —
    /// it was already there, or a blob decoded into the slot.
    Served,
    /// Nothing to serve: no usable source, or a blob that stopped decoding
    /// since the probe.
    Missed,
}

impl Index<NodeIdx> for RuntimeCache {
    type Output = RuntimeSlot;

    fn index(&self, node_idx: NodeIdx) -> &RuntimeSlot {
        &self.slots[node_idx]
    }
}

impl IndexMut<NodeIdx> for RuntimeCache {
    fn index_mut(&mut self, node_idx: NodeIdx) -> &mut RuntimeSlot {
        &mut self.slots[node_idx]
    }
}

impl RuntimeCache {
    /// The span of the slot column — the program node count it is aligned to.
    pub(crate) fn slot_count(&self) -> usize {
        self.slots.len()
    }

    /// The slots in index order, for a walk that pairs them with the program
    /// they belong to.
    pub(crate) fn slots(&self) -> impl Iterator<Item = &RuntimeSlot> {
        self.slots.iter()
    }

    /// Attach the store this cache persists to and serves from. Called between
    /// runs, when a document gains or changes its cache root: every reuse
    /// verdict already given was answered from the *previous* store, so this
    /// must not land mid-run.
    pub(crate) fn set_disk_store(&mut self, disk_store: DiskStore) {
        self.disk_store = disk_store;
    }

    pub(crate) fn clear(&mut self) {
        self.slots.clear();
        self.fs_paths.clear();
        self.stamp_job.clear_queue();
    }

    /// Drop `seeds` and everything downstream of them from RAM and disk.
    ///
    /// The cone, not the seeds alone: a consumer whose own value survives reuses
    /// it, and reuse prunes its producers — so evicting one node in isolation
    /// would free its slot and change nothing a later run does. A seed absent
    /// from the program is skipped; the host names authored nodes and the
    /// installed program need not still hold every one.
    pub(crate) async fn evict(
        &mut self,
        program: &CompiledGraph,
        seeds: impl IntoIterator<Item = NodeId>,
    ) -> Vec<CacheNodeFailure> {
        let downstream = self.cone.of(
            program,
            seeds
                .into_iter()
                .filter_map(|node_id| program.node(node_id)),
        );
        let mut failures = Vec::new();
        for node_idx in downstream.iter() {
            let node_id = program.node_ids[node_idx];
            match self.disk_store.remove_node(node_id).await {
                // Dropping the value drops the belief that a blob backed it,
                // which is exactly what the removal just made false.
                Ok(()) => self.slots[node_idx].clear_output(),
                Err(error) => failures.push(CacheNodeFailure {
                    node_id,
                    cause: CacheNodeError::Removal(error),
                }),
            }
        }
        failures
    }

    /// Measure the RAM held by resident values: the global total, returned, and the
    /// per-node breakdown, left in [`node_ram`](Self::node_ram) for whoever pairs a
    /// node's footprint with its run result. The total deduplicates shared custom values
    /// by pointer identity, while each node reports the full size of every value it
    /// holds. `Empty` slots read as zero.
    ///
    /// One walk for both because the dedup pass is the expensive half and the two answers
    /// fall out of it together. The breakdown is refilled from scratch and aligned like the
    /// slots themselves — dense rather than sparse, so it is indexed the same way every
    /// other per-node column is.
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
                        self.ram_seen.insert(Arc::as_ptr(arc) as *const () as usize)
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
    pub(crate) fn node_ram(&self) -> &Column<NodeIdx, RamUsage> {
        &self.node_ram
    }

    /// Realign the slots from the program they currently belong to onto a newly
    /// installed one: re-pair them with the new index order by stable id —
    /// dropping persistent state whose owning func changed — default new nodes,
    /// trim removed ones, and apply the
    /// installed program's RAM-retention policy immediately. The one place ids
    /// are hashed for slot access; every per-run access is an index read.
    ///
    /// `previous` is the program the slots currently belong to, `None` before
    /// anything was installed. It is named rather than remembered because the
    /// only caller —
    /// [`ExecutionEngine::install`](crate::execution::engine::ExecutionEngine) —
    /// holds both programs at the moment of the swap, so the pair this walks is
    /// established by the owner rather than validated afterwards.
    pub(crate) fn reconcile(&mut self, previous: Option<&CompiledGraph>, program: &CompiledGraph) {
        // `Column::drain` empties the column when its guard drops, so the slots
        // are released even on the first install, where the left side of the zip
        // yields nothing.
        let mut retained: HashMap<NodeId, RuntimeSlot> = previous
            .into_iter()
            .flat_map(|previous| previous.node_ids.iter().copied())
            .zip(self.slots.drain())
            .collect();
        for (node_id, e_node) in program.node_ids.iter().zip(program.e_nodes.iter()) {
            let owner = e_node.func_id;
            let slot = match retained.remove(node_id) {
                Some(mut slot) => {
                    slot.reown(owner);
                    slot
                }
                None => RuntimeSlot::new(owner),
            };
            self.slots.push(slot);
        }
        self.release_dead_outputs(program);
    }

    pub(crate) fn is_resident_current(&self, node_idx: NodeIdx) -> bool {
        self.slots[node_idx].current_snapshot().is_some()
    }

    /// Current *and* holding every output this run demands.
    fn is_resident_hit(&self, node_idx: NodeIdx, demand: &[OutputDemand]) -> bool {
        self.slots[node_idx]
            .current_snapshot()
            .is_some_and(|snapshot| snapshot.covers_demand(demand))
    }

    /// [`RuntimeSlot::read_output`] addressed by [`OutputAddr`], with the one check
    /// the slot cannot make itself: that what it holds still has the arity the
    /// installed program declares.
    pub(crate) fn read_output_port(
        &mut self,
        program: &CompiledGraph,
        address: OutputAddr,
        take: bool,
    ) -> Option<DynamicValue> {
        let arity = program[address.node_idx].outputs.len as usize;
        let slot = &mut self.slots[address.node_idx];
        debug_assert!(
            slot.output_values()
                .is_none_or(|values| values.len() == arity),
            "cached output values must match the node's compiled arity"
        );
        slot.read_output(address.port_idx, take)
    }

    /// Producer-first digest pass over the whole schedule, so a consumer
    /// folds an already-stamped producer digest. Reuse is deliberately not
    /// probed here because exact demand exists only in the resolver's
    /// reverse sweep. A Bind-delivered path value that is not resident yet
    /// stamps `None`; the run loop can improve that node to reuse once its
    /// path producer settles.
    /// `executing` is the run's schedule in producer-first order
    /// ([`RunSchedule::executing`](crate::execution::schedule::RunSchedule::executing));
    /// the cache reads the nodes, not the plan that selected them.
    pub(crate) fn stamp_digests(
        &mut self,
        program: &CompiledGraph,
        executing: impl IntoIterator<Item = NodeIdx>,
    ) {
        for node_idx in executing {
            self.stamp_digest(program, node_idx);
        }
    }

    /// Stamp one node's structural content digest into its slot. The resolver's
    /// pass calls this before exact output demand is known; cache coverage is
    /// probed later by [`probe_reuse`](Self::probe_reuse).
    pub(crate) fn stamp_digest(&mut self, program: &CompiledGraph, node_idx: NodeIdx) {
        // Folded whole before the write, so the fold's read of the slots ends
        // before the slot it stamps is borrowed mutably.
        let digest = self.node_digest(program, node_idx);
        self.slots[node_idx].current_digest = digest;
    }

    /// A node's **content digest** — the one content key it's cached under, folding its identity
    /// (func id + output types) plus its structural inputs. The single digest the whole
    /// cache keys on: RAM reuse ([`is_resident_hit`](Self::is_resident_hit)), disk load/store, and
    /// downstream folding all read the node's stamped `current_digest`. Computed producer-first
    /// (topological), so a `Bind` producer's `current_digest` is already stamped when read.
    ///
    /// - An **`Impure`** node has no digest (`None`) — it varies per run, so it never caches and
    ///   always recomputes; a `Bind` producer with a `None` digest taints this node to `None`.
    /// - Otherwise fold every input structurally: a `Const`'s value + prepared `FsPath`
    ///   file/dir content, or a `Bind` producer's stamped `current_digest` — plus, for a
    ///   resource-typed input, the live identity of the referent behind the *delivered* value
    ///   ([`hash_bound_fs_path`](Self::hash_bound_fs_path)). That last fold needs the producer's
    ///   value: unreadable ⇒ `None`, and the run loop re-stamps such a node at reach time, once
    ///   its producers settled.
    ///
    /// A method on the cache because two of the three things it folds are the cache's: the slots
    /// it reads producer digests and delivered values from, and the `fs_paths` memo behind every
    /// external identity. The third — the node's own identity and inputs — is the program's, so
    /// that arrives as an argument. The *encoding* stays in `digest`, beside the [`DOMAIN`]
    /// versioning it.
    pub(crate) fn node_digest(&self, program: &CompiledGraph, node_idx: NodeIdx) -> Option<Digest> {
        let e_node = &program[node_idx];

        // Only a `Pure` node is content-cacheable; an `Impure` node varies per run, so it has no
        // digest and always recomputes.
        if e_node.behavior != FuncBehavior::Pure {
            return None;
        }

        let mut hasher = DigestHasher::new();
        hasher
            .write_bytes(DOMAIN)
            .write_pod(e_node.func_id.as_u128());

        let outputs = &program.outputs[e_node.outputs];
        hasher.write_pod(outputs.len() as u64);
        for output in outputs {
            hasher.write_data_type(output);
        }

        for input in &program.inputs[e_node.inputs] {
            match &input.binding {
                ExecutionBinding::None => {
                    hasher.write_input_tag(InputTag::Unbound);
                }
                ExecutionBinding::Const(value) => {
                    hasher.write_input_tag(InputTag::Const);
                    hasher.write_static(value);
                    if let Some(paths) = value.as_fs_paths() {
                        self.hash_fs_paths(&mut hasher, paths)?;
                    }
                }
                ExecutionBinding::Bind(addr) => {
                    // The producer was visited first (topological order), so its `current_digest`
                    // is set; a `None` taints this node.
                    let producer = self.slots[addr.node_idx].current_digest?;
                    // Producer digest then port index, both fixed-width, so two
                    // consumers reading different ports of one node fold apart.
                    hasher
                        .write_input_tag(InputTag::Bind)
                        .write_digest(&producer)
                        .write_pod(addr.port_idx);
                    // A resource-typed input dereferences the delivered reference, so the
                    // external state behind the *runtime value* is part of this node's key —
                    // the Bind-side counterpart of the `Const` arm's fold. Needs the
                    // producer's value; unreadable (pre-run) ⇒ `None`, re-stamped at reach
                    // time by the run loop.
                    if input.stamps_fs_path {
                        self.hash_bound_fs_path(&mut hasher, addr)?;
                    }
                }
            }
        }
        Some(hasher.finish())
    }

    /// Fold the referent identity behind a **Bind-delivered** resource input: read the
    /// delivered value off the producer's resident slot and fold its prepared file/directory
    /// identity, so a wired path re-keys its consumer exactly like a const path does. The
    /// producer's value must exist first: an unreadable value (producer not resident) is
    /// `None`, tainting the node's digest — the pre-run sweep stamps it "uncacheable, must
    /// run", and the run loop then re-stamps at reach time, when the producers have settled
    /// and any disk-backed path producer was hydrated (`executor.rs`). A mis-typed delivered
    /// value folds a distinct marker instead.
    fn hash_bound_fs_path(&self, hasher: &mut DigestHasher, addr: &OutputAddr) -> Option<()> {
        // The *current* snapshot, so a value produced under an older digest
        // cannot deliver a reference into this key.
        let delivered = self.slots[addr.node_idx]
            .current_snapshot()?
            .values()
            .get(addr.port_idx as usize)?;
        match delivered.as_fs_paths() {
            Some(paths) => {
                hasher.write_input_tag(InputTag::BoundPaths);
                self.hash_fs_paths(hasher, paths)?;
            }
            // A mis-typed wire — a resource input handed something that is
            // not a path — keys on its marker alone, and stays cacheable.
            None => {
                hasher.write_input_tag(InputTag::BoundMistyped);
            }
        }
        Some(())
    }

    /// Fold the identities behind the paths a value names.
    ///
    /// `None` is the failure: a path this run never stamped, leaving its node
    /// without a sound cache key. A value naming *no* paths does not call at
    /// all — that is a plain const, not a filesystem read.
    ///
    /// An [unset](is_unset_path) slot is neither. It names nothing to stat, so
    /// it folds a marker and leaves the node cacheable — "no file chosen yet"
    /// is a state the author passes through, not a resource that failed. The
    /// marker stands in the slot rather than being skipped: on the Bind side
    /// the authored strings are not folded alongside, so skipping would let
    /// `["", p]` and `[p, ""]` key alike.
    fn hash_fs_paths(&self, hasher: &mut DigestHasher, paths: &[String]) -> Option<()> {
        hasher.write_pod(paths.len() as u64);
        for path in paths {
            if is_unset_path(path) {
                hasher.write_input_tag(InputTag::UnsetPath);
                continue;
            }
            self.fs_paths.get(path)?.hash(hasher);
        }
        Some(())
    }

    /// Identify every executing node's filesystem paths in one off-thread
    /// pass, before the digests that fold them are stamped.
    ///
    /// **A prefetch, not a decision.** A path this fails to stamp simply
    /// does not land, which leaves its node's digest `None` — and a node
    /// with no digest is re-stamped at its own turn by
    /// [`Self::restamp_and_hydrate`], where the failure belongs to exactly one
    /// node and is reported as that node's. So nothing is lost here but
    /// the batching, and no failure is decided at a point that cannot say
    /// which node it concerns.
    pub(crate) async fn prepare(
        &mut self,
        program: &CompiledGraph,
        executing: impl IntoIterator<Item = NodeIdx>,
        cancel: CancelToken,
    ) {
        // A fresh run identifies afresh.
        self.fs_paths.clear();
        self.stamp_job.clear_queue();
        let _ = self.identify(program, executing, cancel).await;
    }

    /// Identify every path `nodes` reads, in one off-thread pass: queue, then
    /// walk. Adds to the run's memo — [`Self::prepare`] is what starts a fresh
    /// one.
    async fn identify(
        &mut self,
        program: &CompiledGraph,
        nodes: impl IntoIterator<Item = NodeIdx>,
        cancel: CancelToken,
    ) -> Result<(), StampError> {
        for node_idx in nodes {
            self.request_node_paths(program, node_idx);
        }
        self.walk_queued(cancel).await
    }

    /// Queue the paths `node_idx` reads, on top of whatever is already queued,
    /// skipping any this run already identified.
    fn request_node_paths(&mut self, program: &CompiledGraph, node_idx: NodeIdx) {
        let e_node = &program[node_idx];
        if e_node.behavior != FuncBehavior::Pure {
            return;
        }
        for input in &program.inputs[e_node.inputs] {
            let paths = match &input.binding {
                ExecutionBinding::Const(value) => value.as_fs_paths(),
                // Any resident value, not only a current one: this runs
                // before the run's digests are stamped, so the digest
                // fold's stricter accessor would see almost nothing here.
                // Over-requesting costs one walk; under-requesting costs a
                // node its pruning.
                ExecutionBinding::Bind(address) if input.stamps_fs_path => self.slots
                    [address.node_idx]
                    .output_values()
                    .and_then(|values| values.get(address.port_idx as usize))
                    .and_then(DynamicValue::as_fs_paths),
                _ => None,
            };
            let Some(paths) = paths else { continue };
            for path in paths {
                // An unset slot has nothing to walk, and queueing it would
                // fail the whole pass on a `metadata("")` — costing every
                // other node in the batch its pre-run identity.
                if is_unset_path(path) || self.fs_paths.contains_key(path) {
                    continue;
                }
                self.stamp_job.request(path);
            }
        }
    }

    /// Run the queued pass on the blocking pool, leaving the queue empty and
    /// what it identified in the memo.
    ///
    /// The job goes out and comes back — it owns its queue and scratch, so
    /// nothing of the cache is borrowed across the boundary and the memo never
    /// moves at all. The memo takes what the pass stamped whatever its verdict,
    /// and the pass walks past a path that will not read
    /// ([`StampJob::run`]) — so one unreadable path costs its own node's
    /// digest and none of the identities the same pass gathered for the rest of
    /// the run.
    async fn walk_queued(&mut self, cancel: CancelToken) -> Result<(), StampError> {
        if !self.stamp_job.is_queued() {
            return Ok(());
        }
        let mut job = std::mem::take(&mut self.stamp_job);
        let (job, resolved) = tokio::task::spawn_blocking(move || {
            let resolved = job.run(&cancel);
            (job, resolved)
        })
        .await
        .expect("resource stamping task panicked");
        self.stamp_job = job;
        self.fs_paths.extend(self.stamp_job.drain_stamped());
        resolved
    }

    /// Identify one node's paths, stamp the digest they complete, and
    /// hydrate its value if that digest now hits — the whole late second
    /// chance at reuse, for a node whose digest the pre-run pass could not
    /// fold because a wired path had no value yet.
    ///
    /// Returns what [`Self::hydrate_reuse`] does, and means it the same way — the
    /// new digest either landed a value the node's consumers can read, or it did
    /// not and the lambda still has to run. Every path here is declared by
    /// `node_idx` alone, so a stamping failure is that node's — the caller fails
    /// it and the ordinary errored-upstream cascade takes its dependents, rather
    /// than a run-wide abort blaming nobody.
    pub(crate) async fn restamp_and_hydrate(
        &mut self,
        program: &CompiledGraph,
        node_idx: NodeIdx,
        demand: &[OutputDemand],
        contexts: &mut ContextStore,
        cancel: CancelToken,
    ) -> Result<ReuseOutcome, StampError> {
        self.identify(program, std::iter::once(node_idx), cancel)
            .await?;
        self.stamp_digest(program, node_idx);
        Ok(self
            .hydrate_reuse(program, node_idx, demand, contexts)
            .await)
    }

    /// Blobs are named by stable id, so they survive installs that shift indices.
    fn blob_target(&self, program: &CompiledGraph, node_idx: NodeIdx) -> Option<BlobTarget> {
        self.disk_store.blob_target(
            program.node_ids[node_idx],
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
        program: &CompiledGraph,
        node_idx: NodeIdx,
        demand: &[OutputDemand],
    ) -> bool {
        match self.reuse_source(program, node_idx, demand) {
            None => false,
            Some(ReuseSource::Resident) => true,
            Some(ReuseSource::Blob(target)) => self.disk_store.covers_demand(&target, demand).await,
        }
    }

    /// Whether this node could be served without running it, and from where.
    /// Shared by the verdict and the load so neither derives its own target.
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

    /// [`probe_reuse`](Self::probe_reuse)'s verdict, but leaving the value readable by the
    /// node's consumers: a resident one needs no load, and a blob is decoded into the slot
    /// here. The run loop calls this when it *reaches* a reuse rather than the resolver when
    /// it probes, so frontier decodes interleave with execution instead of accumulating
    /// ahead of the first lambda, and a reused value lives exactly as long as a freshly
    /// computed one — released by the same last-read bookkeeping.
    ///
    /// [`ReuseOutcome::Missed`] when nothing loads: no usable blob, or one that stopped
    /// decoding since the probe. The decode path deletes an undecodable blob, so the next
    /// run misses cleanly and recomputes.
    pub(crate) async fn hydrate_reuse(
        &mut self,
        program: &CompiledGraph,
        node_idx: NodeIdx,
        demand: &[OutputDemand],
        ctx: &mut ContextStore,
    ) -> ReuseOutcome {
        let target = match self.reuse_source(program, node_idx, demand) {
            None => return ReuseOutcome::Missed,
            Some(ReuseSource::Resident) => {
                self.settle_blob_debt(program, node_idx, ctx).await;
                return ReuseOutcome::Served;
            }
            Some(ReuseSource::Blob(target)) => target,
        };
        let Some(snapshot) = self.disk_store.read(&target, demand, ctx).await else {
            return ReuseOutcome::Missed;
        };
        self.slots[node_idx].load_from_blob(snapshot, target.digest);
        ReuseOutcome::Served
    }

    /// Write the blob a resident value owes.
    ///
    /// A `Both`-mode value is served from RAM without the disk being consulted,
    /// so nothing else would ever notice that the write establishing its
    /// durability never landed — a store that failed, a type whose codec was
    /// missing, or a cache mode the host turned on after the value was already
    /// resident. Only `invoke_node` stores, and a node served from RAM does not
    /// invoke, so without this the node reports itself disk-cached with nothing
    /// behind it until its digest moves.
    ///
    /// The debt check is a digest comparison, so the common case — a blob this
    /// engine already wrote — costs no I/O at all. Runs are not rare enough to
    /// pay a `stat` per disk-backed node in: an event loop executes on every
    /// tick.
    async fn settle_blob_debt(
        &mut self,
        program: &CompiledGraph,
        node_idx: NodeIdx,
        ctx: &mut ContextStore,
    ) {
        if self.slots[node_idx].blob_is_current() {
            return;
        }
        // `PreserveCovering` rather than `KnownMiss`: this carries no reuse
        // verdict about the *blob* — the verdict was about RAM — so a broader
        // one already on disk must survive. It also re-establishes the belief
        // when the debt was only ever a gap in this engine's knowledge.
        self.store_node(program, node_idx, StorePolicy::PreserveCovering, ctx)
            .await;
    }

    /// Write `node_id`'s freshly-computed outputs to disk the moment it finishes (the executor
    /// calls this right after a successful invoke), so a long run's earlier caches are durable
    /// even if a later node errors or the run is cancelled. [`StorePolicy::KnownMiss`] publishes
    /// directly after resolution proved reuse impossible; [`StorePolicy::PreserveCovering`]
    /// first protects a broader blob when a maintenance flush has no such verdict.
    ///
    /// Only writes a value that matches the node's *current* digest
    /// ([`is_resident_hit`](Self::is_resident_hit)): a resident value produced under a superseded
    /// digest must not be stamped with — and overwrite — the new digest's blob. In the run loop
    /// the just-stamped value is always a current hit; this guards maintenance flushes when a
    /// disk store is attached.
    ///
    /// `None` is the node with nothing to write — not disk-backed, or holding no
    /// snapshot current under its digest. Every caller reads it the same way a
    /// successful store reads; it is separate only because there is no
    /// [`StoreOutcome`] to invent for a write that never happened.
    ///
    /// The slot is told the answer here rather than by each caller: a value that
    /// stays resident is served from RAM on every later run, so this is the last
    /// moment anything would notice a write that did not land, and a caller that
    /// forgot to pass the answer on would leave the node quietly claiming a
    /// durability it never got.
    pub(crate) async fn store_node(
        &mut self,
        program: &CompiledGraph,
        node_idx: NodeIdx,
        policy: StorePolicy,
        ctx: &mut ContextStore,
    ) -> Option<StoreResult> {
        let target = self.blob_target(program, node_idx)?;
        let snapshot = self.slots[node_idx].current_snapshot()?;
        let outcome = self.disk_store.store(&target, snapshot, policy, ctx).await;
        self.slots[node_idx].note_store(&outcome);
        Some(outcome)
    }

    /// Persist the resident values of `seeds` — [`evict`](Self::evict)'s
    /// counterpart, for a node the host just made disk-backed while its value
    /// was already in RAM.
    ///
    /// The named nodes alone, never their consumer cone: a blob is written from
    /// one node's own snapshot and changes nothing a consumer does. An eviction
    /// has to take the cone for the opposite reason — a surviving consumer would
    /// reuse and prune what was freed.
    pub(crate) async fn flush(
        &mut self,
        program: &CompiledGraph,
        seeds: impl IntoIterator<Item = NodeId>,
        ctx: &mut ContextStore,
    ) -> CacheFlushReport {
        self.flush_each(
            program,
            seeds
                .into_iter()
                .filter_map(|node_id| program.node(node_id)),
            ctx,
        )
        .await
    }

    /// [`flush`](Self::flush) over the whole installed program — what a newly
    /// attached [`DiskStore`] owes every value computed while it was
    /// memory-only.
    pub(crate) async fn flush_all(
        &mut self,
        program: &CompiledGraph,
        ctx: &mut ContextStore,
    ) -> CacheFlushReport {
        self.flush_each(
            program,
            program.e_nodes.iter_indexed().map(|(node_idx, _)| node_idx),
            ctx,
        )
        .await
    }

    /// Store whichever of `nodes` are disk-backed and hold a value current under
    /// their stamped digest — the shared body of the two flushes above. Both
    /// tests are [`store_node`](Self::store_node)'s own, so a node that is
    /// neither costs one [`DiskStore::blob_target`] that answers `None` before
    /// it reads a digest or builds a path.
    ///
    /// [`StorePolicy::PreserveCovering`] because a flush carries no reuse
    /// verdict: nothing here ran, so a blob that already covers the snapshot
    /// must survive untouched.
    ///
    /// `&mut self` without mutating anything, for [`probe_reuse`](Self::probe_reuse)'s
    /// reason: the slots hold `Send`-but-not-`Sync` node state, so a *shared*
    /// cache borrow held across these awaits would make the whole worker future
    /// non-`Send`.
    async fn flush_each(
        &mut self,
        program: &CompiledGraph,
        nodes: impl Iterator<Item = NodeIdx>,
        ctx: &mut ContextStore,
    ) -> CacheFlushReport {
        let mut report = CacheFlushReport::default();
        for node_idx in nodes {
            // A node with nothing to write is not a shortfall — it holds no
            // value to persist, which is the ordinary state of one that has
            // not run.
            let Some(outcome) = self
                .store_node(program, node_idx, StorePolicy::PreserveCovering, &mut *ctx)
                .await
            else {
                continue;
            };
            let node_id = program.node_ids[node_idx];
            match outcome {
                Ok(StoreOutcome::Published | StoreOutcome::AlreadyCovered) => {}
                Ok(StoreOutcome::Unsupported { type_id }) => report
                    .unsupported
                    .push(CacheFlushUnsupported { node_id, type_id }),
                Err(error) => report.failures.push(CacheNodeFailure {
                    node_id,
                    cause: CacheNodeError::Store(error),
                }),
            }
        }
        report
    }

    /// Release resident values that cannot be a future RAM hit under the installed program.
    /// Called both when a program is installed and after each run, so cache-mode downgrades,
    /// impure outputs, and superseded snapshots do not wait for another execution to free RAM.
    pub(crate) fn release_dead_outputs(&mut self, program: &CompiledGraph) {
        for (node_idx, e_node) in program.e_nodes.iter_indexed() {
            let Some(resident_len) = self.slots[node_idx].output_values().map(<[_]>::len) else {
                continue;
            };
            // A snapshot holding a different number of values cannot
            // describe this node's outputs, whatever its digest says.
            //
            // The digest is the only other thing keeping a snapshot
            // alive, and it does not have to move: a func that grows an
            // output while keeping its id reuses the lowered
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

/// Whether a path string names nothing — the state a path port sits in before
/// a file is chosen, whose placeholder is
/// [`DataType::default_value`](crate::DataType::default_value)'s empty string.
///
/// Blank after trimming, not merely empty: whitespace is what a typed-in path
/// collects, and no such string names a file anyone meant. The path itself is
/// never trimmed — spaces are legal at either end of a filename, so rewriting
/// one would read a file other than the authored one.
///
/// A free fn because the question is the string's alone, and because the queue
/// side and the fold side must answer it identically: two copies of the rule
/// would drift into a path that is walked but not folded, or the reverse.
fn is_unset_path(path: &str) -> bool {
    path.trim().is_empty()
}

#[cfg(test)]
pub(crate) mod internals {
    use common::CancelToken;

    use crate::execution::cache::digest::Digest;
    use crate::execution::cache::disk_store::DiskStore;
    use crate::execution::cache::resource::FsPathId;
    use crate::execution::cache::resource::error::StampError;
    use crate::execution::cache::runtime::RuntimeCache;
    use crate::execution::cache::slot::OutputSnapshot;
    use crate::execution::compile::compiled_graph::CompiledGraph;
    use crate::execution::identity::NodeIdx;

    impl RuntimeCache {
        /// A first install of `program` — the production pairing without an
        /// outgoing program to leave. Tests that reinstall name the outgoing one
        /// themselves, through [`RuntimeCache::reconcile`], the way
        /// [`ExecutionEngine::install`](crate::execution::engine::ExecutionEngine)
        /// does.
        pub(crate) fn install_for_test(&mut self, program: &CompiledGraph) {
            self.reconcile(None, program);
        }

        /// The attached store, for the tests that read its I/O counters or
        /// corrupt a blob behind the cache's back.
        pub(crate) fn disk_store(&self) -> &DiskStore {
            &self.disk_store
        }

        /// [`RuntimeCache::identify`] on this thread — the same queue-then-walk
        /// pass, without the blocking pool a test has no runtime to reach. A
        /// path that will not stamp simply does not land, exactly as in the
        /// batched pre-run pass.
        pub(crate) fn prepare_node_blocking(&mut self, program: &CompiledGraph, node_idx: NodeIdx) {
            let _ = self.prepare_nodes_blocking(program, [node_idx]);
        }

        /// [`RuntimeCache::prepare`]'s batching on this thread: every node's
        /// paths queued, then one walk. Hands back what production discards,
        /// so a test can say whether the shared pass survived.
        pub(crate) fn prepare_nodes_blocking(
            &mut self,
            program: &CompiledGraph,
            nodes: impl IntoIterator<Item = NodeIdx>,
        ) -> Result<(), StampError> {
            for node_idx in nodes {
                self.request_node_paths(program, node_idx);
            }
            self.stamp_queued(&CancelToken::never())
        }

        pub(crate) fn stamp_queued(&mut self, cancel: &CancelToken) -> Result<(), StampError> {
            let resolved = self.stamp_job.run(cancel);
            self.fs_paths.extend(self.stamp_job.drain_stamped());
            resolved
        }

        /// Plant a file identity without touching a filesystem, so a
        /// digest folding a path can be pinned to a constant.
        pub(crate) fn stamp_file(&mut self, path: &str, len: u64, mtime_ns: i128) {
            self.fs_paths
                .insert(path.to_string(), FsPathId::file(len, mtime_ns));
        }

        /// Plant a whole snapshot under the digest it is to count as produced
        /// by, so a reuse test can start from a value no run computed.
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
