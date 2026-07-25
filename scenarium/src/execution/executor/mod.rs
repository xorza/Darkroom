//! The run loop and its transient state. The `Executor` owns the shared
//! `ctx_manager` and the invoke scratch; the per-node cross-run cache lives in
//! the [`RuntimeCache`](crate::execution::cache::runtime::RuntimeCache). Given an immutable
//! [`ExecutionProgram`](crate::execution::program::ExecutionProgram), a prepared
//! [`ExecutionPlan`](crate::execution::plan::ExecutionPlan), and that `RuntimeCache`,
//! [`Executor::run`] invokes each scheduled node's lambda and gathers outcomes.
//! Each node's per-run result is one [`NodeOutcome`] in the per-run outcome map.
//!
//! **Pre-run resolution.** [`run`](Executor::run) takes the
//! [`Resolver`](crate::execution::resolve::Resolver)'s
//! [`ResolvedRun`](crate::execution::resolve::ResolvedRun) — disposition, output demand,
//! and reader counts derived together and authoritative for the whole run. A
//! [`Disposition::Reuse`] is never re-derived after its producers may have been cut. A cut
//! node (its cone feeds only cache hits, so a disk-cached node's stale upstream isn't
//! recomputed on reopen) gets [`NodeOutcome::Cut`]. A missing implementation is reported
//! without probing its cache or retaining its input cone. The one verdict the loop *improves*
//! is a `Run` whose stamped digest is `None` because a Bind-delivered path value exists
//! only once its producers settle: the loop prepares that identity off-thread, re-stamps at
//! reach time, and serves the cache on a hit.

mod outcomes;
mod value_flow;

use std::time::Instant;

use tokio::sync::mpsc::UnboundedSender;
use tokio::task;

use common::CancelToken;

use crate::execution::event::EventTrigger;
use crate::execution::identity::ExecutionEventPort;
use crate::execution::outcome::ExecutionOutcome;
use crate::execution::program::index::NodeColumn;
use crate::execution::report::{RunEvent, RunPhase, RunProgress};
use crate::node::lambda::{InvokeError, InvokeInput};
use crate::runtime::context::ContextManager;

use crate::execution::cache::runtime::RuntimeCache;
use crate::execution::disk_store::StorePolicy;
use crate::execution::error::RunError;
use crate::execution::executor::outcomes::{
    NodeOutcome, collect_execution_outcome, has_errored_dependency, mark_skipped,
};
use crate::execution::executor::value_flow::{ExecutionFrame, RemainingOutputReads};
use crate::execution::plan::ExecutionPlan;
use crate::execution::program::ExecutionProgram;
use crate::execution::resolve::{Disposition, ResolvedRun};
use crate::execution::resource::RunResourceStamps;

/// Why every `events.send(..)` in this module is `.expect`-asserted rather than
/// silently ignored: `send` only fails once every receiver is dropped, and the
/// worker task's `event_rx` isn't dropped until *after*
/// the `execute` future this `run` lives inside resolves — `send` isn't an
/// await point, so an abort mid-run can only land at an earlier `.await` and
/// drop this whole future before a send is ever reached, never selectively
/// close just the receiver. A failed send here means that lifetime invariant
/// broke — a real bug, not an expected failure to shrug off.
const EVENTS_OUTLIVE_RUN: &str =
    "the events receiver outlives this future — the worker only drops it after `execute` resolves";

#[derive(Default, Debug)]
pub(crate) struct Executor {
    pub(crate) ctx_manager: ContextManager,
    /// Per-*invoke* scratch: the node's resolved inputs, refilled for each node that runs.
    inputs: Vec<InvokeInput>,
    /// The run's mutable copy of the resolver's live binding counts. Input consumption or
    /// retirement decrements it; production demand and host pins remain immutable.
    remaining_reads: RemainingOutputReads,
    /// Per-run outcome per node (see [`NodeOutcome`]), aligned to the program's
    /// dense node vector. Reused across runs and rebuilt each run.
    outcomes: NodeColumn<NodeOutcome>,
}

