//! The run loop and its transient state. The `Executor` owns the shared
//! `ctx_manager` and the invoke scratch; the per-node cross-run cache lives in
//! the [`RuntimeCache`](crate::execution::cache::RuntimeCache). Given an immutable
//! [`ExecutionProgram`](crate::execution::program::ExecutionProgram), a prepared
//! [`ExecutionPlan`](crate::execution::plan::ExecutionPlan), and that `RuntimeCache`,
//! [`Executor::run`] invokes each scheduled node's lambda and gathers stats.
//! Each node's per-run result is one [`NodeOutcome`] in the per-run outcome column.
//!
//! **Pre-run resolution.** [`run`](Executor::run) takes the
//! [`Resolver`](crate::execution::resolve::Resolver)'s [`Disposition`] column — each node's
//! merged reuse-verdict + cut state, authoritative for the whole run (a `Reuse` is never
//! re-derived here, since a digest folds live filesystem state and could drift mid-run). A
//! cut node (its cone feeds only cache hits, so a disk-cached node's stale upstream isn't
//! recomputed on reopen) gets [`NodeOutcome::Cut`]. The one verdict the loop *improves* is
//! a `Run` whose stamped digest is `None` — a digest folding a Bind-delivered resource
//! value that exists only once its producers settle: the loop re-stamps it at reach time
//! and serves the cache on a hit. The run loop otherwise walks the schedule unchanged.

use std::sync::Arc;
use std::time::Instant;

use tokio::sync::mpsc::UnboundedSender;

use common::CancelToken;

use crate::data::{DynamicValue, RamUsage};
use crate::execution::stats::{
    ExecutedNodeStats, ExecutionStats, FlattenMap, NodeError, RunPhase, RunProgress,
};
use crate::graph::InputPort;
use crate::node::func_lambda::{InvokeError, InvokeInput, OutputUsage};
use crate::runtime::context::ContextManager;

use crate::execution::cache::RuntimeCache;
use crate::execution::plan::{ExecutionPlan, input_missing};
use crate::execution::program::{ExecutionBinding, ExecutionProgram, NodeIdx};
use crate::execution::resolve::Disposition;
use crate::execution::{NodeColumn, RunError};

/// What became of a node this run — the single per-node result column, so the run-time
/// facts can't contradict (a node can't be `Reused` yet carry a run time, or `Ran` yet
/// also flagged errored). Carries its own `RunError`/elapsed, so nothing lives in a side
/// column.
#[derive(Debug, Clone, Default)]
enum NodeOutcome {
    /// Not reached this run: skipped for missing inputs, below a cancel, or unscheduled.
    #[default]
    Pending,
    /// Served from a RAM/disk cache under an unchanged digest — counted as cached.
    Reused,
    /// Pruned by the pre-run cut: every consumer that would read this node reused a cache,
    /// so its output is never read and its lambda is skipped. `cached` is whether it still
    /// holds a usable value (a deeper disk cache) — reported cached — vs. a memory-only
    /// upstream with nothing resident, which is simply not computed this run.
    Cut { cached: bool },
    /// Its lambda ran and succeeded, taking `secs`.
    Ran { secs: f64 },
    /// Its lambda ran but errored — an invoke failure, or a cancel mid-invoke.
    Failed { secs: f64, error: RunError },
    /// Never ran — an upstream dependency errored, a bound input's blob failed to load,
    /// or its func has no implementation attached.
    Skipped { error: RunError },
}

impl NodeOutcome {
    /// The run error the node carries — a failed run, or a node skipped for an error.
    fn error(&self) -> Option<&RunError> {
        match self {
            NodeOutcome::Failed { error, .. } | NodeOutcome::Skipped { error } => Some(error),
            _ => None,
        }
    }
}

