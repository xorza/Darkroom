//! The run loop and its transient state. The `Executor` owns the shared
//! `ctx_manager` and the invoke scratch; the per-node cross-run cache lives in
//! the [`Cache`](crate::execution::cache::Cache). Given an immutable
//! [`ExecutionProgram`](crate::execution::program::ExecutionProgram), a prepared
//! [`ExecutionPlan`](crate::execution::plan::ExecutionPlan), and that `Cache`,
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

use crate::context::ContextManager;
use crate::data::DynamicValue;
use crate::execution_stats::{
    ExecutedNodeStats, ExecutionStats, FlattenMap, NodeError, RunPhase, RunProgress,
};
use crate::func_lambda::{InvokeError, InvokeInput, OutputUsage};
use crate::graph::InputPort;
use crate::prelude::FuncId;

use crate::execution::cache::Cache;
use crate::execution::digest::node_digest;
use crate::execution::output_cache::OutputCache;
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
    /// Per-*invoke* scratch, refilled for each node that runs: its resolved inputs and
    /// its ports' [`OutputUsage`] flags (sliced from the plan's `output_usage` counts —
    /// a different, per-output-pool thing, hence the `_scratch` suffix here).
    inputs: Vec<InvokeInput>,
    output_usage_scratch: Vec<OutputUsage>,
    /// Per-run outcome per node (see [`NodeOutcome`]), indexed by `e_node_idx`. Reused
    /// across runs, reset to node count each run. The one per-node result column.
    outcomes: NodeColumn<NodeOutcome>,
}

impl Executor {
    /// Whether node `idx` actually recomputed its lambda in the last run — i.e. wasn't
    /// reused from RAM/disk. Before any run (empty column) every node reads as "ran", so
    /// plan-only introspection still sees the full schedule. Test/stats introspection.
    pub(crate) fn ran(&self, idx: NodeIdx) -> bool {
        idx.idx() >= self.outcomes.len() || self.outcomes[idx].ran()
    }

    /// The nodes whose resident value the last run must keep, as a per-node mask:
    /// everything the run recomputed, plus the producers those nodes read as frontier
    /// inputs. A *reused* node (served from cache, its lambda skipped) isn't kept on its
    /// own account — only if a node that ran read its value — so a disk-cached value
    /// behind another reused node is reclaimed. Everything else resident is an untouched
    /// prior-run leftover that [`OutputCache::evict_unused`] may demote to disk. Reads the
    /// run's `outcomes`, so it's the executor's to compute, not the cache's.
    pub(crate) fn protected_after_run(
        &self,
        program: &ExecutionProgram,
        plan: &ExecutionPlan,
    ) -> Vec<bool> {
        let mut protected = vec![false; program.e_nodes.len()];
        for &e_idx in &plan.process_order {
            if !self.ran(e_idx) {
                continue;
            }
            protected[e_idx.idx()] = true;
            let span = program.e_nodes[e_idx].inputs;
            for input in &program.inputs[span.range()] {
                if let ExecutionBinding::Bind(addr) = &input.binding {
                    protected[addr.target_idx.idx()] = true;
                }
            }
        }
        protected
    }

