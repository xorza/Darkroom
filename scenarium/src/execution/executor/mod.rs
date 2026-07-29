//! The run loop and its transient state. The `Executor` owns the shared
//! `ctx_manager` and the invoke scratch; the per-node cross-run cache lives in
//! the [`RuntimeCache`]. Given a
//! [`Resolved`] run — the program and the schedule
//! swept against it — plus that `RuntimeCache`,
//! [`Executor::run`] invokes each scheduled node's lambda and gathers outcomes.
//! Each node's per-run result is one [`NodeOutcome`] in the per-run outcome map.
//!
//! **Pre-run resolution.** The loop cannot be entered any other way: `Resolved` is minted
//! only by [`resolve`](crate::execution::schedule::Scheduled::resolve), so the disposition,
//! output demand,
//! and reader counts it reads were derived together and are authoritative for the whole run. A
//! [`NodeState::Reuse`] is never re-derived after its producers may have been cut. A cut
//! node (its cone feeds only cache hits, so a disk-cached node's stale upstream isn't
//! recomputed on reopen) gets [`NodeOutcome::Cut`]. A missing implementation is reported
//! without probing its cache or retaining its input cone. The one verdict the loop *improves*
//! is a `Run` whose stamped digest is `None` because a Bind-delivered path value exists
//! only once its producers settle: the loop prepares that identity off-thread, re-stamps at
//! reach time, and serves the cache on a hit.

mod node_outcome;

use std::time::Instant;

use tokio::task;

use common::CancelToken;

use crate::DynamicValue;
use crate::RamUsage;
use crate::execution::event::EventTrigger;
use crate::execution::identity::ExecutionEventPort;
use crate::execution::outcome::{ExecutionOutcome, NodeExecutionStatus, NodeStatus};
use crate::execution::program::index::{NodeColumn, NodeIdx, OutputAddr, OutputColumn, OutputIdx};
use crate::execution::report::{RunPhase, RunProgress, RunReporter};
use crate::node::lambda::{Invocation, InvokeError, OutputDemand};
use crate::runtime::context::ContextManager;
use crate::runtime::shared_any_state::SharedAnyState;

use crate::execution::cache::disk_store::StorePolicy;
use crate::execution::cache::runtime::RuntimeCache;
use crate::execution::error::RunError;
use crate::execution::executor::node_outcome::NodeOutcome;
use crate::execution::program::{ExecutionBinding, Program};
use crate::execution::schedule::{NodeState, Resolved, RunSchedule};

#[derive(Default, Debug)]
pub(crate) struct Executor {
    pub(crate) ctx_manager: ContextManager,
    /// Per-*invoke* scratch: the node's resolved inputs, refilled for each node that runs.
    inputs: Vec<DynamicValue>,
    /// The run's mutable copy of the resolved live binding counts. Input consumption or
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
    /// The run, resolved: the program, the schedule planned against it, and the
    /// dispositions, demand, and reader counts one sweep derived from both. One value
    /// because only [`Scheduled::resolve`](crate::execution::schedule::Scheduled::resolve)
    /// mints it — the loop cannot be handed a schedule nothing resolved, or one resolved
    /// against a different program.
    pub(crate) run: Resolved<'a>,
    pub(crate) cache: &'a mut RuntimeCache,
    /// Live per-node feedback, published ahead of the final outcome.
    pub(crate) reporter: &'a mut (dyn RunReporter + 'r),
    pub(crate) cancel: CancelToken,
}

#[derive(Default, Debug)]
pub(super) struct RemainingOutputReads {
    pub(super) counts: OutputColumn<u32>,
}

impl RemainingOutputReads {
    /// Take this run's starting counts from the schedule the loop is about to walk —
    /// the only source, so the counts can't come from a resolution the run isn't using.
    pub(super) fn seed(&mut self, schedule: &RunSchedule) {
        self.counts.clone_from(&schedule.outputs.readers);
    }

    fn is_last(&self, output_idx: OutputIdx) -> bool {
        self.counts[output_idx] == 1
    }

    pub(super) fn consume(&mut self, output_idx: OutputIdx) -> bool {
        let remaining = &mut self.counts[output_idx];
        debug_assert!(
            *remaining > 0,
            "read an output more often than the resolved run counted"
        );
        *remaining = remaining.wrapping_sub(1);
        *remaining == 0
    }

    fn node_drained(&self, program: &Program, node_idx: NodeIdx) -> bool {
        self.counts
            .slice(program[node_idx].outputs)
            .iter()
            .all(|remaining| *remaining == 0)
    }
}