#[derive(Default, Debug)]
pub(crate) struct Executor {
    pub(crate) ctx_manager: ContextManager,
    /// Per-*invoke* scratch: the node's resolved inputs, refilled for each node that runs.
    inputs: Vec<InvokeInput>,
    /// The run's live per-output consumer count, indexed by output-pool index and seeded
    /// each run from the plan's `output_usage`. It serves two roles at once: sliced per node
    /// it's the [`OutputUsage`] a lambda reads to skip unwanted outputs (still at its initial
    /// value when the node runs, since producers run before their consumers), and it counts
    /// *down* as each running consumer reads a bound producer — an output reaching `Skip`
    /// (zero left) is spent, so a non-retained node's value is cleared one output at a time
    /// and, once every output is spent, its whole slot is reclaimed
    /// ([`RuntimeCache::reclaim_slot`]) to trim peak RAM instead of holding it to end-of-run
    /// eviction. Reused across runs, reset each run.
    output_usage: Vec<OutputUsage>,
    /// The run's retention policy, per node: `true` when the outputs must stay resident —
    /// the node's mode caches in RAM (`Ram`/`Both`) or it's a node-seeded preview root
    /// (`plan.pinned`). The single predicate every free/keep decision consults: the
    /// move-on-last-use take, the spent-output release, the post-invoke drain reclaim
    /// (all in this module), and the engine's end-of-run
    /// [`evict_unused`](RuntimeCache::evict_unused). Reused across runs, rebuilt each run.
    pub(crate) retain: NodeColumn<bool>,
    /// Per-run outcome per node (see [`NodeOutcome`]), indexed by `e_node_idx`. Reused
    /// across runs, reset to node count each run. The one per-node result column.
    outcomes: NodeColumn<NodeOutcome>,
}

impl Executor {
    /// Whether node `idx` actually recomputed its lambda in the last run — i.e. wasn't
    /// reused from RAM/disk. Before any run (empty column) every node reads as "ran", so
    /// plan-only introspection still sees the full schedule. Test introspection only.
    #[cfg(test)]
    pub(crate) fn ran(&self, idx: NodeIdx) -> bool {
        idx.idx() >= self.outcomes.len()
            || matches!(
                self.outcomes[idx],
                NodeOutcome::Ran { .. } | NodeOutcome::Failed { .. }
            )
    }

