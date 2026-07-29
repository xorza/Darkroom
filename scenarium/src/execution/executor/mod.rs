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

use std::time::Instant;

use tokio::task;

use common::CancelToken;

use crate::DynamicValue;
use crate::execution::event::EventTrigger;
use crate::execution::identity::ExecutionEventPort;
use crate::execution::outcome::ExecutionOutcome;
use crate::execution::program::index::{NodeColumn, NodeIdx, OutputAddr, OutputColumn, OutputIdx};
use crate::execution::report::{RunPhase, RunProgress, RunReporter};
use crate::node::lambda::{Invocation, InvokeError, OutputDemand};
use crate::runtime::context::ContextManager;
use crate::runtime::shared_any_state::SharedAnyState;

use crate::execution::cache::runtime::RuntimeCache;
use crate::execution::disk_store::StorePolicy;
use crate::execution::error::RunError;
use crate::execution::executor::outcomes::{
    NodeOutcome, collect_execution_outcome, has_errored_dependency, mark_skipped,
};
use crate::execution::plan::ExecutionPlan;
use crate::execution::program::{ExecutionBinding, ExecutionProgram};
use crate::execution::resolve::{Disposition, ResolvedRun};

#[derive(Default, Debug)]
pub(crate) struct Executor {
    pub(crate) ctx_manager: ContextManager,
    /// Per-*invoke* scratch: the node's resolved inputs, refilled for each node that runs.
    inputs: Vec<DynamicValue>,
    /// The run's mutable copy of the resolver's live binding counts. Input consumption or
    /// retirement decrements it; production demand and host pins remain immutable.
    remaining_reads: RemainingOutputReads,
    /// Per-run outcome per node (see [`NodeOutcome`]), aligned to the program's
    /// dense node vector. Reused across runs and rebuilt each run.
    outcomes: NodeColumn<NodeOutcome>,
}

/// Everything one run borrows from the engine. A parameter struct rather than eight
/// positional arguments, so the call site names each collaborator and a new one doesn't
/// become another slot to count. `'r` is the reporter's own lifetime — see
/// [`ExecutionFrame`].
#[derive(Debug)]
pub(crate) struct RunRequest<'a, 'r> {
    pub(crate) program: &'a ExecutionProgram,
    pub(crate) plan: &'a ExecutionPlan,
    pub(crate) resolved: &'a ResolvedRun,
    pub(crate) cache: &'a mut RuntimeCache,
    /// Live per-node feedback, published ahead of the final outcome.
    pub(crate) reporter: &'a mut (dyn RunReporter + 'r),
    pub(crate) cancel: CancelToken,
}

