//! The run loop and its transient state. The `Executor` owns the shared
//! `ctx_manager` and the invoke scratch; the per-node cross-run cache lives in
//! the [`RuntimeCache`](crate::execution::cache::RuntimeCache). Given an immutable
//! [`ExecutionProgram`](crate::execution::program::ExecutionProgram), a prepared
//! [`ExecutionPlan`](crate::execution::plan::ExecutionPlan), and that `RuntimeCache`,
//! [`Executor::run`] invokes each scheduled node's lambda and gathers stats.
//! Each node's per-run result is one [`NodeOutcome`] in the per-run outcome map.
//!
//! **Pre-run resolution.** [`run`](Executor::run) takes the
//! [`Resolver`](crate::execution::resolve::Resolver)'s
//! [`ResolvedRun`](crate::execution::resolve::ResolvedRun) — disposition, output demand,
//! and reader counts derived together and authoritative for the whole run. A
//! [`Disposition::Reuse`] is never re-derived after its producers may have been cut. A cut
//! node (its cone feeds only cache hits, so a disk-cached node's stale upstream isn't
//! recomputed on reopen) gets [`NodeOutcome::Cut`]. The one verdict the loop *improves* is a
//! `Run` whose stamped digest is `None` because a Bind-delivered resource value exists only
//! once its producers settle: the loop prepares that identity off-thread, re-stamps at
//! reach time, and serves the cache on a hit.

use std::time::Instant;

use tokio::sync::mpsc::UnboundedSender;

use common::CancelToken;

use crate::execution::identity::{ExecutionInputPort, ExecutionNodeId};
use crate::execution::report::{PinnedOutput, PinnedOutputs, RunEvent, RunPhase, RunProgress};
use crate::execution::stats::{ExecutedNodeStats, ExecutionStats, NodeError};
use crate::node::lambda::{InvokeError, InvokeInput};
use crate::runtime::context::ContextManager;
use crate::{DynamicValue, RamUsage};

use crate::execution::cache::RuntimeCache;
use crate::execution::plan::{ExecutionPlan, input_missing};
use crate::execution::program::{ExecutionBinding, ExecutionProgram, OutputIdx};
use crate::execution::resolve::{Disposition, ResolvedRun};
use crate::execution::resource::RunResourceStamps;
use crate::execution::{NodeMap, OutputColumn, RunError, reset_node_map};

/// What became of a node this run — the single per-node result map, so the run-time
/// facts can't contradict (a node can't be `Reused` yet carry a run time, or `Ran` yet
/// also flagged errored). Carries its own `RunError`/elapsed, so nothing lives in a side
/// map.
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
    /// Never ran — an upstream dependency errored or its func has no implementation attached.
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
    fn seed(&mut self, resolved: &ResolvedRun) {
        self.counts.clone_from(&resolved.outputs.readers);
    }

    fn is_last(&self, output_idx: OutputIdx) -> bool {
        self.counts[output_idx] == 1
    }

    fn consume(&mut self, output_idx: OutputIdx) -> bool {
        let remaining = &mut self.counts[output_idx];
        debug_assert!(
            *remaining > 0,
            "read an output more often than the resolved run counted"
        );
        *remaining = remaining.wrapping_sub(1);
        *remaining == 0
    }

    fn node_drained(&self, program: &ExecutionProgram, e_node_id: ExecutionNodeId) -> bool {
        self.counts
            .slice(program.e_nodes[&e_node_id].outputs)
            .iter()
            .all(|remaining| *remaining == 0)
    }
}

#[derive(Debug)]
struct ExecutionFrame<'a> {
    program: &'a ExecutionProgram,
    plan: &'a ExecutionPlan,
    cache: &'a mut RuntimeCache,
    resource_stamps: &'a mut RunResourceStamps,
    remaining_reads: &'a mut RemainingOutputReads,
    inputs: &'a mut Vec<InvokeInput>,
}

