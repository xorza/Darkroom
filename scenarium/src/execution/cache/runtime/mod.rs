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
use std::ops::{Index, IndexMut};
use std::sync::Arc;

use common::CancelToken;
use hashbrown::HashMap;

use crate::execution::cache::digest::{DOMAIN, Digest, DigestHasher, InputTag};
use crate::execution::cache::disk_store::{BlobTarget, DiskStore, StorePolicy};
use crate::execution::cache::resource::{FsPathId, StampError, StampJob};
use crate::execution::cache::slot::{RuntimeSlot, StateOwner, ValueState};
use crate::execution::identity::ExecutionNodeId;
use crate::execution::program::index::{NodeColumn, NodeIdx, OutputAddr};
use crate::execution::program::{ExecutionBinding, Program};
use crate::execution::ram::NodeRamUsage;
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
    /// Private: the *column* is the cache's own — its length is the alignment
    /// invariant [`reconcile`](Self::reconcile) establishes, so nothing outside
    /// may push, drain, or resize it. Individual slots are reached by
    /// [`Index<NodeIdx>`], the same way a node is reached on [`Program`].
    slots: NodeColumn<RuntimeSlot>,
    pub(crate) disk_store: DiskStore,
    /// What each path this run reads *was*, identified once. Held beside the
    /// slots rather than inside the walker that fills it, because the digest
    /// fold reads both together — a path's identity, and the producer slot
    /// that delivered the path.
    fs_paths: HashMap<String, FsPathId>,
    /// The off-thread walk that fills `fs_paths`: queue, then pass. It owns
    /// only what crosses to the blocking pool.
    stamp_job: StampJob,
    ram_seen: HashSet<usize>,
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

#[derive(Debug)]
pub(crate) struct CacheEvictionFailure {
    pub(crate) e_node_id: ExecutionNodeId,
    pub(crate) message: String,
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

    pub(crate) fn clear(&mut self) {
        self.slots.clear();
        self.fs_paths.clear();
        self.stamp_job.requests.clear();
    }