impl Executor {
    /// Walk `plan.process_order` (producer-first). For each node: skip it as
    /// [`NodeOutcome::Cut`] if the resolver pruned its cone, report a missing implementation,
    /// serve it from RAM/disk on [`Disposition::Reuse`], or invoke its lambda and persist the
    /// result to disk right away (so a long run's earlier caches survive a later failure or
    /// cancel). The `program`, `plan`, and `resolved` run are read-only.
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
        outcome: &mut ExecutionOutcome,
    ) {
        outcome.clear();
        let start = Instant::now();
        // Hold the cancel flag on the context so lambdas can poll it inside
        // off-thread work, and so the loop-top / post-loop checks below read
        // one source.
        self.ctx_manager.cancel = cancel;
        self.ctx_manager.logs.clear();
        self.outcomes
            .reset(program.e_nodes.len(), NodeOutcome::Pending);

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
            for (process_idx, &node_idx) in plan.process_order.iter().enumerate() {
                // Drain point for the live-report relay: sync-completing lambdas
                // give the worker's select no suspension point of their own, and
                // without one per node the whole "live" stream would flush only
                // after the run.
                task::yield_now().await;

                // Coarse cancel: stop scheduling further nodes and retire the tail's reads. A
                // node already mid-invoke isn't interrupted, while unreached outcomes stay
                // `Pending` and are omitted from the outcome.
                if self.ctx_manager.cancel.is_cancelled() {
                    for &pending_idx in &plan.process_order[process_idx..] {
                        if resolved.disposition[pending_idx] == Disposition::Run {
                            frame.abandon_input_reads(pending_idx);
                        }
                    }
                    break;
                }
                let e_node = &program[node_idx];
                let e_node_id = program.e_node_ids[node_idx];
                if !plan.verdicts[node_idx].wants_execute() {
                    continue;
                }
                match resolved.disposition[node_idx] {
                    Disposition::Cut => {
                        // Pruned by the pre-run cut: every consumer that would read this node reused
                        // a cache, so its output is never read. Report only a current resident value;
                        // unneeded disk blobs remain unprobed.
                        self.outcomes[node_idx] = NodeOutcome::Cut {
                            cached: frame.cache.is_resident_current(node_idx),
                        };
                        continue;
                    }
                    Disposition::MissingLambda => {
                        mark_skipped(
                            frame.cache,
                            &mut self.outcomes,
                            node_idx,
                            RunError::MissingLambda {
                                func_id: e_node.func_id,
                            },
                        );
                        continue;
                    }
                    Disposition::Reuse | Disposition::Run => {}
                }

                let demand = resolved.outputs.demand.slice(e_node.outputs);

                // The resolver's pre-run verdict is authoritative — a `Reuse` is never
                // re-derived here, since its producers may already be pruned (see `resolve.rs`).
                // The one sanctioned improvement is a `Run` whose stamped digest is `None`: the
                // resolver taints a node whose digest folds a Bind-delivered path value it
                // couldn't read yet (`hash_bound_fs_path`). Its producers settled earlier in
                // this walk (the `Run` verdict kept them alive), so re-stamp it now and serve
                // the cache on a hit — a genuinely uncacheable node (an impure cone) just
                // re-folds to `None` and runs as before. Reuse is served *before* the
                // errored-dependency check: a digest-valid cached value stays valid even when an
                // upstream re-ran for another consumer and failed, so it must not be cleared as
                // skipped.
                let reused = match resolved.disposition[node_idx] {
                    Disposition::Reuse => true,
                    Disposition::Run if frame.cache.slots[node_idx].current_digest.is_none() => {
                        frame
                            .resource_stamps
                            .prepare_node(
                                program,
                                frame.cache,
                                node_idx,
                                self.ctx_manager.cancel.clone(),
                            )
                            .await;
                        frame
                            .cache
                            .stamp_digest(program, frame.resource_stamps, node_idx);
                        let reused = frame
                            .cache
                            .hydrate_reuse(
                                program,
                                node_idx,
                                demand,
                                &mut self.ctx_manager.contexts,
                            )
                            .await;
                        if reused {
                            frame.abandon_input_reads(node_idx);
                        }
                        reused
                    }
                    Disposition::Run => false,
                    Disposition::Cut | Disposition::MissingLambda => {
                        unreachable!("cut and missing-lambda dispositions were handled above")
                    }
                };
                if reused {
                    // The resolver only *probed* a disk hit, so the decode happens here, at
                    // the node's own turn: producer-first order puts it ahead of every
                    // consumer that reads it, and `release_drained_outputs` below frees it
                    // on the same last-read bookkeeping a computed value gets. A no-op for a
                    // resident value and for the re-stamp arm above, which already loaded.
                    // A blob that stopped loading since the probe can't fall back to running
                    // — the cut already pruned this node's producers — so it fails the node
                    // and its consumers skip as errored-upstream.
                    if !frame
                        .cache
                        .hydrate_reuse(program, node_idx, demand, &mut self.ctx_manager.contexts)
                        .await
                    {
                        mark_skipped(
                            frame.cache,
                            &mut self.outcomes,
                            node_idx,
                            RunError::CacheLoadFailed {
                                func_id: e_node.func_id,
                            },
                        );
                        continue;
                    }
                    self.outcomes[node_idx] = NodeOutcome::Reused;
                    frame.emit_pinned_values(node_idx, events);
                    frame.release_drained_outputs(node_idx);
                    continue;
                }

                debug_assert!(!e_node.lambda.is_none());
                let func_id = e_node.func_id;

                if has_errored_dependency(program, &self.outcomes, node_idx) {
                    frame.abandon_input_reads(node_idx);
                    mark_skipped(
                        frame.cache,
                        &mut self.outcomes,
                        node_idx,
                        RunError::SkippedUpstream { func_id },
                    );
                    continue;
                }

                // Read already-resolved inputs and release each producer whose last read this
                // satisfies. A disk-reused producer was decoded at its own earlier turn.
                frame.collect_inputs(node_idx);

                let output_count = e_node.outputs.len as usize;
                let event_state = frame.cache.slots[node_idx].event_state.clone();
                debug_assert!(matches!(self.outcomes[node_idx], NodeOutcome::Pending));

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
                let result = {
                    let slot = frame.cache.slots[node_idx].invoke_slot(output_count);
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
                        let outputs = frame.cache.slots[node_idx].unbound_demanded_outputs(demand);
                        if outputs.is_empty() {
                            Ok(())
                        } else {
                            Err(RunError::OutputsNotProduced { func_id, outputs })
                        }
                    }
                    other => other,
                };
                let cancelled = matches!(&result, Err(RunError::Cancelled { .. }));
                let slot = &mut frame.cache.slots[node_idx];
                let succeeded = match result {
                    // The fresh output now corresponds to this node's current digest; record
                    // it so the next run's reuse check is a RAM hit.
                    Ok(()) => {
                        slot.stamp_produced();
                        self.outcomes[node_idx] = NodeOutcome::Ran { secs: run_time };
                        true
                    }
                    Err(error) => {
                        slot.clear_output();
                        self.outcomes[node_idx] = NodeOutcome::Failed {
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
                    if plan.event_sources.contains(node_idx) {
                        outcome.event_triggers.extend(
                            program.events[e_node.events]
                                .iter()
                                .enumerate()
                                .filter(|(_, event)| {
                                    !event.subscribers.is_empty() && !event.lambda.is_none()
                                })
                                .map(|(event_idx, event)| EventTrigger {
                                    event: ExecutionEventPort {
                                        e_node_id,
                                        event_idx,
                                    },
                                    lambda: event.lambda.clone(),
                                    state: event_state.clone(),
                                }),
                        );
                    }
                    // Deliver before later consumers can release values; host delivery is not a reader.
                    frame.emit_pinned_values(node_idx, events);
                    // The preceding reuse miss proves that no blob can cover this result.
                    frame
                        .cache
                        .store_node(
                            program,
                            node_idx,
                            StorePolicy::KnownMiss,
                            &mut self.ctx_manager.contexts,
                        )
                        .await;
                    frame.release_drained_outputs(node_idx);
                }
            }
        }

        self.ctx_manager.current_node = None;
        collect_execution_outcome(program, plan, &self.outcomes, start, outcome);
        outcome.logs.append(&mut self.ctx_manager.logs);
        outcome.cancelled = self.ctx_manager.cancel.is_cancelled();
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::execution::executor::Executor;
    use crate::execution::executor::outcomes::NodeOutcome;
    use crate::execution::identity::ExecutionNodeId;
    use crate::execution::program::ExecutionProgram;

    impl Executor {
        /// Whether `e_node_id` actually recomputed its lambda in the last run — i.e.
        /// wasn't reused from RAM/disk. Before any run (empty outcomes) every node
        /// reads as "ran", so plan-only introspection still sees the full schedule;
        /// an id absent from the installed program is a caller bug and panics.
        pub(crate) fn ran(&self, program: &ExecutionProgram, e_node_id: ExecutionNodeId) -> bool {
            let node_idx = program.e_node_index[&e_node_id];
            self.outcomes.get(node_idx).is_none_or(|outcome| {
                matches!(
                    outcome,
                    NodeOutcome::Ran { .. } | NodeOutcome::Failed { .. }
                )
            })
        }
    }
}

#[cfg(test)]
mod tests;