impl ExecutionFrame<'_> {
    fn emit_pinned_values(
        &mut self,
        e_node_id: ExecutionNodeId,
        events: Option<&UnboundedSender<RunEvent>>,
    ) {
        let Some(events) = events else { return };
        let output_count = self.program.e_nodes[&e_node_id].outputs.len as usize;
        let pinned_root = self.plan.pinned.contains(&e_node_id);
        let pinned_ports: Vec<_> = (0..output_count)
            .filter(|&port_idx| {
                pinned_root
                    || self
                        .program
                        .is_output_pinned(self.program.output_idx(e_node_id, port_idx))
            })
            .collect();
        if pinned_ports.is_empty() {
            return;
        }

        let values = pinned_ports
            .into_iter()
            .map(|port_idx| {
                let value = self
                    .cache
                    .read_output_port(self.program, e_node_id, port_idx, false)
                    .expect("a node's pinned output must be resident when delivered");
                PinnedOutput { port_idx, value }
            })
            .collect();
        events
            .send(RunEvent::PinnedOutputs(PinnedOutputs { e_node_id, values }))
            .expect(EVENTS_OUTLIVE_RUN);
    }

    fn collect_inputs(&mut self, e_node_id: ExecutionNodeId) {
        self.inputs.clear();
        let node_inputs = self.program.node_inputs(&self.program.e_nodes[&e_node_id]);
        for e_input in node_inputs {
            let value = match &e_input.binding {
                ExecutionBinding::None => DynamicValue::Unbound,
                ExecutionBinding::Const(value) => value.into(),
                ExecutionBinding::Bind(addr) => {
                    let target = addr.e_node_id;
                    let port_idx = addr.port_idx;
                    let output_idx = self.program.output_idx(target, port_idx);
                    let take = self.remaining_reads.is_last(output_idx)
                        && !self.program.e_nodes[&target].cache.caches_in_ram();
                    let value = self
                        .cache
                        .read_output_port(self.program, target, port_idx, take)
                        .expect("a resolved producer output must be resident when consumed");
                    self.finish_read(target, port_idx, output_idx);
                    value
                }
            };
            self.inputs.push(InvokeInput { value });
        }
    }

    fn cancel_input_reads(&mut self, e_node_id: ExecutionNodeId) {
        for input in self.program.node_inputs(&self.program.e_nodes[&e_node_id]) {
            if let ExecutionBinding::Bind(address) = &input.binding {
                let output_idx = self.program.output_idx(address.e_node_id, address.port_idx);
                self.finish_read(address.e_node_id, address.port_idx, output_idx);
            }
        }
    }

    fn finish_read(&mut self, target: ExecutionNodeId, port_idx: usize, output_idx: OutputIdx) {
        if !self.remaining_reads.consume(output_idx)
            || self.program.e_nodes[&target].cache.caches_in_ram()
            || self.cache.slots[&target].output_values().is_none()
        {
            return;
        }
        if self.remaining_reads.node_drained(self.program, target) {
            self.cache.reclaim_slot(self.program, target);
        } else {
            self.cache.clear_output_port(target, port_idx);
        }
    }
}

#[derive(Default, Debug)]
pub(crate) struct Executor {
    pub(crate) ctx_manager: ContextManager,
    /// Per-*invoke* scratch: the node's resolved inputs, refilled for each node that runs.
    inputs: Vec<InvokeInput>,
    /// The run's mutable copy of the resolver's live binding counts. Only real bound
    /// input reads decrement it; production demand and host pins remain immutable.
    remaining_reads: RemainingOutputReads,
    /// Per-run outcome per node (see [`NodeOutcome`]), keyed by node id. Reused
    /// across runs and rebuilt each run.
    outcomes: NodeMap<NodeOutcome>,
}

impl Executor {
    /// Whether `e_node_id` actually recomputed its lambda in the last run — i.e. wasn't
    /// reused from RAM/disk. Before any run (empty map) every node reads as "ran", so
    /// plan-only introspection still sees the full schedule. Test introspection only.
    #[cfg(test)]
    pub(crate) fn ran(&self, e_node_id: ExecutionNodeId) -> bool {
        self.outcomes.get(&e_node_id).is_none_or(|outcome| {
            matches!(
                outcome,
                NodeOutcome::Ran { .. } | NodeOutcome::Failed { .. }
            )
        })
    }

