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
use crate::execution::identity::{ExecutionEventPort, ExecutionNodeId};
use crate::execution::outcome::ExecutionOutcome;
use crate::execution::program::index::{NodeColumn, NodeIdx};
use crate::execution::report::{RunEvent, RunPhase, RunProgress};
use crate::node::lambda::{InvokeError, InvokeInput, OutputDemand};
use crate::runtime::context::ContextManager;
use crate::runtime::shared_any_state::SharedAnyState;

use crate::execution::cache::runtime::RuntimeCache;
use crate::execution::disk_store::StorePolicy;
use crate::execution::error::RunError;
use crate::execution::executor::outcomes::{
    NodeOutcome, collect_execution_outcome, has_errored_dependency, mark_skipped,
};
use crate::execution::executor::value_flow::RemainingOutputReads;
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

/// Everything one run borrows from the engine. A parameter struct rather than eight
/// positional arguments, so the call site names each collaborator and a new one doesn't
/// become another slot to count.
#[derive(Debug)]
pub(crate) struct RunRequest<'a> {
    pub(crate) program: &'a ExecutionProgram,
    pub(crate) plan: &'a ExecutionPlan,
    pub(crate) resolved: &'a ResolvedRun,
    pub(crate) cache: &'a mut RuntimeCache,
    pub(crate) resource_stamps: &'a mut RunResourceStamps,
    /// When set, live per-node feedback streams ahead of the final outcome.
    pub(crate) events: Option<&'a UnboundedSender<RunEvent>>,
    pub(crate) cancel: CancelToken,
}

impl Executor {
    /// Walk `plan.process_order` (producer-first), giving each node one turn. The loop
    /// itself owns only the two decisions that end it early or skip it wholesale — the
    /// per-node work is [`ExecutionFrame::run_node`].
    pub(crate) async fn run(&mut self, request: RunRequest<'_>, outcome: &mut ExecutionOutcome) {
        let RunRequest {
            program,
            plan,
            resolved,
            cache,
            resource_stamps,
            events,
            cancel,
        } = request;

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
                resolved,
                cache,
                resource_stamps,
                remaining_reads: &mut self.remaining_reads,
                inputs: &mut self.inputs,
                node_outcomes: &mut self.outcomes,
                ctx: &mut self.ctx_manager,
                events,
                outcome: &mut *outcome,
            };

            // The producer-first schedule excludes unseeded disabled nodes; the
            // resolved run cuts cache-hidden and blocked cones.
            for (process_idx, &node_idx) in plan.process_order.iter().enumerate() {
                // Drain point for the live-report relay: sync-completing lambdas
                // give the worker's select no suspension point of their own, and
                // without one per node the whole "live" stream would flush only
                // after the run.
                task::yield_now().await;

                if frame.ctx.cancel.is_cancelled() {
                    frame.retire_cancelled_tail(process_idx);
                    break;
                }
                frame.run_node(node_idx).await;
            }
        }

        self.ctx_manager.current_node = None;
        collect_execution_outcome(program, plan, &self.outcomes, start, outcome);
        outcome.logs.append(&mut self.ctx_manager.logs);
        outcome.cancelled = self.ctx_manager.cancel.is_cancelled();
    }
}

/// One run's live state: the read-only schedule, the cache and per-invoke scratch it
/// mutates, and the sinks it reports through. Bundled so every step of the loop is a method
/// taking a node index rather than a closure over ten borrows — and so the disjoint-field
/// borrows the run needs (cache vs. contexts vs. outcomes) are expressed once, here.
///
/// Value movement — input collection, pinned delivery, and the last-read releases — lives in
/// [`value_flow`].
#[derive(Debug)]
pub(crate) struct ExecutionFrame<'a> {
    program: &'a ExecutionProgram,
    plan: &'a ExecutionPlan,
    resolved: &'a ResolvedRun,
    cache: &'a mut RuntimeCache,
    resource_stamps: &'a mut RunResourceStamps,
    remaining_reads: &'a mut RemainingOutputReads,
    inputs: &'a mut Vec<InvokeInput>,
    /// Per-node results for this run, distinct from the whole-run `outcome` below.
    node_outcomes: &'a mut NodeColumn<NodeOutcome>,
    ctx: &'a mut ContextManager,
    events: Option<&'a UnboundedSender<RunEvent>>,
    outcome: &'a mut ExecutionOutcome,
}