    /// Walk `plan.process_order` (producer-first). For each node: skip it as
    /// [`NodeOutcome::Cut`] if the resolver pruned its cone, serve it from RAM/disk on
    /// [`Disposition::Reuse`], else invoke its lambda and persist the result to disk right
    /// away (so a long run's earlier caches survive a later failure or cancel). The
    /// `program`, `plan`, and `disposition` column are read-only. Returns per-run stats.
    #[allow(clippy::too_many_arguments)] // an orchestration entry point; each arg is a distinct collaborator
    pub(crate) async fn run(
        &mut self,
        program: &ExecutionProgram,
        plan: &ExecutionPlan,
        disposition: &NodeColumn<Disposition>,
        cache: &mut RuntimeCache,
        flatten: &FlattenMap,
        progress: Option<&UnboundedSender<RunProgress>>,
        cancel: CancelToken,
    ) -> ExecutionStats {
        let start = Instant::now();
        // Hold the cancel flag on the context so lambdas can poll it inside
        // off-thread work, and so the loop-top / post-loop checks below read
        // one source.
        self.ctx_manager.cancel = cancel;
        self.ctx_manager.logs.clear();
        self.outcomes
            .reset(program.e_nodes.len(), NodeOutcome::Pending);

        // Build the run's retention policy (see the field doc): RAM-caching mode or pinned.
        self.retain.reset(program.e_nodes.len(), false);
        for idx in program.node_indices() {
            self.retain[idx] = program.e_nodes[idx].cache.caches_in_ram();
        }
        for &idx in &plan.pinned {
            self.retain[idx] = true;
        }

        // Seed the run's live per-output consumer counts from the plan (`0` ⇒ `Skip`, the
        // lambda-facing "nobody reads this output"; `> 0` ⇒ `Needed`). This same column is
        // counted down as consumers read, marking each spent output (see `collect_inputs` and
        // the post-invoke release below).
        self.output_usage.clear();
        self.output_usage.extend(plan.output_usage.iter().map(|&c| {
            if c == 0 {
                OutputUsage::Skip
            } else {
                OutputUsage::Needed(c)
            }
        }));
        // A pinned root's outputs have a real reader *after* the run — the preview fetch —
        // so an output with zero in-run consumers still seeds `Needed(1)`: a usage-honoring
        // lambda must compute it. Keeping the value is `retain`'s concern, not the count's.
        for &idx in &plan.pinned {
            for usage in &mut self.output_usage[program.e_nodes[idx].outputs.range()] {
                if *usage == OutputUsage::Skip {
                    *usage = OutputUsage::Needed(1);
                }
            }
        }

        // The schedule is `process_order` (all reachable, producer-first). A
        // `MissingInputs` node can't run, so it's skipped here rather than pruned by a
        // separate pass; a runnable node whose *only* consumer is one of those may then
        // run needlessly (its output is unread — harmless, and missing inputs are an
        // error state anyway).
        for &e_node_idx in &plan.process_order {
            // Coarse cancel: stop scheduling further nodes. A node already
            // mid-invoke isn't interrupted (it finishes), but nothing after
            // it starts. The unreached tail stays `Pending`, which the stats
            // ignore.
            if self.ctx_manager.cancel.is_cancelled() {
                break;
            }
            let e_node = &program.e_nodes[e_node_idx];
            if disposition[e_node_idx] == Disposition::Cut {
                // Pruned by the pre-run cut: every consumer that would read this node reused
                // a cache, so its output is never read. Report it cached iff it still holds a
                // usable value (a deeper disk cache), else it's simply not computed this run.
                self.outcomes[e_node_idx] = NodeOutcome::Cut {
                    cached: cache.has_available_value(e_node_idx),
                };
                continue;
            }
            if !plan.verdicts[e_node_idx].wants_execute() {
                continue;
            }
            // A func registered without an implementation can't execute — a host/library
            // configuration error, reported on the node every run (before the reuse check,
            // so it can't flicker with cache state); its consumers skip as errored-upstream.
            if e_node.lambda.is_none() {
                mark_skipped(
                    cache,
                    &mut self.outcomes,
                    e_node_idx,
                    RunError::MissingLambda {
                        func_id: e_node.func_id,
                    },
                );
                continue;
            }

            // The resolver's pre-run verdict is authoritative — a `Reuse` is never
            // re-derived here, since its producers may already be pruned (see `resolve.rs`).
            // The one sanctioned improvement is a `Run` whose stamped digest is `None`: the
            // resolver taints a node whose digest folds a Bind-delivered resource value it
            // couldn't read yet (`hash_bound_resource`). Its producers settled earlier in
            // this walk (the `Run` verdict kept them alive), so re-stamp it now and serve
            // the cache on a hit — a genuinely uncacheable node (an impure cone) just
            // re-folds to `None` and runs as before. Reuse is served *before* the
            // errored-dependency check: a digest-valid cached value stays valid even when an
            // upstream re-ran for another consumer and failed, so it must not be cleared as
            // skipped.
            let reused = match disposition[e_node_idx] {
                Disposition::Reuse => true,
                Disposition::Run if cache.slots[e_node_idx].current_digest.is_none() => {
                    hydrate_resource_producers(program, cache, e_node_idx).await;
                    cache.stamp_and_check_reuse(program, e_node_idx)
                }
                _ => false,
            };
            if reused {
                self.outcomes[e_node_idx] = NodeOutcome::Reused;
                continue;
            }

            let func_id = e_node.func_id;

            if has_errored_dependency(program, &self.outcomes, e_node_idx) {
                mark_skipped(
                    cache,
                    &mut self.outcomes,
                    e_node_idx,
                    RunError::SkippedUpstream { func_id },
                );
                continue;
            }

            // Load the node's inputs, pulling any disk-cached producer in on demand (the
            // lazy frontier read) and releasing each producer whose last read this
            // satisfies. A bound producer whose blob failed to load drops this node for
            // the run under its own reason (`InputLoadFailed`) — unlike an errored
            // dependency, there is no upstream error to point at.
            if let Err(error) = collect_inputs(
                program,
                cache,
                &mut self.output_usage,
                &self.retain,
                e_node_idx,
                &mut self.inputs,
            )
            .await
            {
                mark_skipped(cache, &mut self.outcomes, e_node_idx, error);
                continue;
            }

            let output_count = e_node.outputs.len as usize;
            let event_state = cache.slots[e_node_idx].event_state.clone();
            assert!(matches!(self.outcomes[e_node_idx], NodeOutcome::Pending));

            // Attribute any logs this node emits to it (read by
            // `ContextManager::log`).
            let flat_id = e_node.id;
            self.ctx_manager.current_node = Some(flat_id);
            let invoke_start = Instant::now();
            if let Some(progress) = progress {
                let _ = progress.send(RunProgress {
                    nodes: flatten.attribution(flat_id).collect(),
                    phase: RunPhase::Started { at: invoke_start },
                });
            }
            // The node's own outputs are still at their seeded counts (producers run before
            // consumers), so this slice is the `OutputUsage` the lambda reads to skip
            // unwanted outputs.
            let usage = &self.output_usage[e_node.outputs.range()];
            let result = {
                let slot = cache.slots[e_node_idx].invoke_slot(output_count);
                e_node
                    .lambda
                    .invoke(
                        &mut self.ctx_manager,
                        slot.state,
                        &event_state,
                        &mut self.inputs,
                        usage,
                        slot.outputs,
                    )
                    .await
                    .map_err(|e| match e {
                        // A lambda that bailed on cancel reports it truthfully;
                        // surface it as a cancel rather than a generic invoke error.
                        InvokeError::Cancelled => RunError::Cancelled { func_id },
                        other => RunError::Invoke {
                            func_id,
                            message: other.to_string(),
                        },
                    })
            };

            let run_time = invoke_start.elapsed().as_secs_f64();
            // A cancellable lambda reports a cancel itself (→ `RunError::Cancelled`
            // above). This is the safety net for the rest: a lambda that doesn't
            // poll the token (a builtin, a single decode) but ran while the run
            // was cancelled returns `Ok` with a result from an aborted run — map
            // that to `Cancelled` too so its output isn't cached. A genuine
            // error stands on its own, even mid-cancel.
            let result = match result {
                Ok(()) if self.ctx_manager.cancel.is_cancelled() => {
                    Err(RunError::Cancelled { func_id })
                }
                other => other,
            };
            let cancelled = matches!(&result, Err(RunError::Cancelled { .. }));
            let slot = &mut cache.slots[e_node_idx];
            let succeeded = match result {
                // The fresh output now corresponds to this node's current digest; record
                // it so the next run's reuse check is a RAM hit.
                Ok(()) => {
                    slot.stamp_produced();
                    self.outcomes[e_node_idx] = NodeOutcome::Ran { secs: run_time };
                    true
                }
                Err(error) => {
                    slot.clear_output();
                    self.outcomes[e_node_idx] = NodeOutcome::Failed {
                        secs: run_time,
                        error,
                    };
                    false
                }
            };
            // No `Finished` for the cancelled node — it didn't complete; the
            // consumer would otherwise paint it executed live.
            if !cancelled && let Some(progress) = progress {
                let _ = progress.send(RunProgress {
                    nodes: flatten.attribution(flat_id).collect(),
                    phase: RunPhase::Finished {
                        elapsed_secs: run_time,
                    },
                });
            }
            // Persist this node's cache the moment it finishes (durable as the run
            // progresses), not at the end of the whole run. The snapshot is taken
            // synchronously inside `store_node`; only the write awaits, so the cache
            // borrow doesn't cross it.
            if succeeded {
                cache
                    .store_node(program, e_node_idx, &mut self.ctx_manager)
                    .await;
                // A node no consumer reads (a terminal sink, or every output already `Skip`) is
                // spent the instant it's stored — reclaim its non-retained slot now rather
                // than holding it to end-of-run eviction. A node that still owes reads is
                // reclaimed later, in `collect_inputs`, when its last consumer lands.
                if !self.retain[e_node_idx] && node_drained(program, &self.output_usage, e_node_idx)
                {
                    cache.reclaim_slot(program, e_node_idx);
                }
            }
        }

        self.ctx_manager.current_node = None;
        let mut stats = collect_execution_stats(program, plan, &self.outcomes, start);
        stats.logs = std::mem::take(&mut self.ctx_manager.logs);
        stats.cancelled = self.ctx_manager.cancel.is_cancelled();
        stats
    }
}

