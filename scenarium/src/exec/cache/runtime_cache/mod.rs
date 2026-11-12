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

use std::collections::HashSet;
use std::iter;
use std::ops::{Index, IndexMut};
use std::sync::Arc;

use common::CancelToken;

use crate::containers::col::Col;
use crate::data::dynamic_value::DynamicValue;
use crate::data::ram_usage::RamUsage;
use crate::exec::cache::digest::{DOMAIN, Digest, DigestHasher, InputTag};
use crate::exec::cache::disk_store::blob_target::BlobTarget;
use crate::exec::cache::disk_store::error::StoreResult;
use crate::exec::cache::disk_store::store_outcome::StoreOutcome;
use crate::exec::cache::disk_store::{DiskStore, StorePolicy};
use crate::exec::cache::path_identities::PathIdentities;
use crate::exec::cache::runtime_cache::cache_flush_report::CacheFlushReport;
use crate::exec::cache::runtime_cache::error::{CacheFlushUnsupported, CacheNodeFailure};
use crate::exec::cache::runtime_slot::{OutputRead, RuntimeSlot};
use crate::exec::cache::stamp_job::error::StampError;
use crate::exec::compile::compiled_graph::{CompiledGraph, ExecBinding};
use crate::exec::compile::consumer_cone::ConsumerCone;
use crate::exec::idx::{NodeIdx, OutputAddr};
use crate::exec::schedule::{Delivery, RunSchedule};
use crate::graph::func::FuncBehavior;
use crate::graph::func::lambda::OutputDemand;
use crate::graph::identity::NodeId;

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

impl IndexMut<NodeIdx> for RuntimeCache {
    fn index_mut(&mut self, node_idx: NodeIdx) -> &mut RuntimeSlot {
        &mut self.slots[node_idx]
    }
}

impl RuntimeCache {
    /// The span of the slot column — the program node count it is aligned to.
    pub(crate) const fn slot_count(&self) -> usize {
        self.slots.len()
    }

    /// Attach the store this cache persists to and serves from. Must be called between runs:
    /// every reuse verdict already given was answered from the previous store.
    pub(crate) fn set_disk_store(&mut self, disk_store: DiskStore) {
        self.disk_store = disk_store;
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
        seeds: impl IntoIterator<Item = NodeId>,
    ) -> Vec<CacheNodeFailure> {
        let downstream = self.cone.of(
            program,
            seeds
                .into_iter()
                .filter_map(|node_id| program.node(node_id)),
        );
        let mut failures = Vec::new();
        for node_idx in downstream {
            let node_id = program.node_ids[node_idx];
            match self.disk_store.remove_node(node_id).await {
                // The removal just made the belief that a blob backed the value false.
                Ok(()) => self.slots[node_idx].clear_output(),
                Err(error) => failures.push(CacheNodeFailure {
                    node_id,
                    cause: error.into(),
                }),
            }
        }
        failures
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
                // An unwritable verdict was the previous program's codecs'.
                slot.reconsider_unwritable_blob();
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
        let arity = program[address.node_idx].outputs.len as usize;
        let slot = &mut self.slots[address.node_idx];
        debug_assert!(
            slot.output_values()
                .is_none_or(|values| values.len() == arity),
            "cached output values must match the node's compiled arity"
        );
        slot.read_output(address.port_idx, read)
    }

    /// Producer-first digest pass over the nodes `schedule` runs ([`RunSchedule::executing`]), so
    /// a consumer folds an already-stamped producer digest. Reuse is not probed here because exact
    /// demand exists only in the resolver's reverse sweep. A Bind-delivered path value that is not
    /// resident yet stamps `None`; the run loop re-stamps once its path producer settles.
    pub(crate) fn stamp_digests(&mut self, program: &CompiledGraph, schedule: &RunSchedule) {
        for node_idx in schedule.executing() {
            self.stamp_digest(program, schedule, node_idx);
        }
    }

    /// Stamp one node's structural content digest into its slot; cache coverage is probed later
    /// by [`probe_reuse`](Self::probe_reuse).
    pub(crate) fn stamp_digest(
        &mut self,
        program: &CompiledGraph,
        schedule: &RunSchedule,
        node_idx: NodeIdx,
    ) {
        // Folded whole first, so the read of the slots ends before the slot is borrowed mutably.
        let digest = self.node_digest(program, schedule, node_idx);
        self.slots[node_idx].cur_digest = digest;
    }