impl Executor {
    /// Walk `schedule.process_order` (producer-first), giving each node one turn. The loop
    /// itself owns only the two decisions that end it early or skip it wholesale — the
    /// per-node work is [`ExecutionFrame::run_node`].
    pub(crate) async fn run(
        &mut self,
        request: RunRequest<'_, '_>,
        outcome: &mut ExecutionOutcome,
    ) {
        let RunRequest {
            run,
            cache,
            reporter,
            cancel,
        } = request;
        // The handle's job is done the moment the loop has it: it proved these two
        // belong together, and every step below reads them as the pair it vouched for.
        let (program, schedule) = (run.program(), run.schedule());

        outcome.clear();
        let start = Instant::now();
        // Hold the cancel flag on the context so lambdas can poll it inside
        // off-thread work, and so the loop-top / post-loop checks below read
        // one source.
        self.ctx_manager.cancel = cancel;
        self.ctx_manager.logs.clear();
        self.outcomes
            .reset(program.e_nodes.len(), NodeOutcome::Pending);
        self.remaining_reads.seed(schedule);

        {
            let mut frame = ExecutionFrame {
                program,
                schedule,
                cache,
                remaining_reads: &mut self.remaining_reads,
                inputs: &mut self.inputs,
                node_outcomes: &mut self.outcomes,
                ctx: &mut self.ctx_manager,
                reporter,
                outcome,
            };

            // The producer-first schedule excludes unseeded disabled nodes; the
            // resolved run cuts cache-hidden and blocked cones.
            for (process_idx, &node_idx) in schedule.process_order.iter().enumerate() {
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
        outcome.elapsed_secs = start.elapsed().as_secs_f64();
        outcome.logs.append(&mut self.ctx_manager.logs);
        outcome.cancelled = self.ctx_manager.cancel.is_cancelled();
    }

    /// Reduce this run's per-node column and the RAM it left resident into the caller's
    /// outcome — one [`NodeStatus`] row per node, which is also the row the host publishes.
    /// A node says nothing at all (no row) when it neither produced a result nor holds a
    /// value: the cancelled run's tail, a node the planner excluded, a pruned cut.
    ///
    /// Separate from [`run`](Self::run) because `node_ram` can only be measured once the
    /// engine has released what the run left dead — but taking the same [`Resolved`] the
    /// loop walked, so the outcome can only be collected against the program and schedule
    /// that produced it.
    ///
    /// Every column here spans the whole program, so this walks the program rather than
    /// `process_order`: a node the run never scheduled can still hold RAM from an earlier
    /// one, and that is the only thing it has to report.
    pub(crate) fn collect_outcome(
        &self,
        run: Resolved<'_>,
        node_ram: &NodeColumn<RamUsage>,
        outcome: &mut ExecutionOutcome,
    ) {
        let (program, schedule) = (run.program(), run.schedule());
        for (node_idx, node_outcome) in self.outcomes.iter_indexed() {
            let status = match node_outcome {
                // A reuse hit, or a node the cut pruned that still holds a resident value, are
                // both "available, not recomputed" — reported cached.
                NodeOutcome::Reused | NodeOutcome::Cut { cached: true } => {
                    Some(NodeExecutionStatus::Cached)
                }
                NodeOutcome::Ran { secs } => {
                    outcome.ran_node_count += 1;
                    Some(NodeExecutionStatus::Executed {
                        elapsed_secs: *secs,
                    })
                }
                // A cancelled invoke didn't complete, so it has neither a run to report nor a
                // failure of its own — the run as a whole is what was cancelled.
                NodeOutcome::Failed {
                    error: RunError::Cancelled { .. },
                    ..
                } => None,
                // A genuine failure did run: one row carries both the attempt's time and the
                // reason it ended, so nothing has to reconcile a node listed as two things.
                NodeOutcome::Failed { secs, error } => {
                    outcome.ran_node_count += 1;
                    Some(NodeExecutionStatus::Errored {
                        elapsed_secs: Some(*secs),
                        error: error.clone(),
                    })
                }
                NodeOutcome::Skipped { error } => Some(NodeExecutionStatus::Errored {
                    elapsed_secs: None,
                    error: error.clone(),
                }),
                // The planner may have stopped at this node for want of an input — the one
                // verdict that isn't the run loop's to give, since a node it never reached
                // holds no outcome of its own.
                NodeOutcome::Pending => self.missing_inputs(program, schedule, node_idx),
                NodeOutcome::Cut { cached: false } => None,
            };
            let ram = node_ram[node_idx];
            if status.is_none() && ram.total() == 0 {
                continue;
            }
            outcome.nodes.push(NodeStatus {
                e_node_id: program.e_node_ids[node_idx],
                status,
                ram,
            });
        }
    }

    /// The unsatisfied input ports of a node the planner verdicted `MissingInputs`, or
    /// `None` for any other node.
    ///
    /// Which ports is recomputed from the schedule's own predicate (the one the planner
    /// reached its verdict with) rather than stored: only the rare missing node pays for it,
    /// so it isn't worth a column spanning the program.
    fn missing_inputs(
        &self,
        program: &Program,
        schedule: &RunSchedule,
        node_idx: NodeIdx,
    ) -> Option<NodeExecutionStatus> {
        if !schedule.states[node_idx].missing_required_inputs() {
            return None;
        }
        let ports: Vec<usize> = program.inputs[program[node_idx].inputs]
            .iter()
            .enumerate()
            .filter(|(_, input)| schedule.input_missing(input))
            .map(|(port_idx, _)| port_idx)
            .collect();
        debug_assert!(
            !ports.is_empty(),
            "a node verdicted `MissingInputs` has at least one unsatisfied port"
        );
        Some(NodeExecutionStatus::MissingInputs { ports })
    }
}

/// One run's live state: the read-only schedule, the cache and per-invoke scratch it
/// mutates, and the sinks it reports through. Bundled so every step of the loop is a method
/// taking a node index rather than a closure over ten borrows — and so the disjoint-field
/// borrows the run needs (cache vs. contexts vs. outcomes) are expressed once, here.
///
/// `'r` is the reporter's own lifetime, kept separate from the frame's `'a`: a
/// `&mut dyn Trait` is invariant, so sharing one lifetime would extend every borrow here to
/// the caller's.
#[derive(Debug)]
pub(crate) struct ExecutionFrame<'a, 'r> {
    program: &'a Program,
    schedule: &'a RunSchedule,
    cache: &'a mut RuntimeCache,
    remaining_reads: &'a mut RemainingOutputReads,
    inputs: &'a mut Vec<DynamicValue>,
    /// Per-node results for this run, distinct from the whole-run `outcome` below.
    node_outcomes: &'a mut NodeColumn<NodeOutcome>,
    ctx: &'a mut ContextManager,
    reporter: &'a mut (dyn RunReporter + 'r),
    outcome: &'a mut ExecutionOutcome,
}

impl ExecutionFrame<'_, '_> {
    /// One node's turn. The resolved disposition decides which of the four things happens,
    /// and it is authoritative — a [`NodeState::Reuse`] is never re-derived here, since its
    /// producers may already be pruned (see [`resolve`](crate::execution::schedule::Scheduled::resolve)).
    async fn run_node(&mut self, node_idx: NodeIdx) {
        let e_node = &self.program[node_idx];
        let demand = self.schedule.outputs.demand.slice(e_node.outputs);
        match self.schedule.states[node_idx] {
            // `process_order` and the state column are written by the same arm
            // of the walk, so a scheduled node without a settled state is a
            // broken plan — not a node to quietly skip.
            NodeState::Unvisited => unreachable!("a scheduled node has a settled state"),
            // The planner excluded it, so it holds no result this run and its
            // outcome stays `Pending`.
            NodeState::Disabled | NodeState::MissingInputs => {}
            // Pruned by the pre-run cut: every consumer that would read this node reused a
            // cache, so its output is never read. Report only a current resident value;
            // unneeded disk blobs remain unprobed.
            NodeState::Cut => {
                self.node_outcomes[node_idx] = NodeOutcome::Cut {
                    cached: self.cache.is_resident_current(node_idx),
                };
            }
            NodeState::MissingLambda => {
                let error = RunError::MissingLambda {
                    func_id: e_node.func_id,
                };
                self.mark_skipped(node_idx, error);
            }
            NodeState::Reuse => self.serve_reuse(node_idx, demand).await,
            // Reuse is settled *before* the errored-dependency check inside `invoke_node`: a
            // digest-valid cached value stays valid even when an upstream re-ran for another
            // consumer and failed, so it must not be cleared as skipped.
            NodeState::Run => {
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
        let schedule = self.schedule;
        for &node_idx in &schedule.process_order[from_process_idx..] {
            if schedule.states[node_idx] == NodeState::Run {
                self.abandon_input_reads(node_idx);
            }
        }
    }

    /// Serve a resolved reuse. The sweep only *probed* a disk hit, so the decode happens
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
            .hydrate_reuse(node_idx, demand, &mut self.ctx.contexts)
            .await
        {
            let error = RunError::CacheLoadFailed {
                func_id: program[node_idx].func_id,
            };
            self.mark_skipped(node_idx, error);
            return;
        }
        self.node_outcomes[node_idx] = NodeOutcome::Reused;
        self.release_drained_outputs(node_idx);
    }

    /// Whether this node's lambda still has to run — and the late second chance at reuse
    /// that decides it.
    ///
    /// The one verdict the loop *improves*: a `Run` whose stamped digest is `None` because it
    /// folds a Bind-delivered path value the sweep could not read yet
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
        if self.cache[node_idx].current_digest.is_some() {
            return true;
        }
        let program = self.program;
        let cancel = self.ctx.cancel.clone();
        let hydrated = self
            .cache
            .restamp_and_hydrate(node_idx, demand, &mut self.ctx.contexts, cancel)
            .await;
        match hydrated {
            // Attributable to exactly this node, so it fails as one rather
            // than taking the run down — and the invoke is skipped because
            // the node is already marked, and running it would report a
            // second, less specific failure for the same cause.
            Err(error) => {
                let run_error = RunError::ResourceUnavailable {
                    func_id: program[node_idx].func_id,
                    message: error.to_string(),
                };
                self.mark_skipped(node_idx, run_error);
                false
            }
            Ok(false) => true,
            Ok(true) => {
                // It came in as `Run`, so the sweep counted it as a
                // reader of each producer — reads it will now not take.
                // Handing them back is what lets a producer's value go the
                // moment its last real reader is gone. A `Reuse` node
                // never owned them, which is why `serve_reuse` has no
                // such line.
                self.abandon_input_reads(node_idx);
                self.node_outcomes[node_idx] = NodeOutcome::Reused;
                self.release_drained_outputs(node_idx);
                false
            }
        }
    }