/// Hydrate the producers feeding node `idx`'s resource-typed bound inputs, so the
/// reach-time re-stamp can read the delivered reference values (an `OnDisk` producer's
/// value lives in its blob). Loading a blob just for the stamp is the cost of wiring a
/// reference over a disk-cached producer. A failed hydrate leaves the value unreadable and
/// the fold taints the digest to `None` — uncacheable, so the node proceeds to run and
/// `collect_inputs` reports the real load failure.
async fn hydrate_resource_producers(
    program: &ExecutionProgram,
    cache: &mut RuntimeCache,
    idx: NodeIdx,
) {
    for input in program.node_inputs(&program.e_nodes[idx]) {
        if input.stamper.is_some()
            && let ExecutionBinding::Bind(addr) = &input.binding
        {
            cache.hydrate_slot(program, addr.target_idx).await;
        }
    }
}

/// Drop node `idx` from this run: clear any stale cached output so it isn't served as
/// this run's result, and record the outcome under the caller's reason —
/// [`RunError::SkippedUpstream`] for an errored dependency, [`RunError::InputLoadFailed`]
/// for a cached input that failed to load, [`RunError::MissingLambda`] for a func with
/// no implementation.
fn mark_skipped(
    cache: &mut RuntimeCache,
    outcomes: &mut NodeColumn<NodeOutcome>,
    idx: NodeIdx,
    error: RunError,
) {
    cache.slots[idx].clear_output();
    outcomes[idx] = NodeOutcome::Skipped { error };
}

