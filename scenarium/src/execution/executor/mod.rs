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

use std::time::Instant;

use tokio::sync::mpsc::UnboundedSender;

use common::CancelToken;

use crate::execution::identity::FlattenMap;
use crate::execution::report::{PinnedOutput, PinnedOutputs, RunEvent, RunPhase, RunProgress};
use crate::execution::stats::{ExecutedNodeStats, ExecutionStats, NodeError};
use crate::graph::InputPort;
use crate::node::lambda::{InvokeError, InvokeInput};
use crate::runtime::context::ContextManager;
use crate::{DynamicValue, RamUsage};

use crate::execution::cache::RuntimeCache;
use crate::execution::plan::{ExecutionPlan, input_missing};
use crate::execution::program::{ExecutionBinding, ExecutionProgram, NodeIdx, OutputIdx};
use crate::execution::resolve::Disposition;
use crate::execution::{NodeColumn, OutputColumn, RunError};

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

/// Why every `events.send(..)` in this module is `.expect`-asserted rather than
/// silently ignored: `send` only fails once every receiver is dropped, and the
/// worker's `event_rx` (see `worker::worker_loop`) isn't dropped until *after*
/// the `execute` future this `run` lives inside resolves — `send` isn't an
/// await point, so an abort mid-run can only land at an earlier `.await` and
/// drop this whole future before a send is ever reached, never selectively
/// close just the receiver. A failed send here means that lifetime invariant
/// broke — a real bug, not an expected failure to shrug off.
const EVENTS_OUTLIVE_RUN: &str =
    "the events receiver outlives this future — worker_loop only drops it after `execute` resolves";

#[derive(Default, Debug)]
struct RemainingOutputReads {
    counts: OutputColumn<u32>,
}

impl RemainingOutputReads {
    fn seed(&mut self, plan: &ExecutionPlan) {
        self.counts.clone_from(&plan.outputs.readers);
    }

    fn is_last(&self, output_idx: OutputIdx) -> bool {
        self.counts[output_idx] == 1
    }

    fn consume(&mut self, output_idx: OutputIdx) -> bool {
        let remaining = &mut self.counts[output_idx];
        assert!(
            *remaining > 0,
            "read an output more often than the plan counted"
        );
        *remaining -= 1;
        *remaining == 0
    }

    fn node_drained(&self, program: &ExecutionProgram, node_idx: NodeIdx) -> bool {
        self.counts
            .slice(program.e_nodes[node_idx].outputs)
            .iter()
            .all(|remaining| *remaining == 0)
    }
}

#[derive(Debug)]
struct ExecutionFrame<'a> {
    program: &'a ExecutionProgram,
    plan: &'a ExecutionPlan,
    cache: &'a mut RuntimeCache,
    flatten: &'a FlattenMap,
    remaining_reads: &'a mut RemainingOutputReads,
    retain: &'a NodeColumn<bool>,
    inputs: &'a mut Vec<InvokeInput>,
}

impl ExecutionFrame<'_> {
    async fn hydrate_resource_producers(&mut self, node_idx: NodeIdx) {
        for input in self.program.node_inputs(&self.program.e_nodes[node_idx]) {
            if input.stamper.is_some()
                && let ExecutionBinding::Bind(addr) = &input.binding
            {
                self.cache.hydrate_slot(self.program, addr.target_idx).await;
            }
        }
    }

    fn collect_pinned_values(&mut self, node_idx: NodeIdx) -> Option<PinnedOutputs> {
        let e_node = &self.program.e_nodes[node_idx];
        let pinned_root = self.plan.pinned.contains(&node_idx);
        let mut values = Vec::new();
        for port_idx in 0..e_node.outputs.len as usize {
            let output_idx = self.program.output_idx(node_idx, port_idx);
            if pinned_root || self.program.is_output_pinned(output_idx) {
                let value = self
                    .cache
                    .read_output_port(self.program, node_idx, port_idx, false)
                    .expect("a node's output is resident immediately after it succeeds");
                values.push(PinnedOutput { port_idx, value });
            }
        }
        if values.is_empty() {
            return None;
        }
        let node = self
            .flatten
            .address(e_node.id)
            .expect("a node that just ran must have an authoring address")
            .clone();
        Some(PinnedOutputs { node, values })
    }

    async fn collect_inputs(&mut self, node_idx: NodeIdx) -> Result<(), RunError> {
        self.inputs.clear();
        let node_inputs = self.program.node_inputs(&self.program.e_nodes[node_idx]);
        for (input_idx, e_input) in node_inputs.iter().enumerate() {
            let value = match &e_input.binding {
                ExecutionBinding::None => DynamicValue::Unbound,
                ExecutionBinding::Const(value) => value.into(),
                ExecutionBinding::Bind(addr) => {
                    let target = addr.target_idx;
                    let port_idx = addr.port_idx;
                    self.cache.hydrate_slot(self.program, target).await;
                    let output_idx = self.program.output_idx(target, port_idx);
                    let take = self.remaining_reads.is_last(output_idx) && !self.retain[target];
                    let Some(value) =
                        self.cache
                            .read_output_port(self.program, target, port_idx, take)
                    else {
                        return Err(RunError::InputLoadFailed {
                            func_id: self.program.e_nodes[node_idx].func_id,
                            input: input_idx,
                        });
                    };
                    if self.remaining_reads.consume(output_idx) && !self.retain[target] {
                        if self.remaining_reads.node_drained(self.program, target) {
                            self.cache.reclaim_slot(self.program, target);
                        } else {
                            self.cache.clear_output_port(target, port_idx);
                        }
                    }
                    value
                }
            };
            self.inputs.push(InvokeInput { value });
        }
        Ok(())
    }
}