impl Executor {
    /// Walk `plan.process_order` (producer-first), giving each node one turn. The loop
    /// itself owns only the two decisions that end it early or skip it wholesale — the
    /// per-node work is [`ExecutionFrame::run_node`].
    pub(crate) async fn run(
        &mut self,
        request: RunRequest<'_, '_>,
        outcome: &mut ExecutionOutcome,
    ) {
        let RunRequest {
            program,
            plan,
            resolved,
            cache,
            resource_stamper,
            reporter,
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
                resource_stamper,
                remaining_reads: &mut self.remaining_reads,
                inputs: &mut self.inputs,
                node_outcomes: &mut self.outcomes,
                ctx: &mut self.ctx_manager,
                reporter,
                outcome,
            };

            // The producer-first schedule excludes unseeded disabled nodes; the
            // resolved run cuts cache-hidden and blocked cones.
            for (process_idx, &node_idx) in plan.process_order.iter().enumerate() {
                // A schedule of sync-completing lambdas never suspends on its own, so
                // without this it would hold its executor thread from the first node to the
                // last — starving everything sharing that thread (event-loop lambdas, and
                // every other task under a current-thread runtime).
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
///
/// `'r` is the reporter's own lifetime, kept separate from the frame's `'a`: a
/// `&mut dyn Trait` is invariant, so sharing one lifetime would extend every borrow here to
/// the caller's.
#[derive(Debug)]
pub(crate) struct ExecutionFrame<'a, 'r> {
    program: &'a ExecutionProgram,
    plan: &'a ExecutionPlan,
    resolved: &'a ResolvedRun,
    cache: &'a mut RuntimeCache,
    resource_stamper: &'a mut ResourceStamper,
    remaining_reads: &'a mut RemainingOutputReads,
    inputs: &'a mut Vec<DynamicValue>,
    /// Per-node results for this run, distinct from the whole-run `outcome` below.
    node_outcomes: &'a mut NodeColumn<NodeOutcome>,
    ctx: &'a mut ContextManager,
    reporter: &'a mut (dyn RunReporter + 'r),
    outcome: &'a mut ExecutionOutcome,
}

impl ExecutionFrame<'_, '_> {
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
                if self.needs_invoke(node_idx, demand).await {
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

    /// Whether this node's lambda still has to run — and the late second chance at reuse
    /// that decides it.
    ///
    /// The one verdict the loop *improves*: a `Run` whose stamped digest is `None` because it
    /// folds a Bind-delivered path value the resolver couldn't read yet
    /// (`hash_bound_fs_path`). Its producers settled earlier in this walk — the `Run` verdict
    /// kept them alive — so re-stamp it now and serve the cache on a hit. A genuinely
    /// uncacheable node (an impure cone) just re-folds to `None` and runs as before.
    ///
    /// `true` — nothing was improvable, or the improved digest still missed: the node runs.
    /// `false` — its turn is already settled, whether it was *served* from cache or *failed*
    /// on its own resource. The two are one answer to the caller, which asks only whether to
    /// invoke; which of them happened is in the node's outcome.
    ///
    /// Loading *before* retiring this node's input reads is what lets a failed load fall
    /// through to a normal invoke here, unlike [`serve_reuse`](Self::serve_reuse).
    async fn needs_invoke(&mut self, node_idx: NodeIdx, demand: &[OutputDemand]) -> bool {
        if self.cache.slots[node_idx].current_digest.is_some() {
            return true;
        }
        let program = self.program;
        let cancel = self.ctx.cancel.clone();
        if let Err(error) = self
            .resource_stamper
            .prepare_node(program, self.cache, node_idx, cancel)
            .await
        {
            // Attributable to exactly this node, so it fails as one rather
            // than taking the run down — and the invoke is skipped because
            // the node is already marked, and running it would report a
            // second, less specific failure for the same cause.
            let run_error = RunError::ResourceUnavailable {
                func_id: program[node_idx].func_id,
                message: error.to_string(),
            };
            mark_skipped(self.cache, self.node_outcomes, node_idx, run_error);
            return false;
        }
        self.cache
            .stamp_digest(program, self.resource_stamper, node_idx);
        if !self
            .cache
            .hydrate_reuse(program, node_idx, demand, &mut self.ctx.contexts)
            .await
        {
            return true;
        }
        self.abandon_input_reads(node_idx);
        self.deliver_reused(node_idx);
        false
    }

    /// The tail both reuse paths share, once the value is readable.
    fn deliver_reused(&mut self, node_idx: NodeIdx) {
        self.node_outcomes[node_idx] = NodeOutcome::Reused;
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
        self.reporter.progress(RunProgress {
            e_node_id,
            phase: RunPhase::Started { at: invoke_start },
        });

        let result = {
            let slot = self.cache.slots[node_idx].invoke_slot(e_node.outputs.len as usize);
            e_node
                .lambda
                .invoke(Invocation {
                    ctx: self.ctx,
                    state: slot.state,
                    event_state: &event_state,
                    inputs: self.inputs,
                    demand,
                    outputs: slot.outputs,
                })
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
            self.reporter.progress(RunProgress {
                e_node_id,
                phase: RunPhase::Finished {
                    elapsed_secs: run_time,
                },
            });
        }
        if !succeeded {
            return;
        }

        if self.plan.event_sources.contains(node_idx) {
            self.collect_event_triggers(node_idx, &event_state);
        }
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