fn has_errored_dependency(
    program: &ExecutionProgram,
    outcomes: &NodeColumn<NodeOutcome>,
    e_node_idx: NodeIdx,
) -> bool {
    program.node_inputs(&program.e_nodes[e_node_idx]).iter().any(|input| {
        matches!(&input.binding, ExecutionBinding::Bind(addr) if outcomes[addr.target_idx].error().is_some())
    })
}

/// Resolve `e_node_idx`'s inputs into `inputs`. Fails with [`RunError::InputLoadFailed`]
/// if a bound producer's value can't be materialized — its disk blob was reused-from-disk
/// but failed to load (corrupt/deleted): [`hydrate_slot`](RuntimeCache::hydrate_slot) then
/// removes the bad blob, so this run drops the consumer and the next reopen recomputes the
/// producer fresh. Any other missing value is a planner bug.
async fn collect_inputs(
    program: &ExecutionProgram,
    cache: &mut RuntimeCache,
    output_usage: &mut [OutputUsage],
    retain: &NodeColumn<bool>,
    e_node_idx: NodeIdx,
    inputs: &mut Vec<InvokeInput>,
) -> Result<(), RunError> {
    inputs.clear();
    let node_inputs = program.node_inputs(&program.e_nodes[e_node_idx]);
    for (input_idx, e_input) in node_inputs.iter().enumerate() {
        let value = match &e_input.binding {
            ExecutionBinding::None => DynamicValue::Unbound,
            ExecutionBinding::Const(v) => v.into(),
            ExecutionBinding::Bind(addr) => {
                let (target, port) = (addr.target_idx, addr.port_idx);
                // The producer settled earlier this run: it either ran (resident) or was
                // reused-from-disk (marked `OnDisk`, not yet loaded) — hydrate the latter
                // now, the lazy frontier read. A resident value is a no-op load; a failed
                // one leaves the slot empty and drops this consumer for the run.
                cache.hydrate_slot(program, target).await;
                let out_idx = program.e_nodes[target].outputs.start as usize + port;
                // Move-on-last-use: on a non-retained output's last read the release below
                // drops the RAM copy anyway, so hand the consumer the slot's value
                // itself — it becomes the sole `Arc` holder and `into_custom` can
                // reuse the allocation in place. A retained or still-owed output
                // is cloned as before.
                let take =
                    matches!(output_usage[out_idx], OutputUsage::Needed(1)) && !retain[target];
                let Some(value) = cache.read_output_port(program, target, port, take) else {
                    return Err(RunError::InputLoadFailed {
                        func_id: program.e_nodes[e_node_idx].func_id,
                        input: input_idx,
                    });
                };
                // Count this read against the producer's output. Each `Bind` edge was counted
                // once in the plan's usage, so when this output's count reaches `Skip` every
                // in-run consumer has read it and it's spent — freed one output at a time here.
                match output_usage[out_idx] {
                    OutputUsage::Needed(1) => {
                        output_usage[out_idx] = OutputUsage::Skip;
                        release_spent_output(program, cache, output_usage, retain, target, port);
                    }
                    OutputUsage::Needed(n) => output_usage[out_idx] = OutputUsage::Needed(n - 1),
                    OutputUsage::Skip => {
                        panic!("consumer read output {out_idx} the plan marked as unused (Skip)")
                    }
                }
                value
            }
        };
        inputs.push(InvokeInput { value });
    }
    Ok(())
}

