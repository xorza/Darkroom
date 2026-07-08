//! The run loop and its transient state. The `Executor` owns the shared
//! `ctx_manager` and the invoke scratch; the per-node cross-run cache lives in
//! the [`RuntimeCache`](crate::execution::cache::RuntimeCache). Given an immutable
//! [`ExecutionProgram`](crate::execution::program::ExecutionProgram), a prepared
//! [`ExecutionPlan`](crate::execution::plan::ExecutionPlan), and that `RuntimeCache`,
//! [`Executor::run`] invokes each scheduled node's lambda and gathers stats.
//! Each node's per-run result is one [`NodeOutcome`] in a column local to the run.
//!
//! **Pre-run cut.** [`run`](Executor::run) takes a `needed` mask precomputed by the
//! [`Resolver`](crate::execution::resolve::Resolver): the pruned set of nodes some running
//! node will read, with cones that feed only cache hits cut out (so a disk-cached node's
//! stale upstream isn't recomputed on reopen). A pruned node gets [`NodeOutcome::Cut`]; the
//! run loop otherwise walks the schedule unchanged, [`prepare_node`] re-deriving each
//! surviving node's digest.

use std::sync::Arc;
use std::time::Instant;

use tokio::sync::mpsc::UnboundedSender;

use common::CancelToken;

use crate::data::DynamicValue;
use crate::execution::stats::{
    ExecutedNodeStats, ExecutionStats, FlattenMap, NodeError, RunPhase, RunProgress,
};
use crate::graph::InputPort;
use crate::node::func_lambda::{InvokeError, InvokeInput, OutputUsage};
use crate::node::function::FuncId;
use crate::runtime::context::ContextManager;

use crate::execution::cache::RuntimeCache;
use crate::execution::plan::{ExecutionPlan, input_missing};
use crate::execution::program::{ExecutionBinding, ExecutionProgram, NodeIdx};
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
    /// Never ran — an upstream dependency errored, or a bound input's blob failed to load.
    Skipped { error: RunError },
}