impl ExecutionFrame<'_> {
    /// One node's turn. The resolver's disposition decides which of the four things happens,
    /// and it is authoritative — a [`Disposition::Reuse`] is never re-derived here, since its
    /// producers may already be pruned (see `resolve.rs`).
    async fn run_node(&mut self, node_idx: NodeIdx) {
        if !self.plan.verdicts[node_idx].wants_execute() {
            return;
        }
        let e_node = &self.program[node_idx];
        let demand = self.resolved.outputs.demand.slice(e_node.outputs);
        match self.resolved.disposition[node_idx] {
            // Pruned by the pre-run cut: every consumer that would read this node reused a
            // cache, so its output is never read. Report only a current resident value;
            // unneeded disk blobs remain unprobed.
            Disposition::Cut => {
                self.node_outcomes[node_idx] = NodeOutcome::Cut {
                    cached: self.cache.is_resident_current(node_idx),
                };
            }
            Disposition::MissingLambda => {
                let error = RunError::MissingLambda {
                    func_id: e_node.func_id,
                };
                mark_skipped(self.cache, self.node_outcomes, node_idx, error);
            }
            Disposition::Reuse => self.serve_reuse(node_idx, demand).await,
            // Reuse is settled *before* the errored-dependency check inside `invoke_node`: a
            // digest-valid cached value stays valid even when an upstream re-ran for another
            // consumer and failed, so it must not be cleared as skipped.
            Disposition::Run => {
                if !self.improved_to_reuse(node_idx, demand).await {
                    self.invoke_node(node_idx, demand).await;
                }
            }
        }
    }

    /// Coarse cancel: stop scheduling further nodes and retire the tail's reads. A node
    /// already mid-invoke isn't interrupted, while unreached outcomes stay `Pending` and are
    /// omitted from the outcome.
    fn retire_cancelled_tail(&mut self, from_process_idx: usize) {
        let plan = self.plan;
        for &node_idx in &plan.process_order[from_process_idx..] {
            if self.resolved.disposition[node_idx] == Disposition::Run {
                self.abandon_input_reads(node_idx);
            }
        }
    }

    /// Serve a resolved reuse. The resolver only *probed* a disk hit, so the decode happens
    /// here, at the node's own turn: producer-first order puts it ahead of every consumer
    /// that reads it, and the release below frees it on the same last-read bookkeeping a
    /// computed value gets.
    ///
    /// A blob that stopped loading since the probe cannot fall back to running — the cut
    /// already pruned this node's producers — so it fails the node and its consumers skip as
    /// errored-upstream.
    async fn serve_reuse(&mut self, node_idx: NodeIdx, demand: &[OutputDemand]) {
        let program = self.program;
        if !self
            .cache
            .hydrate_reuse(program, node_idx, demand, &mut self.ctx.contexts)
            .await
        {
            let error = RunError::CacheLoadFailed {
                func_id: program[node_idx].func_id,
            };
            mark_skipped(self.cache, self.node_outcomes, node_idx, error);
            return;
        }
        self.deliver_reused(node_idx);
    }

    /// The one verdict the loop *improves*: a `Run` whose stamped digest is `None` because it
    /// folds a Bind-delivered path value the resolver couldn't read yet
    /// (`hash_bound_fs_path`). Its producers settled earlier in this walk — the `Run` verdict
    /// kept them alive — so re-stamp it now and serve the cache on a hit. A genuinely
    /// uncacheable node (an impure cone) just re-folds to `None` and runs as before.
    ///
    /// Loading *before* retiring this node's input reads is what lets a failed load fall
    /// through to a normal invoke here, unlike [`serve_reuse`](Self::serve_reuse).
    async fn improved_to_reuse(&mut self, node_idx: NodeIdx, demand: &[OutputDemand]) -> bool {
        if self.cache.slots[node_idx].current_digest.is_some() {
            return false;
        }
        let program = self.program;
        let cancel = self.ctx.cancel.clone();
        self.resource_stamps
            .prepare_node(program, self.cache, node_idx, cancel)
            .await;
        self.cache
            .stamp_digest(program, self.resource_stamps, node_idx);
        if !self
            .cache
            .hydrate_reuse(program, node_idx, demand, &mut self.ctx.contexts)
            .await
        {
            return false;
        }
        self.abandon_input_reads(node_idx);
        self.deliver_reused(node_idx);
        true
    }

    /// The tail both reuse paths share, once the value is readable.
    fn deliver_reused(&mut self, node_idx: NodeIdx) {
        self.node_outcomes[node_idx] = NodeOutcome::Reused;
        self.emit_pinned_values(node_idx);
        self.release_drained_outputs(node_idx);
    }

    /// Invoke the node's lambda and record what came of it, persisting a success to disk
    /// right away — so a long run's earlier caches survive a later failure or cancel.
    async fn invoke_node(&mut self, node_idx: NodeIdx, demand: &[OutputDemand]) {
        let program = self.program;
        let e_node = &program[node_idx];
        let e_node_id = program.e_node_ids[node_idx];
        let func_id = e_node.func_id;
        debug_assert!(!e_node.lambda.is_none());

        if has_errored_dependency(program, self.node_outcomes, node_idx) {
            self.abandon_input_reads(node_idx);
            let error = RunError::SkippedUpstream { func_id };
            mark_skipped(self.cache, self.node_outcomes, node_idx, error);
            return;
        }

        // Read already-resolved inputs and release each producer whose last read this
        // satisfies. A disk-reused producer was decoded at its own earlier turn.
        self.collect_inputs(node_idx);

        let event_state = self.cache.slots[node_idx].event_state.clone();
        debug_assert!(matches!(self.node_outcomes[node_idx], NodeOutcome::Pending));

        // Attribute any logs this node emits to it (read by `ContextManager::log`).
        self.ctx.current_node = Some(e_node_id);
        let invoke_start = Instant::now();
        self.report_progress(e_node_id, RunPhase::Started { at: invoke_start });

        let result = {
            let slot = self.cache.slots[node_idx].invoke_slot(e_node.outputs.len as usize);
            e_node
                .lambda
                .invoke(
                    self.ctx,
                    slot.state,
                    &event_state,
                    self.inputs,
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

        // A cancellable lambda reports a cancel itself (→ `RunError::Cancelled` above). This
        // is the safety net for the rest: a lambda that doesn't poll the token (a builtin, a
        // single decode) but ran while the run was cancelled returns `Ok` with a result from
        // an aborted run — map that to `Cancelled` too so its output isn't cached. A genuine
        // error stands on its own, even mid-cancel.
        let result = match result {
            Ok(()) if self.ctx.cancel.is_cancelled() => Err(RunError::Cancelled { func_id }),
            Ok(()) => match self.cache.slots[node_idx].unbound_demanded_outputs(demand) {
                outputs if outputs.is_empty() => Ok(()),
                outputs => Err(RunError::OutputsNotProduced { func_id, outputs }),
            },
            other => other,
        };
        let cancelled = matches!(&result, Err(RunError::Cancelled { .. }));
        let slot = &mut self.cache.slots[node_idx];
        let succeeded = match result {
            // The fresh output now corresponds to this node's current digest; record it so
            // the next run's reuse check is a RAM hit.
            Ok(()) => {
                slot.stamp_produced();
                self.node_outcomes[node_idx] = NodeOutcome::Ran { secs: run_time };
                true
            }
            Err(error) => {
                slot.clear_output();
                self.node_outcomes[node_idx] = NodeOutcome::Failed {
                    secs: run_time,
                    error,
                };
                false
            }
        };
        // No `Finished` for the cancelled node — it didn't complete; the consumer would
        // otherwise paint it executed live.
        if !cancelled {
            self.report_progress(
                e_node_id,
                RunPhase::Finished {
                    elapsed_secs: run_time,
                },
            );
        }
        if !succeeded {
            return;
        }

        if self.plan.event_sources.contains(node_idx) {
            self.collect_event_triggers(node_idx, &event_state);
        }
        // Deliver before later consumers can release values; host delivery is not a reader.
        self.emit_pinned_values(node_idx);
        // Persist this node's cache the moment it finishes (durable as the run progresses),
        // not at the end of the whole run. The snapshot is taken synchronously inside
        // `store_node`; only the write awaits, so the cache borrow doesn't cross it. The
        // preceding reuse miss proves that no blob can cover this result.
        self.cache
            .store_node(
                program,
                node_idx,
                StorePolicy::KnownMiss,
                &mut self.ctx.contexts,
            )
            .await;
        self.release_drained_outputs(node_idx);
    }

    /// Hand the run's outcome the triggers a freshly initialized event source owns — only
    /// events that have a subscriber and an implementation can fire.
    fn collect_event_triggers(&mut self, node_idx: NodeIdx, event_state: &SharedAnyState) {
        let program = self.program;
        let e_node_id = program.e_node_ids[node_idx];
        self.outcome.event_triggers.extend(
            program.events[program[node_idx].events]
                .iter()
                .enumerate()
                .filter(|(_, event)| !event.subscribers.is_empty() && !event.lambda.is_none())
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

    fn report_progress(&self, e_node_id: ExecutionNodeId, phase: RunPhase) {
        if let Some(events) = self.events {
            events
                .send(RunEvent::Progress(RunProgress { e_node_id, phase }))
                .expect(EVENTS_OUTLIVE_RUN);
        }
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