    /// Walk `plan.process_order` (producer-first). For each node: skip it as
    /// [`NodeOutcome::Cut`] if the resolver pruned its cone, serve it from RAM/disk on
    /// [`Disposition::Reuse`], else invoke its lambda and persist the result to disk right
    /// away (so a long run's earlier caches survive a later failure or cancel). The
    /// `program`, `plan`, and `resolved` run are read-only. Returns per-run stats.
    #[allow(clippy::too_many_arguments)] // Each argument is a distinct run collaborator.
    pub(crate) async fn run(
        &mut self,
        program: &ExecutionProgram,
        plan: &ExecutionPlan,
        resolved: &ResolvedRun,
        cache: &mut RuntimeCache,
        resource_stamps: &mut RunResourceStamps,
        events: Option<&UnboundedSender<RunEvent>>,
        cancel: CancelToken,
    ) -> ExecutionStats {
        let start = Instant::now();
        // Hold the cancel flag on the context so lambdas can poll it inside
        // off-thread work, and so the loop-top / post-loop checks below read
        // one source.
        self.ctx_manager.cancel = cancel;
        self.ctx_manager.logs.clear();
        reset_node_map(
            &mut self.outcomes,
            program.e_nodes.keys().copied(),
            NodeOutcome::Pending,
        );

        self.remaining_reads.seed(resolved);

        {
            let mut frame = ExecutionFrame {
                program,
                plan,
                cache,
                resource_stamps,
                remaining_reads: &mut self.remaining_reads,
                inputs: &mut self.inputs,
            };

            // The producer-first schedule excludes unseeded disabled nodes; the
            // resolved run cuts cache-hidden and blocked cones.
            for &e_node_id in &plan.process_order {
                // Coarse cancel: stop scheduling further nodes. A node already
                // mid-invoke isn't interrupted (it finishes), but nothing after
                // it starts. The unreached tail stays `Pending`, which the stats
                // ignore.
                if self.ctx_manager.cancel.is_cancelled() {
                    break;
                }
                let e_node = &program.e_nodes[&e_node_id];
                if !plan.verdicts[&e_node_id].wants_execute() {
                    continue;
                }
                if resolved.disposition[&e_node_id] == Disposition::Cut {
                    // Pruned by the pre-run cut: every consumer that would read this node reused
                    // a cache, so its output is never read. Report it cached iff it still holds a
                    // usable value (a deeper disk cache), else it's simply not computed this run.
                    *self.outcomes.get_mut(&e_node_id).unwrap() = NodeOutcome::Cut {
                        cached: frame.cache.has_available_value(e_node_id),
                    };
                    continue;
                }
                // A func registered without an implementation can't execute — a host/library
                // configuration error, reported on the node every run (before the reuse check,
                // so it can't flicker with cache state); its consumers skip as errored-upstream.
                if e_node.lambda.is_none() {
                    mark_skipped(
                        frame.cache,
                        &mut self.outcomes,
                        e_node_id,
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
                let reused = match resolved.disposition[&e_node_id] {
                    Disposition::Reuse => true,
                    Disposition::Run if frame.cache.slots[&e_node_id].current_digest.is_none() => {
                        frame
                            .resource_stamps
                            .prepare_node(
                                program,
                                frame.cache,
                                e_node_id,
                                self.ctx_manager.cancel.clone(),
                            )
                            .await;
                        frame
                            .cache
                            .stamp_digest(program, frame.resource_stamps, e_node_id);
                        let demand = resolved.outputs.demand.slice(e_node.outputs);
                        let reused = frame.cache.check_reuse(program, e_node_id, demand).await;
                        if reused {
                            frame.cancel_input_reads(e_node_id);
                        }
                        reused
                    }
                    _ => false,
                };
                if reused {
                    *self.outcomes.get_mut(&e_node_id).unwrap() = NodeOutcome::Reused;
                    frame.emit_pinned_values(e_node_id, events);
                    continue;
                }

                let func_id = e_node.func_id;

                if has_errored_dependency(program, &self.outcomes, e_node_id) {
                    mark_skipped(
                        frame.cache,
                        &mut self.outcomes,
                        e_node_id,
                        RunError::SkippedUpstream { func_id },
                    );
                    continue;
                }

                // Read already-resolved inputs and release each producer whose last read this
                // satisfies. Disk reuse is hydrated before the resolver cuts producer cones.
                frame.collect_inputs(e_node_id);

                let output_count = e_node.outputs.len as usize;
                let event_state = frame.cache.slots[&e_node_id].event_state.clone();
                debug_assert!(matches!(self.outcomes[&e_node_id], NodeOutcome::Pending));

                // Attribute any logs this node emits to it (read by
                // `ContextManager::log`).
                self.ctx_manager.current_node = Some(e_node_id);
                let invoke_start = Instant::now();
                if let Some(events) = events {
                    events
                        .send(RunEvent::Progress(RunProgress {
                            e_node_id,
                            phase: RunPhase::Started { at: invoke_start },
                        }))
                        .expect(EVENTS_OUTLIVE_RUN);
                }
                let demand = resolved.outputs.demand.slice(e_node.outputs);
                let result = {
                    let slot = frame
                        .cache
                        .slots
                        .get_mut(&e_node_id)
                        .unwrap()
                        .invoke_slot(output_count);
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
                            frame.cache.slots[&e_node_id].unbound_demanded_outputs(demand);
                        if outputs.is_empty() {
                            Ok(())
                        } else {
                            Err(RunError::OutputsNotProduced { func_id, outputs })
                        }
                    }
                    other => other,
                };
                let cancelled = matches!(&result, Err(RunError::Cancelled { .. }));
                let slot = frame.cache.slots.get_mut(&e_node_id).unwrap();
                let succeeded = match result {
                    // The fresh output now corresponds to this node's current digest; record
                    // it so the next run's reuse check is a RAM hit.
                    Ok(()) => {
                        slot.stamp_produced();
                        *self.outcomes.get_mut(&e_node_id).unwrap() =
                            NodeOutcome::Ran { secs: run_time };
                        true
                    }
                    Err(error) => {
                        slot.clear_output();
                        *self.outcomes.get_mut(&e_node_id).unwrap() = NodeOutcome::Failed {
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
                            e_node_id,
                            phase: RunPhase::Finished {
                                elapsed_secs: run_time,
                            },
                        }))
                        .expect(EVENTS_OUTLIVE_RUN);
                }
                // Persist this node's cache the moment it finishes (durable as the run
                // progresses), not at the end of the whole run. The snapshot is taken
                // synchronously inside `store_node`; only the write awaits, so the cache
                // borrow doesn't cross it.
                if succeeded {
                    // Deliver before later consumers can release values; host delivery is not a reader.
                    frame.emit_pinned_values(e_node_id, events);
                    frame
                        .cache
                        .store_node(program, e_node_id, &mut self.ctx_manager)
                        .await;
                    // A node no consumer reads (a sink, or every output already `Skip`) is
                    // spent the instant it's stored — reclaim its non-retained slot now rather
                    // than holding it to end-of-run eviction. A node that still owes reads is
                    // reclaimed later, in `collect_inputs`, when its last consumer lands.
                    if !program.e_nodes[&e_node_id].cache.caches_in_ram()
                        && frame.remaining_reads.node_drained(program, e_node_id)
                    {
                        frame.cache.reclaim_slot(program, e_node_id);
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

/// Drop `e_node_id` from this run: clear any stale cached output so it isn't served as
/// this run's result, and record the outcome under the caller's reason —
/// [`RunError::SkippedUpstream`] for an errored dependency or
/// [`RunError::MissingLambda`] for a func with no implementation.
fn mark_skipped(
    cache: &mut RuntimeCache,
    outcomes: &mut NodeMap<NodeOutcome>,
    e_node_id: ExecutionNodeId,
    error: RunError,
) {
    cache.slots.get_mut(&e_node_id).unwrap().clear_output();
    *outcomes.get_mut(&e_node_id).unwrap() = NodeOutcome::Skipped { error };
}

fn has_errored_dependency(
    program: &ExecutionProgram,
    outcomes: &NodeMap<NodeOutcome>,
    e_node_id: ExecutionNodeId,
) -> bool {
    program.node_inputs(&program.e_nodes[&e_node_id]).iter().any(|input| {
        matches!(&input.binding, ExecutionBinding::Bind(addr) if outcomes[&addr.e_node_id].error().is_some())
    })
}

fn collect_execution_stats(
    program: &ExecutionProgram,
    plan: &ExecutionPlan,
    outcomes: &NodeMap<NodeOutcome>,
    start: Instant,
) -> ExecutionStats {
    let mut executed_nodes = Vec::new();
    let mut missing_inputs = Vec::new();
    let mut cached_nodes = Vec::new();
    let mut node_errors = Vec::new();

    // The schedule (and its per-node outcomes) is `process_order`. Each node's outcome is
    // the sole source of truth; a node the run never reached (a cancelled run's tail, or
    // skipped for missing inputs) is `Pending` and contributes to no list here.
    for &e_node_id in &plan.process_order {
        let e = &program.e_nodes[&e_node_id];
        match &outcomes[&e_node_id] {
            // A reuse hit, or a node the cut pruned that still holds a value, are both
            // "available, not recomputed" — reported cached. A pruned memory-only node
            // (`Cut { cached: false }`) has no value this run and falls through, uncounted.
            NodeOutcome::Reused | NodeOutcome::Cut { cached: true } => {
                cached_nodes.push(e_node_id);
            }
            NodeOutcome::Ran { secs } => executed_nodes.push(ExecutedNodeStats {
                e_node_id,
                elapsed_secs: *secs,
            }),
            // A cancelled invoke didn't complete — omit it from the executed set so the
            // consumer doesn't paint it as executed (its error still lands below). A
            // genuine failure did run; it appears in both lists.
            NodeOutcome::Failed { secs, error } if !matches!(error, RunError::Cancelled { .. }) => {
                executed_nodes.push(ExecutedNodeStats {
                    e_node_id,
                    elapsed_secs: *secs,
                });
            }
            _ => {}
        }
        if plan.verdicts[&e_node_id].missing_required_inputs() {
            // Recompute which ports are unsatisfied (shares `input_missing` with the
            // planner) — only for the rare missing node, so it isn't worth a stored column.
            for (i, input) in program.node_inputs(e).iter().enumerate() {
                if input_missing(input, &plan.verdicts) {
                    missing_inputs.push(ExecutionInputPort {
                        e_node_id,
                        port_idx: i,
                    });
                }
            }
        }
        if let Some(err) = outcomes[&e_node_id].error() {
            node_errors.push(NodeError {
                e_node_id,
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