    pub(crate) async fn evict(
        &mut self,
        program: &Program,
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
    ///
    /// `program` names the slots: they are index-aligned to it by
    /// [`reconcile`](Self::reconcile), so its id column is the cache's too.
    pub(crate) fn resident_ram_stats(
        &mut self,
        program: &Program,
        by_node: &mut Vec<NodeRamUsage>,
    ) -> RamUsage {
        debug_assert_eq!(self.slots.len(), program.e_nodes.len());
        self.ram_seen.clear();
        by_node.clear();
        let mut total = RamUsage::default();
        for (e_node_id, slot) in program.e_node_ids.iter().zip(self.slots.iter()) {
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

    /// Realign the slots from the program they currently belong to onto a newly
    /// installed one: re-pair them with the new index order by stable id —
    /// dropping persistent state whose owning implementation (func id +
    /// version) changed — default new nodes, trim removed ones, and apply the
    /// installed program's RAM-retention policy immediately. The one place ids
    /// are hashed for slot access; every per-run access is an index read.
    ///
    /// `installed` is the program the slots are aligned to when this returns,
    /// so it must be called before the engine swaps its artifact — naming the
    /// two programs is what lets the slots be a bare column rather than one
    /// carrying a duplicate of `installed.e_node_ids` to interpret itself by.
    pub(crate) fn reconcile(&mut self, previous: &Program, installed: &Program) {
        debug_assert_eq!(self.slots.len(), previous.e_nodes.len());
        let mut retained: HashMap<ExecutionNodeId, RuntimeSlot> = previous
            .e_node_ids
            .iter()
            .copied()
            .zip(self.slots.drain())
            .collect();
        for (e_node_id, e_node) in installed.e_node_ids.iter().zip(installed.e_nodes.iter()) {
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
            self.slots.push(slot);
        }
        self.release_dead_outputs(installed);
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

    /// Read a producer output for a consumer: a clone of the value, or — with
    /// `take` — the value itself, moved out of the slot (leaving `Unbound`). The move is the
    /// executor's last-read fast path for a non-RAM producer: the RAM copy would be released
    /// right after anyway, and handing over the slot's copy leaves the consumer as the sole
    /// `Arc` holder so [`DynamicValue::into_custom`] can reuse the allocation in place.
    /// `None` when the slot holds no resident values.
    pub(crate) fn read_output_port(
        &mut self,
        program: &Program,
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

    /// Producer-first digest pass over the whole schedule, so a consumer
    /// folds an already-stamped producer digest. Reuse is deliberately not
    /// probed here because exact demand exists only in the resolver's
    /// reverse sweep. A Bind-delivered path value that is not resident yet
    /// stamps `None`; the run loop can improve that node to reuse once its
    /// path producer settles.
    /// `executing` is the run's schedule in producer-first order
    /// ([`ExecutionPlan::executing`](crate::execution::plan::ExecutionPlan::executing));
    /// the cache reads the nodes, not the plan that selected them.
    pub(crate) fn stamp_digests(
        &mut self,
        program: &Program,
        executing: impl IntoIterator<Item = NodeIdx>,
    ) {
        for node_idx in executing {
            self.stamp_digest(program, node_idx);
        }
    }

    /// Stamp one node's structural content digest into its slot. The resolver's
    /// pass calls this before exact output demand is known; cache coverage is
    /// probed later by [`probe_reuse`](Self::probe_reuse).
    pub(crate) fn stamp_digest(&mut self, program: &Program, node_idx: NodeIdx) {
        // Folded whole before the write, so the fold's read of the slots ends
        // before the slot it stamps is borrowed mutably.
        let digest = self.node_digest(program, node_idx);
        self.slots[node_idx].current_digest = digest;
    }

    /// A node's **content digest** — the one content key it's cached under, folding its identity
    /// (func id + version + output types) plus its structural inputs. The single digest the whole
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
    /// A method on the cache because all three things it folds are the cache's: the slots it
    /// reads producer digests and delivered values from, and the `fs_paths` memo behind every
    /// external identity. The *encoding* stays in `digest`, beside the [`DOMAIN`] versioning it.
    pub(crate) fn node_digest(&self, program: &Program, node_idx: NodeIdx) -> Option<Digest> {
        let e_node = &program[node_idx];

        // Only a `Pure` node is content-cacheable; an `Impure` node varies per run, so it has no
        // digest and always recomputes.
        if e_node.behavior != FuncBehavior::Pure {
            return None;
        }

        let mut hasher = DigestHasher::new();
        hasher
            .write_bytes(DOMAIN)
            .write_pod(e_node.func_id.as_u128())
            .write_pod(e_node.version);

        let outputs = &program.outputs[e_node.outputs];
        hasher.write_pod(outputs.len() as u64);
        for output in outputs {
            hasher.write_data_type(&output.data_type);
        }

        for input in &program.inputs[e_node.inputs] {
            match &input.binding {
                ExecutionBinding::None => {
                    hasher.write_input_tag(InputTag::Unbound);
                }
                ExecutionBinding::Const(value) => {
                    hasher.write_input_tag(InputTag::Const);
                    hasher.write_static(value);
                    self.hash_fs_paths(&mut hasher, value.as_fs_paths())?;
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
        // `current_output_values`, so a value produced under an older digest
        // cannot deliver a reference into this key.
        let delivered = self.slots[addr.node_idx]
            .current_output_values()?
            .get(addr.port_idx as usize)?;
        match delivered.as_fs_paths() {
            Some(paths) => {
                hasher.write_input_tag(InputTag::BoundPaths);
                self.hash_fs_paths(hasher, Some(paths))?;
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
    /// The two `Option`s mean opposite things. A value naming *no* paths
    /// folds nothing and succeeds — that is a plain const, not a
    /// filesystem read. Returning `None` is the failure: a path this run
    /// never stamped, leaving its node without a sound cache key.
    fn hash_fs_paths(&self, hasher: &mut DigestHasher, paths: Option<&[String]>) -> Option<()> {
        let Some(paths) = paths else {
            return Some(());
        };
        hasher.write_pod(paths.len() as u64);
        for path in paths {
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
        program: &Program,
        executing: impl IntoIterator<Item = NodeIdx>,
        cancel: CancelToken,
    ) {
        // A fresh run identifies afresh.
        self.fs_paths.clear();
        self.stamp_job.requests.clear();
        let _ = self.identify(program, executing, cancel).await;
    }

    /// Identify every path `nodes` reads, in one off-thread pass: queue, then
    /// walk. Adds to the run's memo — [`Self::prepare`] is what starts a fresh
    /// one.
    async fn identify(
        &mut self,
        program: &Program,
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
    fn request_node_paths(&mut self, program: &Program, node_idx: NodeIdx) {
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
                if !self.fs_paths.contains_key(path) {
                    self.stamp_job.requests.insert(path.clone());
                }
            }
        }
    }

    /// Run the queued pass on the blocking pool, leaving the queue empty and
    /// what it identified in the memo.
    ///
    /// The job goes out and comes back — it owns its queue and scratch, so
    /// nothing of the cache is borrowed across the boundary and the memo never
    /// moves at all. It returns on the failing path too, which is what keeps a
    /// run that hits one unreadable path from re-walking every directory it had
    /// already identified.
    async fn walk_queued(&mut self, cancel: CancelToken) -> Result<(), StampError> {
        if self.stamp_job.requests.is_empty() {
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
        self.fs_paths.extend(self.stamp_job.stamped.drain(..));
        resolved
    }

    /// Identify one node's paths, stamp the digest they complete, and
    /// hydrate its value if that digest now hits — the whole late second
    /// chance at reuse, for a node whose digest the pre-run pass could not
    /// fold because a wired path had no value yet.
    ///
    /// Returns what [`Self::hydrate_reuse`] does, and means it the same
    /// way: `true` once the value is resident under the new digest and
    /// ready to serve, `false` when there was nothing to serve and the
    /// lambda still has to run. Every path here is declared by `node_idx`
    /// alone, so a stamping failure is that node's — the caller fails it
    /// and the ordinary errored-upstream cascade takes its dependents,
    /// rather than a run-wide abort blaming nobody.
    pub(crate) async fn restamp_and_hydrate(
        &mut self,
        program: &Program,
        node_idx: NodeIdx,
        demand: &[OutputDemand],
        contexts: &mut ContextStore,
        cancel: CancelToken,
    ) -> Result<bool, StampError> {
        self.identify(program, std::iter::once(node_idx), cancel)
            .await?;
        self.stamp_digest(program, node_idx);
        Ok(self
            .hydrate_reuse(program, node_idx, demand, contexts)
            .await)
    }

    /// Blobs are named by stable id, so they survive installs that shift indices.
    fn blob_target(&self, program: &Program, node_idx: NodeIdx) -> Option<BlobTarget> {
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
        program: &Program,
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
        program: &Program,
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
    /// `false` when nothing loads: no usable blob, or one that stopped decoding since the
    /// probe. The decode path deletes an undecodable blob, so the next run misses cleanly
    /// and recomputes.
    pub(crate) async fn hydrate_reuse(
        &mut self,
        program: &Program,
        node_idx: NodeIdx,
        demand: &[OutputDemand],
        ctx: &mut ContextStore,
    ) -> bool {
        let target = match self.reuse_source(program, node_idx, demand) {
            None => return false,
            Some(ReuseSource::Resident) => return true,
            Some(ReuseSource::Blob(target)) => target,
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
        program: &Program,
        node_idx: NodeIdx,
        policy: StorePolicy,
        ctx: &'a mut ContextStore,
    ) -> impl Future<Output = ()> + 'a {
        let target = self.blob_target(program, node_idx);
        let resident = self.slots[node_idx].current_snapshot();
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
    pub(crate) fn release_dead_outputs(&mut self, program: &Program) {
        for (node_idx, e_node) in program.e_nodes.iter_indexed() {
            let Some(resident_len) = self.slots[node_idx].output_values().map(<[_]>::len) else {
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
    use common::CancelToken;

    use crate::execution::cache::digest::Digest;
    use crate::execution::cache::resource::{FsPathId, StampError};
    use crate::execution::cache::runtime::RuntimeCache;
    use crate::execution::cache::slot::{OutputSnapshot, ValueState};
    use crate::execution::program::Program;
    use crate::execution::program::index::NodeIdx;

    impl RuntimeCache {
        /// [`RuntimeCache::reconcile`] onto a cache that holds nothing yet —
        /// the empty program a `default()` cache belongs to, named once here
        /// rather than spelled at every fixture that starts from one.
        pub(crate) fn reconcile_fresh(&mut self, installed: &Program) {
            self.reconcile(&Program::default(), installed);
        }

        /// [`RuntimeCache::identify`] on this thread — the same queue-then-walk
        /// pass, without the blocking pool a test has no runtime to reach. A
        /// path that will not stamp simply does not land, exactly as in the
        /// batched pre-run pass.
        pub(crate) fn prepare_node_blocking(&mut self, program: &Program, node_idx: NodeIdx) {
            self.request_node_paths(program, node_idx);
            let _ = self.stamp_queued(&CancelToken::never());
        }

        pub(crate) fn stamp_queued(&mut self, cancel: &CancelToken) -> Result<(), StampError> {
            let resolved = self.stamp_job.run(cancel);
            self.fs_paths.extend(self.stamp_job.stamped.drain(..));
            resolved
        }

        /// Plant a file identity without touching a filesystem, so a
        /// digest folding a path can be pinned to a constant.
        pub(crate) fn stamp_file(&mut self, path: &str, len: u64, mtime_ns: i128) {
            self.fs_paths
                .insert(path.to_string(), FsPathId::file(len, mtime_ns));
        }
    }

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