#[derive(Default, Debug)]
pub(crate) struct Executor {
    pub(crate) ctx_manager: ContextManager,
    /// Per-*invoke* scratch: the node's resolved inputs, refilled for each node that runs.
    inputs: Vec<InvokeInput>,
    /// The run's mutable copy of the plan's structural binding counts. Only real bound
    /// input reads decrement it; production demand and host pins remain immutable plan data.
    remaining_reads: RemainingOutputReads,
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
        events: Option<&UnboundedSender<RunEvent>>,
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

        self.remaining_reads.seed(plan);

        {
            let mut frame = ExecutionFrame {
                program,
                plan,
                cache,
                flatten,
                remaining_reads: &mut self.remaining_reads,
                retain: &self.retain,
                inputs: &mut self.inputs,
            };

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
                        cached: frame.cache.has_available_value(e_node_idx),
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
                        frame.cache,
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
                    Disposition::Run if frame.cache.slots[e_node_idx].current_digest.is_none() => {
                        frame.hydrate_resource_producers(e_node_idx).await;
                        let demand = plan.outputs.demand.slice(e_node.outputs);
                        frame
                            .cache
                            .stamp_and_check_reuse(program, e_node_idx, demand)
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
                        frame.cache,
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
                if let Err(error) = frame.collect_inputs(e_node_idx).await {
                    mark_skipped(frame.cache, &mut self.outcomes, e_node_idx, error);
                    continue;
                }

                let output_count = e_node.outputs.len as usize;
                let event_state = frame.cache.slots[e_node_idx].event_state.clone();
                assert!(matches!(self.outcomes[e_node_idx], NodeOutcome::Pending));

                // Attribute any logs this node emits to it (read by
                // `ContextManager::log`).
                let flat_id = e_node.id;
                self.ctx_manager.current_node = Some(flat_id);
                let invoke_start = Instant::now();
                if let Some(events) = events {
                    events
                        .send(RunEvent::Progress(RunProgress {
                            node_id: flat_id,
                            phase: RunPhase::Started { at: invoke_start },
                        }))
                        .expect(EVENTS_OUTLIVE_RUN);
                }
                let demand = plan.outputs.demand.slice(e_node.outputs);
                let result = {
                    let slot = frame.cache.slots[e_node_idx].invoke_slot(output_count);
                    e_node
                        .lambda
                        .invoke(
                            &mut self.ctx_manager,
                            slot.state,
                            &event_state,
                            frame.inputs,
                            demand,
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
                    Ok(()) => {
                        let outputs =
                            frame.cache.slots[e_node_idx].unbound_demanded_outputs(demand);
                        if outputs.is_empty() {
                            Ok(())
                        } else {
                            Err(RunError::OutputsNotProduced { func_id, outputs })
                        }
                    }
                    other => other,
                };
                let cancelled = matches!(&result, Err(RunError::Cancelled { .. }));
                let slot = &mut frame.cache.slots[e_node_idx];
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
                if !cancelled && let Some(events) = events {
                    events
                        .send(RunEvent::Progress(RunProgress {
                            node_id: flat_id,
                            phase: RunPhase::Finished {
                                elapsed_secs: run_time,
                            },
                        }))
                        .expect(EVENTS_OUTLIVE_RUN);
                }
                // Push immediately after a fresh success, before any later consumer can release
                // the values. Host delivery does not participate in binding-reader accounting.
                if succeeded
                    && let Some(events) = events
                    && let Some(payload) = frame.collect_pinned_values(e_node_idx)
                {
                    events
                        .send(RunEvent::PinnedOutputs(payload))
                        .expect(EVENTS_OUTLIVE_RUN);
                }
                // Persist this node's cache the moment it finishes (durable as the run
                // progresses), not at the end of the whole run. The snapshot is taken
                // synchronously inside `store_node`; only the write awaits, so the cache
                // borrow doesn't cross it.
                if succeeded {
                    frame
                        .cache
                        .store_node(program, e_node_idx, &mut self.ctx_manager)
                        .await;
                    // A node no consumer reads (a sink, or every output already `Skip`) is
                    // spent the instant it's stored — reclaim its non-retained slot now rather
                    // than holding it to end-of-run eviction. A node that still owes reads is
                    // reclaimed later, in `collect_inputs`, when its last consumer lands.
                    if !frame.retain[e_node_idx]
                        && frame.remaining_reads.node_drained(program, e_node_idx)
                    {
                        frame.cache.reclaim_slot(program, e_node_idx);
                    }
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