    /// A node's **content digest**, the one key it is cached under: its identity (func id + port
    /// signature) plus its structural inputs. RAM reuse
    /// ([`is_resident_hit`](Self::is_resident_hit)),
    /// disk load/store, and downstream folding all read the stamped `cur_digest`. Computed
    /// producer-first, so a `Wire` producer's digest is already stamped.
    ///
    /// - An **`Impure`** node has no digest (`None`): it never caches, and a `Wire` producer with
    ///   a `None` digest taints this node to `None`.
    /// - Otherwise fold what each input delivers ([`RunSchedule::delivery`]): a `Const`'s value +
    ///   prepared `FsPath` content, a `Wire` producer's digest, or the unbound marker — plus, for
    ///   a resource-typed input, the live identity of the referent behind the *delivered* value
    ///   ([`hash_bound_fs_path`](Self::hash_bound_fs_path)). That fold needs the producer's value:
    ///   unreadable ⇒ `None`, and the run loop re-stamps the node at reach time.
    ///
    /// A method on the cache because the slots and the path memo it folds are the cache's; the
    /// *encoding* stays in `digest`, beside the [`DOMAIN`] versioning it.
    pub(crate) fn node_digest(
        &self,
        program: &CompiledGraph,
        schedule: &RunSchedule,
        node_idx: NodeIdx,
    ) -> Option<Digest> {
        let e_node = &program[node_idx];

        // An `Impure` node varies per run, so it has no digest.
        if e_node.behavior != FuncBehavior::Pure {
            return None;
        }

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
                    if let Some(paths) = value.as_fs_paths() {
                        self.paths.hash(&mut hasher, paths)?;
                    }
                }
                Delivery::Wire(addr) => {
                    // Visited first (topological order), so its `cur_digest` is set.
                    let producer = self.slots[addr.node_idx].cur_digest?;
                    // Digest then port index, both fixed-width, so consumers reading different
                    // ports of one node fold apart.
                    hasher
                        .write_input_tag(InputTag::Wire)
                        .write_digest(&producer)
                        .write_pod(addr.port_idx);
                    // The external state behind the *runtime value* is part of this node's key —
                    // the Bind-side counterpart of the `Const` arm's fold.
                    if input.fs_path {
                        self.hash_bound_fs_path(&mut hasher, addr)?;
                    }
                }
            }
        }
        Some(hasher.finish())
    }

    /// Fold the referent identity behind a **Bind-delivered** resource input, read off the
    /// producer's resident slot, so a wired path re-keys its consumer like a const path does. An
    /// unreadable value (producer not resident) is `None`, tainting the digest; the run loop
    /// re-stamps at reach time, once producers settled and any disk-backed path producer was
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
        let digest = self.slots[node_idx].cur_digest?;
        self.disk_store
            .blob_target(program.node_ids[node_idx], digest)
    }

    /// Whether an unchanged output can satisfy this run's exact demand — the verdict alone,
    /// **without loading anything**, so the resolver can cut a producer cone without paying for a
    /// decode. A `None` digest never reuses.
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
            Some(ReuseSource::Blob(target)) => target.covers_demand(&program.codecs, demand).await,
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
            Some(ReuseSource::Resident) => {
                self.settle_blob_debt(program, node_idx).await;
                return ReuseOutcome::Served;
            }
            Some(ReuseSource::Blob(target)) => target,
        };
        let Some(snapshot) = target.read(&program.codecs, demand).await else {
            return ReuseOutcome::Missed;
        };
        self.slots[node_idx].load_from_blob(snapshot, target.digest);
        ReuseOutcome::Served
    }

    /// Write the blob a resident value owes.
    ///
    /// A `Both`-mode value is served from RAM without the disk being consulted, so nothing else
    /// would notice that the write establishing its durability never landed (a failed store, a
    /// missing codec, a cache mode turned on after the value was resident). Only `invoke_node`
    /// stores, and a node served from RAM does not invoke.
    ///
    /// The debt check is a digest comparison, so a blob this engine already wrote costs no I/O:
    /// an event loop executes on every tick.
    async fn settle_blob_debt(&mut self, program: &CompiledGraph, node_idx: NodeIdx) {
        if !self.slots[node_idx].owes_blob() {
            return;
        }
        // `PreserveCovering`, not `KnownMiss`: the reuse verdict was about RAM, so a broader blob
        // already on disk must survive. It also re-establishes the belief when the debt was only a
        // gap in this engine's knowledge.
        self.store_node(program, node_idx, StorePolicy::PreserveCovering)
            .await;
    }

    /// Write `node_id`'s freshly-computed outputs to disk the moment it finishes, so a long run's
    /// earlier caches are durable even if a later node errors or the run is cancelled.
    /// [`StorePolicy::KnownMiss`] publishes directly after resolution proved reuse impossible;
    /// [`StorePolicy::PreserveCovering`] first protects a broader blob when a maintenance flush
    /// has no such verdict.
    ///
    /// Only writes a value current under the node's digest
    /// ([`is_resident_hit`](Self::is_resident_hit)): a value produced under a superseded digest
    /// must not overwrite the new digest's blob. In the run loop it is always a current hit; this
    /// guards maintenance flushes.
    ///
    /// `None` is a node with nothing to write — not disk-backed, or holding no snapshot current
    /// under its digest; callers read it as a successful store. The slot is told the answer here
    /// rather than by each caller: a value that stays resident is served from RAM on every later
    /// run, so this is the last moment anything would notice a write that did not land.
    pub(crate) async fn store_node(
        &mut self,
        program: &CompiledGraph,
        node_idx: NodeIdx,
        policy: StorePolicy,
    ) -> Option<StoreResult> {
        let target = self.blob_target(program, node_idx)?;
        let snapshot = self.slots[node_idx].cur_snapshot()?;
        let outcome = target.store(&program.codecs, snapshot, policy).await;
        self.slots[node_idx].note_store(&outcome);
        Some(outcome)
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
        seeds: impl IntoIterator<Item = NodeId>,
    ) -> CacheFlushReport {
        self.flush_each(
            program,
            seeds
                .into_iter()
                .filter_map(|node_id| program.node(node_id)),
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
            // A node with nothing to write holds no value to persist; that is not a shortfall.
            let Some(outcome) = self
                .store_node(program, node_idx, StorePolicy::PreserveCovering)
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
            // `produced_under` still equals the stale `cur_digest`.
            let retained = resident_len == e_node.outputs.len as usize
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
                .cur_digest
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