    /// Invoke the node's lambda and record what came of it, persisting a success to disk
    /// right away — so a long run's earlier caches survive a later failure or cancel.
    async fn invoke_node(&mut self, node_idx: NodeIdx, demand: &[OutputDemand]) {
        let program = self.program;
        let e_node = &program[node_idx];
        let e_node_id = program.e_node_ids[node_idx];
        let func_id = e_node.func_id;
        debug_assert!(!e_node.lambda.is_none());

        if self.has_errored_dependency(node_idx) {
            self.abandon_input_reads(node_idx);
            let error = RunError::SkippedUpstream { func_id };
            self.mark_skipped(node_idx, error);
            return;
        }

        // Read already-resolved inputs and release each producer whose last read this
        // satisfies. A disk-reused producer was decoded at its own earlier turn.
        self.collect_inputs(node_idx);

        let event_state = self.cache[node_idx].event_state.clone();
        debug_assert!(matches!(self.node_outcomes[node_idx], NodeOutcome::Pending));

        // Attribute any logs this node emits to it (read by `ContextManager::log`).
        self.ctx.current_node = Some(e_node_id);
        let invoke_start = Instant::now();
        self.reporter.progress(RunProgress {
            e_node_id,
            phase: RunPhase::Started { at: invoke_start },
        });

        let result = {
            let slot = self.cache[node_idx].invoke_slot(e_node.outputs.len as usize);
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
            Ok(()) => match self.cache[node_idx].unbound_demanded_outputs(demand) {
                outputs if outputs.is_empty() => Ok(()),
                outputs => Err(RunError::OutputsNotProduced { func_id, outputs }),
            },
            other => other,
        };
        let cancelled = matches!(&result, Err(RunError::Cancelled { .. }));
        let slot = &mut self.cache[node_idx];
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

        if self.schedule.event_sources.contains(node_idx) {
            self.collect_event_triggers(node_idx, &event_state);
        }
        // Persist this node's cache the moment it finishes (durable as the run progresses),
        // not at the end of the whole run. The snapshot is taken synchronously inside
        // `store_node`; only the write awaits, so the cache borrow doesn't cross it. The
        // preceding reuse miss proves that no blob can cover this result.
        self.cache
            .store_node(node_idx, StorePolicy::KnownMiss, &mut self.ctx.contexts)
            .await;
        self.release_drained_outputs(node_idx);
    }