impl NodeOutcome {
    /// The node recomputed its lambda (whether it then succeeded or errored).
    fn ran(&self) -> bool {
        matches!(self, NodeOutcome::Ran { .. } | NodeOutcome::Failed { .. })
    }
    /// Elapsed lambda time, for a node that ran.
    fn elapsed(&self) -> Option<f64> {
        match self {
            NodeOutcome::Ran { secs } | NodeOutcome::Failed { secs, .. } => Some(*secs),
            _ => None,
        }
    }
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
    /// (zero left) is spent, so its value is cleared one output at a time and, once every
    /// output of the node is spent, the whole slot is reclaimed
    /// ([`RuntimeCache::reclaim_slot`]) to trim peak RAM instead of holding it to end-of-run
    /// eviction. Reused across runs, reset each run.
    output_usage: Vec<OutputUsage>,
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
        idx.idx() >= self.outcomes.len() || self.outcomes[idx].ran()
    }

    /// Walk `plan.process_order` (producer-first). For each node: skip it as
    /// [`NodeOutcome::Cut`] if it's outside the `needed` mask (the resolver pruned its cone),
    /// else stamp its output digest and decide reuse ([`RuntimeCache::stamp_and_check_reuse`]),
    /// reuse from RAM/disk if unchanged, else invoke its lambda and persist
    /// the result to disk right away (so a long run's earlier caches survive a later
    /// failure or cancel). The `program`, `plan`, and `needed` mask are read-only. Returns
    /// per-run stats.
    #[allow(clippy::too_many_arguments)] // an orchestration entry point; each arg is a distinct collaborator
    pub(crate) async fn run(
        &mut self,
        program: &ExecutionProgram,
        plan: &ExecutionPlan,
        needed: &NodeColumn<bool>,
        cache: &mut RuntimeCache,
        flatten: &FlattenMap,
        progress: Option<&UnboundedSender<RunProgress>>,
        cancel: CancelToken,
    ) -> ExecutionStats {
        let start = Instant::now();
        let n_nodes = program.e_nodes.len();
        // Hold the cancel flag on the context so lambdas can poll it inside
        // off-thread work, and so the loop-top / post-loop checks below read
        // one source.
        self.ctx_manager.cancel = cancel;
        self.ctx_manager.logs.clear();

        // Detach the scratch buffers from `self` so the run loop can mutate the
        // context and these columns without aliasing `self`. The per-run result
        // columns start fresh at the current node count.
        let mut inputs = std::mem::take(&mut self.inputs);
        let mut outcomes = std::mem::take(&mut self.outcomes);
        outcomes.reset(n_nodes, NodeOutcome::Pending);

        // Seed the run's live per-output consumer counts from the plan (`0` ⇒ `Skip`, the
        // lambda-facing "nobody reads this output"; `> 0` ⇒ `Needed`). This same column is
        // counted down as consumers read, freeing each spent output (see `collect_inputs` and
        // the post-invoke release below). Borrowed in place (a distinct field from the
        // `ctx_manager` the loop also mutates), reusing its allocation across runs.
        let output_usage = &mut self.output_usage;
        output_usage.clear();
        output_usage.extend(plan.output_usage.iter().map(|&c| {
            if c == 0 {
                OutputUsage::Skip
            } else {
                OutputUsage::Needed(c)
            }
        }));

        // How far down `process_order` the run got. Stays the full length on a
        // normal run; on cancel it's the count reached before bailing, so the
        // stats report only the nodes that actually ran (not the unrun tail).
        let mut executed_count = plan.process_order.len();
        // The node (if any) that was mid-invoke when the run was cancelled. Its
        // result is untrustworthy (a cancellable lambda bails with Ok + partial
        // output), so it's dropped from the cache and the stats below.
        let mut cancelled_in_flight: Option<NodeIdx> = None;

        // The schedule is `process_order` (all reachable, producer-first). A
        // `MissingInputs` node can't run, so it's skipped here rather than pruned by a
        // separate pass; a runnable node whose *only* consumer is one of those may then
        // run needlessly (its output is unread — harmless, and missing inputs are an
        // error state anyway).
        for (pos, e_node_idx) in plan.process_order.iter().copied().enumerate() {
            // Coarse cancel: stop scheduling further nodes. A node already
            // mid-invoke isn't interrupted (it finishes), but nothing after
            // it starts. The unrun nodes simply don't appear in the stats.
            if self.ctx_manager.cancel.is_cancelled() {
                executed_count = pos;
                break;
            }
            if !needed[e_node_idx] {
                // Pruned by the pre-run cut: every consumer that would read this node reused
                // a cache, so its output is never read. Report it cached iff it still holds a
                // usable value (a deeper disk cache), else it's simply not computed this run.
                outcomes[e_node_idx] = NodeOutcome::Cut {
                    cached: cache.has_available_value(e_node_idx),
                };
                continue;
            }
            if program.e_nodes[e_node_idx].lambda.is_none()
                || !plan.verdicts[e_node_idx].wants_execute()
            {
                continue;
            }

            let func_id = program.e_nodes[e_node_idx].func_id;

            if has_errored_dependency(program, &outcomes, e_node_idx) {
                mark_skipped(cache, &mut outcomes, e_node_idx, func_id);
                continue;
            }

            // Resolve the node's digest and cache state — the one and only digest
            // computation (see [`prepare_node`]) — then act on the verdict. On `Run`,
            // `inputs` has been populated with the node's resolved arguments.
            match prepare_node(program, cache, output_usage, e_node_idx, &mut inputs) {
                Readiness::Reuse => {
                    outcomes[e_node_idx] = NodeOutcome::Reused;
                    continue;
                }
                Readiness::InputsUnavailable => {
                    mark_skipped(cache, &mut outcomes, e_node_idx, func_id);
                    continue;
                }
                Readiness::Run => {}
            }

            let output_count = program.e_nodes[e_node_idx].outputs.len as usize;
            let event_state = cache.slots[e_node_idx].event_state.clone();
            debug_assert!(matches!(outcomes[e_node_idx], NodeOutcome::Pending));

            // Attribute any logs this node emits to it (read by
            // `ContextManager::log`).
            let flat_id = program.e_nodes[e_node_idx].id;
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
            let usage = &output_usage[program.e_nodes[e_node_idx].outputs.range()];
            let result = {
                let lambda = &program.e_nodes[e_node_idx].lambda;
                let slot = cache.slots[e_node_idx].invoke_slot(output_count);
                lambda
                    .invoke(
                        &mut self.ctx_manager,
                        slot.state,
                        &event_state,
                        &inputs,
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
            if cancelled {
                cancelled_in_flight = Some(e_node_idx);
            }
            let slot = &mut cache.slots[e_node_idx];
            let succeeded = match result {
                // The fresh output now corresponds to this node's current digest; record
                // it so the next run's reuse check is a RAM hit.
                Ok(()) => {
                    slot.stamp_produced();
                    outcomes[e_node_idx] = NodeOutcome::Ran { secs: run_time };
                    true
                }
                Err(error) => {
                    slot.clear_output();
                    outcomes[e_node_idx] = NodeOutcome::Failed {
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
                // spent the instant it's stored — reclaim its non-RAM slot now rather than
                // holding it to end-of-run eviction. A node that still owes reads is reclaimed
                // later, in `collect_inputs`, when its last consumer lands.
                if !program.e_nodes[e_node_idx].cache.caches_in_ram()
                    && node_drained(program, output_usage, e_node_idx)
                {
                    cache.reclaim_slot(program, e_node_idx);
                }
            }
        }

        self.ctx_manager.current_node = None;
        let mut stats = collect_execution_stats(
            program,
            plan,
            &outcomes,
            start,
            executed_count,
            cancelled_in_flight,
        );
        stats.logs = std::mem::take(&mut self.ctx_manager.logs);
        stats.cancelled = self.ctx_manager.cancel.is_cancelled();
        self.inputs = inputs;
        self.outcomes = outcomes;
        stats
    }
}

/// A node's cache verdict once its digest is resolved, produced by [`prepare_node`].
enum Readiness {
    /// An unchanged output is cached (resident in RAM, or a blob on disk) — serve it
    /// without running the lambda.
    Reuse,
    /// A bound producer's disk blob failed to load (corrupt/deleted) — drop this node for
    /// the run like an errored dependency; the removed blob recomputes next reopen.
    InputsUnavailable,
    /// The node must run; `inputs` has been filled with its resolved arguments.
    Run,
}

/// Stamp node `idx`'s content digest and decide whether it reuses a cached output or must
/// run, via [`RuntimeCache::stamp_and_check_reuse`] — the same call the pre-run
/// [`resolve`](crate::execution::resolve) sweep makes, so the two can't diverge. Producer-first
/// order means every `Bind` producer's digest is already stamped when this runs; the sweep
/// already stamped the same digest, and this re-derives it idempotently (a cheap fold), so the
/// run loop needs no special case for surviving reuse-vs-run nodes.
///
/// The digest folds producer digests + consts *without values*, and input collection is
/// deferred until *after* the reuse check — so a node that turns out to reuse never loads its
/// producers' outputs (and its now-unread producers can have been cut). On [`Readiness::Run`],
/// `inputs` holds the resolved arguments.
fn prepare_node(
    program: &ExecutionProgram,
    cache: &mut RuntimeCache,
    output_usage: &mut [OutputUsage],
    idx: NodeIdx,
    inputs: &mut Vec<InvokeInput>,
) -> Readiness {
    // Stamp the digest and decide reuse through the one shared helper — the pre-run cut's
    // `resolve_structural` makes the identical call, so the two verdicts can't drift. A disk
    // hit is flagged but its bytes load lazily only when a running consumer reads them (so a
    // disk-cached value behind another never enters RAM); a `None` digest (impure) never reuses.
    if cache.stamp_and_check_reuse(program, idx) {
        return Readiness::Reuse;
    }

    // Not reusing: load the node's inputs, pulling any disk-cached producer in on demand (the
    // lazy frontier read) and releasing each producer whose last read this satisfies.
    if !collect_inputs(program, cache, output_usage, idx, inputs) {
        return Readiness::InputsUnavailable;
    }
    Readiness::Run
}

/// Drop node `idx` from this run as skipped-for-upstream: clear any stale cached output so
/// it isn't served as this run's result, and record the outcome. Shared by the
/// errored-dependency and failed-input-load paths, which are indistinguishable downstream.
fn mark_skipped(
    cache: &mut RuntimeCache,
    outcomes: &mut NodeColumn<NodeOutcome>,
    idx: NodeIdx,
    func_id: FuncId,
) {
    cache.slots[idx].clear_output();
    outcomes[idx] = NodeOutcome::Skipped {
        error: RunError::SkippedUpstream { func_id },
    };
}

fn has_errored_dependency(
    program: &ExecutionProgram,
    outcomes: &NodeColumn<NodeOutcome>,
    e_node_idx: NodeIdx,
) -> bool {
    let span = program.e_nodes[e_node_idx].inputs;
    program.inputs[span.range()].iter().any(|input| {
        matches!(&input.binding, ExecutionBinding::Bind(addr) if outcomes[addr.target_idx].error().is_some())
    })
}

/// Resolve `e_node_idx`'s inputs into `inputs`. Returns `false` if a bound producer's
/// value can't be materialized — its disk blob was reused-from-disk but failed to load
/// (corrupt/deleted): [`hydrate_slot`](RuntimeCache::hydrate_slot) then removes the bad
/// blob, so this run drops the consumer (like an errored dependency) and the next reopen
/// recomputes the producer fresh. Any other missing value is a planner bug.
fn collect_inputs(
    program: &ExecutionProgram,
    cache: &mut RuntimeCache,
    output_usage: &mut [OutputUsage],
    e_node_idx: NodeIdx,
    inputs: &mut Vec<InvokeInput>,
) -> bool {
    inputs.clear();
    let span = program.e_nodes[e_node_idx].inputs;
    for pool_idx in span.range() {
        let value = match &program.inputs[pool_idx].binding {
            ExecutionBinding::None => DynamicValue::Unbound,
            ExecutionBinding::Const(v) => v.into(),
            ExecutionBinding::Bind(addr) => {
                let (target, port) = (addr.target_idx, addr.port_idx);
                // The producer settled earlier this run: it either ran (resident) or was
                // reused-from-disk (marked `OnDisk`, not yet loaded) — hydrate the latter
                // now, the lazy frontier read. A resident value is a no-op load; a failed
                // one leaves the slot empty and drops this consumer for the run.
                cache.hydrate_slot(program, target);
                let value = {
                    let Some(outputs) = cache.slots[target].output_values() else {
                        return false;
                    };
                    assert_eq!(outputs.len(), program.e_nodes[target].outputs.len as usize);
                    outputs[port].clone()
                };
                // Count this read against the producer's output. Each `Bind` edge was counted
                // once in the plan's usage, so when this output's count reaches `Skip` every
                // in-run consumer has read it and it's spent — freed one output at a time here.
                let out_idx = program.e_nodes[target].outputs.start as usize + port;
                match output_usage[out_idx] {
                    OutputUsage::Needed(1) => {
                        output_usage[out_idx] = OutputUsage::Skip;
                        release_spent_output(program, cache, output_usage, target, port);
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
    true
}

/// The last consumer of `target`'s output `port` just took its copy. A `Ram`/`Both` node
/// stays hot for the next run; for a non-RAM one, that spent output is freed now rather than
/// at end-of-run eviction — the whole slot reclaimed
/// ([`RuntimeCache::reclaim_slot`], demoted to disk or dropped) once *every* output is spent,
/// else just this one value cleared while sibling outputs are still owed to other consumers.
fn release_spent_output(
    program: &ExecutionProgram,
    cache: &mut RuntimeCache,
    output_usage: &[OutputUsage],
    target: NodeIdx,
    port: usize,
) {
    if program.e_nodes[target].cache.caches_in_ram() {
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
    executed_count: usize,
    cancelled_in_flight: Option<NodeIdx>,
) -> ExecutionStats {
    let mut executed_nodes = Vec::with_capacity(executed_count);
    let mut missing_inputs = Vec::new();
    let mut cached_nodes = Vec::new();
    let mut node_errors = Vec::new();

    // The schedule (and its per-node outcomes) is `process_order`; `executed_count` bounds
    // it to what ran before a cancel. Each node's outcome is the sole source of truth.
    for &idx in &plan.process_order[..executed_count] {
        // The node interrupted mid-invoke by a cancel didn't complete — omit it from the
        // executed set so the consumer doesn't paint it as executed (it stays in errors).
        let e = &program.e_nodes[idx];
        match &outcomes[idx] {
            // A reuse hit, or a node the cut pruned that still holds a value, are both
            // "available, not recomputed" — reported cached. A pruned memory-only node
            // (`Cut { cached: false }`) has no value this run and falls through, uncounted.
            NodeOutcome::Reused | NodeOutcome::Cut { cached: true } => cached_nodes.push(e.id),
            outcome if outcome.ran() && Some(idx) != cancelled_in_flight => {
                executed_nodes.push(ExecutedNodeStats {
                    node_id: e.id,
                    elapsed_secs: outcome.elapsed().unwrap_or(0.0),
                });
            }
            _ => {}
        }
    }

    for &idx in &plan.process_order {
        let e = &program.e_nodes[idx];
        if plan.verdicts[idx].missing_required_inputs() {
            // Recompute which ports are unsatisfied (shares `input_missing` with the
            // planner) — only for the rare missing node, so it isn't worth a stored column.
            for (i, pool_idx) in e.inputs.range().enumerate() {
                if input_missing(&program.inputs[pool_idx], &plan.verdicts) {
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
    }
}

#[cfg(test)]
mod tests;