    /// Walk `plan.process_order` (producer-first). For each node: skip it as
    /// [`NodeOutcome::Cut`] if it's outside the `needed` mask (the resolver pruned its cone),
    /// else compute its output digest ([`digest::node_digest`], stamped as its
    /// `current_digest`), reuse from RAM/disk if unchanged, else invoke its lambda and persist
    /// the result to `output_cache` right away (so a long run's earlier caches survive a later
    /// failure or cancel). The `program`, `plan`, and `needed` mask are read-only. Returns
    /// per-run stats.
    #[allow(clippy::too_many_arguments)] // an orchestration entry point; each arg is a distinct collaborator
    pub(crate) async fn run(
        &mut self,
        program: &ExecutionProgram,
        plan: &ExecutionPlan,
        needed: &NodeColumn<bool>,
        cache: &mut Cache,
        output_cache: &OutputCache,
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
        let mut output_usage_scratch = std::mem::take(&mut self.output_usage_scratch);
        let mut outcomes = std::mem::take(&mut self.outcomes);
        outcomes.reset(n_nodes, NodeOutcome::Pending);

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
            match prepare_node(program, output_cache, cache, e_node_idx, &mut inputs) {
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

            collect_output_usage(program, plan, e_node_idx, &mut output_usage_scratch);

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
            let result = {
                let lambda = &program.e_nodes[e_node_idx].lambda;
                let slot = cache.slots[e_node_idx].invoke_slot(output_count);
                lambda
                    .invoke(
                        &mut self.ctx_manager,
                        slot.state,
                        &event_state,
                        &inputs,
                        &output_usage_scratch,
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
                output_cache
                    .store_node(program, e_node_idx, cache, &mut self.ctx_manager)
                    .await;
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
        self.output_usage_scratch = output_usage_scratch;
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

/// Compute node `idx`'s content digest, stamp it as the slot's `current_digest`, and decide
/// whether the node reuses a cached output or must run. Producer-first order means every
/// `Bind` producer's digest is already stamped when this runs. The pre-run
/// [`resolve`](crate::execution::resolve) sweep already stamped the same digest; this
/// re-derives it idempotently (a cheap fold), so the run loop needs no special case for
/// surviving reuse-vs-run nodes.
///
/// The digest folds producer digests + consts *without values*, and input collection is
/// deferred until *after* the reuse check — so a node that turns out to reuse never loads its
/// producers' outputs (and its now-unread producers can have been cut). On [`Readiness::Run`],
/// `inputs` holds the resolved arguments.
fn prepare_node(
    program: &ExecutionProgram,
    output_cache: &OutputCache,
    cache: &mut Cache,
    idx: NodeIdx,
    inputs: &mut Vec<InvokeInput>,
) -> Readiness {
    let digest = node_digest(program, idx, cache);
    cache.slots[idx].current_digest = digest;

    // A disk blob is loaded lazily only when a running consumer reads it (so a disk-cached
    // value behind another never enters RAM). A `None` digest (impure) never reuses.
    if digest.is_some()
        && (cache.is_resident_hit(idx) || output_cache.mark_on_disk_if_present(program, idx, cache))
    {
        return Readiness::Reuse;
    }

    // Not reusing: load the node's inputs, pulling any disk-cached producer in on demand (the
    // lazy frontier read).
    if !collect_inputs(program, output_cache, cache, idx, inputs) {
        return Readiness::InputsUnavailable;
    }
    Readiness::Run
}

/// Drop node `idx` from this run as skipped-for-upstream: clear any stale cached output so
/// it isn't served as this run's result, and record the outcome. Shared by the
/// errored-dependency and failed-input-load paths, which are indistinguishable downstream.
fn mark_skipped(
    cache: &mut Cache,
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
/// (corrupt/deleted): [`hydrate_slot`](OutputCache::hydrate_slot) then removes the bad
/// blob, so this run drops the consumer (like an errored dependency) and the next reopen
/// recomputes the producer fresh. Any other missing value is a planner bug.
fn collect_inputs(
    program: &ExecutionProgram,
    output_cache: &OutputCache,
    cache: &mut Cache,
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
                output_cache.hydrate_slot(program, cache, target);
                let Some(outputs) = cache.slots[target].output_values() else {
                    return false;
                };
                assert_eq!(outputs.len(), program.e_nodes[target].outputs.len as usize);
                outputs[port].clone()
            }
        };
        inputs.push(InvokeInput { value });
    }
    true
}

fn collect_output_usage(
    program: &ExecutionProgram,
    plan: &ExecutionPlan,
    e_node_idx: NodeIdx,
    usage: &mut Vec<OutputUsage>,
) {
    usage.clear();
    let span = program.e_nodes[e_node_idx].outputs;
    usage.extend(plan.output_usage[span.range()].iter().map(|&c| {
        if c == 0 {
            OutputUsage::Skip
        } else {
            OutputUsage::Needed(c)
        }
    }));
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