/// The last consumer of `target`'s output `port` just took its copy. A retained node
/// (RAM-caching mode, or a pinned preview root) stays hot; for any other, that spent output
/// is freed now rather than at end-of-run eviction — the whole slot reclaimed
/// ([`RuntimeCache::reclaim_slot`], demoted to disk or dropped) once *every* output is spent,
/// else just this one value cleared while sibling outputs are still owed to other consumers.
fn release_spent_output(
    program: &ExecutionProgram,
    cache: &mut RuntimeCache,
    output_usage: &[OutputUsage],
    retain: &NodeColumn<bool>,
    target: NodeIdx,
    port: usize,
) {
    if retain[target] {
        return;
    }
    if node_drained(program, output_usage, target) {
        cache.reclaim_slot(program, target);
    } else {
        cache.clear_output_port(target, port);
    }
}

/// Whether every output of node `idx` is spent — no consumer this run still owes a read
/// (each output is `Skip`, either seeded that way with zero consumers or counted down to it).
fn node_drained(program: &ExecutionProgram, output_usage: &[OutputUsage], idx: NodeIdx) -> bool {
    let outputs = program.e_nodes[idx].outputs;
    output_usage[outputs.range()]
        .iter()
        .all(|u| matches!(u, OutputUsage::Skip))
}

fn collect_execution_stats(
    program: &ExecutionProgram,
    plan: &ExecutionPlan,
    outcomes: &NodeColumn<NodeOutcome>,
    start: Instant,
) -> ExecutionStats {
    let mut executed_nodes = Vec::new();
    let mut missing_inputs = Vec::new();
    let mut cached_nodes = Vec::new();
    let mut node_errors = Vec::new();

    // The schedule (and its per-node outcomes) is `process_order`. Each node's outcome is
    // the sole source of truth; a node the run never reached (a cancelled run's tail, or
    // skipped for missing inputs) is `Pending` and contributes to no list here.
    for &idx in &plan.process_order {
        let e = &program.e_nodes[idx];
        match &outcomes[idx] {
            // A reuse hit, or a node the cut pruned that still holds a value, are both
            // "available, not recomputed" — reported cached. A pruned memory-only node
            // (`Cut { cached: false }`) has no value this run and falls through, uncounted.
            NodeOutcome::Reused | NodeOutcome::Cut { cached: true } => cached_nodes.push(e.id),
            NodeOutcome::Ran { secs } => executed_nodes.push(ExecutedNodeStats {
                node_id: e.id,
                elapsed_secs: *secs,
            }),
            // A cancelled invoke didn't complete — omit it from the executed set so the
            // consumer doesn't paint it as executed (its error still lands below). A
            // genuine failure did run; it appears in both lists.
            NodeOutcome::Failed { secs, error } if !matches!(error, RunError::Cancelled { .. }) => {
                executed_nodes.push(ExecutedNodeStats {
                    node_id: e.id,
                    elapsed_secs: *secs,
                });
            }
            _ => {}
        }
        if plan.verdicts[idx].missing_required_inputs() {
            // Recompute which ports are unsatisfied (shares `input_missing` with the
            // planner) — only for the rare missing node, so it isn't worth a stored column.
            for (i, input) in program.node_inputs(e).iter().enumerate() {
                if input_missing(input, &plan.verdicts) {
                    missing_inputs.push(InputPort::new(e.id, i));
                }
            }
        }
        if let Some(err) = outcomes[idx].error() {
            node_errors.push(NodeError {
                node_id: e.id,
                error: err.clone(),
            });
        }
    }

    ExecutionStats {
        elapsed_secs: start.elapsed().as_secs_f64(),
        executed_nodes,
        missing_inputs,
        cached_nodes,
        triggered_events: Vec::default(),
        node_errors,
        // Filled by `run` from the context manager's per-run buffer.
        logs: Vec::new(),
        // Filled by `ExecutionEngine::execute` from the flatten pass; the executor
        // doesn't know the authoring graph.
        flatten: Arc::default(),
        // Set by `run` from the cancel flag after the loop.
        cancelled: false,
        // Stamped by `ExecutionEngine::execute` after end-of-run eviction, when
        // the cache's resident set is final.
        cache_ram: RamUsage::default(),
        node_ram: Vec::new(),
    }
}

#[cfg(test)]
mod tests;