    /// Drop `e_node_id` from this run: clear any stale cached output so it isn't served as
    /// this run's result, and record the outcome under the caller's reason —
    /// [`RunError::SkippedUpstream`] for an errored dependency,
    /// [`RunError::MissingLambda`] for a func with no implementation, or
    /// [`RunError::CacheLoadFailed`] for a probed blob that no longer loads.
    fn mark_skipped(&mut self, node_idx: NodeIdx, error: RunError) {
        self.cache[node_idx].clear_output();
        self.node_outcomes[node_idx] = NodeOutcome::Skipped { error };
    }

    /// Whether any node this one binds an input to already failed or was skipped for
    /// an error — the check that turns an upstream failure into a skipped consumer.
    fn has_errored_dependency(&self, node_idx: NodeIdx) -> bool {
        let program = self.program;
        program.inputs[program[node_idx].inputs]
            .iter()
            .any(|input| {
                matches!(&input.binding, ExecutionBinding::Bind(addr) if self.node_outcomes[addr.node_idx].error().is_some())
            })
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

    pub(super) fn collect_inputs(&mut self, node_idx: NodeIdx) {
        self.inputs.clear();
        for input in &self.program.inputs[self.program[node_idx].inputs] {
            let binding = &input.binding;
            let value = match binding {
                ExecutionBinding::None => DynamicValue::Unbound,
                ExecutionBinding::Const(value) => value.into(),
                ExecutionBinding::Bind(addr) if !self.producer_runs(*addr) => {
                    // Nothing will produce this. Only an *optional* input
                    // can reach here — `input_missing` turns a required
                    // one into a `MissingInputs` verdict, so this node
                    // would not be running at all — and unbound is
                    // precisely what optional means. The sweep planned
                    // no read for it either, so none is completed.
                    DynamicValue::Unbound
                }
                ExecutionBinding::Bind(addr) => {
                    let address = *addr;
                    let output_idx = self.program.output_idx(address);
                    let take = self.remaining_reads.is_last(output_idx)
                        && !self.program[address.node_idx].cache.caches_in_ram();
                    let value = self
                        .cache
                        .read_output_port(address, take)
                        .expect("a resolved producer output must be resident when consumed");
                    self.complete_planned_read(address);
                    value
                }
            };
            self.inputs.push(value);
        }
    }

    /// Whether the plan will run `addr`'s producer — the same predicate the
    /// sweep registers reader counts by, so a read is completed exactly
    /// when one was planned.
    fn producer_runs(&self, addr: OutputAddr) -> bool {
        self.schedule.states[addr.node_idx].is_runnable()
    }

    /// Abandons every bound-input read owned by a consumer that will not invoke, allowing
    /// non-RAM producer values to be released as soon as their remaining readers disappear.
    pub(super) fn abandon_input_reads(&mut self, consumer_idx: NodeIdx) {
        for input in &self.program.inputs[self.program[consumer_idx].inputs] {
            if let ExecutionBinding::Bind(address) = &input.binding
                && self.producer_runs(*address)
            {
                self.complete_planned_read(*address);
            }
        }
    }

    pub(super) fn release_drained_outputs(&mut self, node_idx: NodeIdx) {
        if !self.program[node_idx].cache.caches_in_ram()
            && self.remaining_reads.node_drained(self.program, node_idx)
        {
            self.cache[node_idx].clear_output();
        }
    }

    /// Completes one resolved read and releases its producer port or slot when no
    /// planned reader can still use it.
    fn complete_planned_read(&mut self, address: OutputAddr) {
        let output_idx = self.program.output_idx(address);
        if !self.remaining_reads.consume(output_idx)
            || self.cache[address.node_idx].output_values().is_none()
        {
            return;
        }
        if self
            .remaining_reads
            .node_drained(self.program, address.node_idx)
        {
            self.release_drained_outputs(address.node_idx);
        } else if !self.program[address.node_idx].cache.caches_in_ram() {
            self.cache[address.node_idx].clear_output_port(address.port_idx);
        }
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::execution::executor::Executor;
    use crate::execution::executor::node_outcome::NodeOutcome;
    use crate::execution::identity::ExecutionNodeId;
    use crate::execution::program::Program;

    impl Executor {
        /// Whether `e_node_id` actually recomputed its lambda in the last run — i.e.
        /// wasn't reused from RAM/disk. Before any run (empty outcomes) every node
        /// reads as "ran", so plan-only introspection still sees the full schedule;
        /// an id absent from the installed program is a caller bug and panics.
        pub(crate) fn ran(&self, program: &Program, e_node_id: ExecutionNodeId) -> bool {
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
