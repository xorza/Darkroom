//! The run loop and its transient state. The `Executor` owns the shared `ctx_manager`, the invoke
//! scratch, and each node's cross-run lambda state; the per-node cross-run output cache lives in
//! the [`RuntimeCache`]. [`Executor::run`] invokes each scheduled node's lambda and gathers one
//! [`NodeOutcome`] per node.
//!
//! **Pre-run resolution.** The loop reads a schedule
//! [`resolve::sweep`](crate::exec::resolve::sweep)
//! has already swept, so its disposition, output demand, and reader counts are authoritative for
//! the whole run. A [`NodeState::Reuse`] is never re-derived after its producers may have been
//! cut. A cut node (its cone feeds only cache hits, so a disk-cached node's stale upstream isn't
//! recomputed on reopen) gets [`NodeOutcome::Cut`]. A missing implementation is reported without
//! probing its cache or retaining its input cone. The one verdict the loop *improves* is a `Run`
//! whose stamped digest is `None` because a Bind-delivered path value exists only once its
//! producers settle: the loop prepares that identity off-thread, re-stamps at reach time, and
//! serves the cache on a hit.

use std::sync::Arc;
use std::time::Instant;

use tokio::task;

use common::{CancelToken, ErrorChain};

use crate::containers::col::Col;
use crate::data::dynamic_value::DynamicValue;
use crate::data::path_form::PathForm;
use crate::data::ram_usage::RamUsage;
use crate::exec::idx::{NodeIdx, OutputAddr, OutputIdx};
use crate::exec::report::event_trigger::EventTrigger;
use crate::exec::report::exec_outcome::ExecOutcome;
use crate::exec::report::node_status::{NodeExecStatus, NodeStatus};
use crate::exec::report::run_reporter::{RunPhase, RunReporter};
use crate::graph::func::error::InvokeError;
use crate::graph::func::lambda::{Invocation, OutputDemand};
use crate::graph::identity::EventPort;
use crate::runtime::ctx_manager::CtxManager;
use crate::runtime::log_entry::LogLevel;

use crate::exec::cache::disk_store::StorePolicy;
use crate::exec::cache::runtime_cache::{Restamp, ReuseOutcome, RuntimeCache};
use crate::exec::cache::runtime_slot::OutputRead;
use crate::exec::cache::stamp_job::error::StampError;
use crate::exec::compile::compiled_graph::{CompiledGraph, ExecBinding};
use crate::exec::error::RunError;
use crate::exec::executor::node_runtime::NodeRuntime;
use crate::exec::schedule::{Delivery, NodeState, RunSchedule};

mod node_runtime;

/// What became of a node this run — one result map, so run-time facts can't contradict (a node
/// can't be `Reused` yet carry a run time).
#[derive(Debug, Clone, Default)]
enum NodeOutcome {
    /// Not reached this run: skipped for missing inputs, below a cancel, or unscheduled.
    #[default]
    Pending,
    /// Served from a RAM/disk cache under an unchanged digest — counted as cached.
    Reused,
    /// Pruned by the pre-run cut: every consumer that would read this node reused a cache, so its
    /// lambda is skipped. `cached` is whether its output remains resident; unprobed disk blobs
    /// are not runtime cache state.
    Cut { cached: bool },
    /// Its lambda ran and succeeded, taking `secs`.
    Ran { secs: f64 },
    /// Its lambda ran but errored — an invoke failure, or a cancel mid-invoke.
    Failed { error: RunError },
    /// Never ran — an upstream dependency errored, its func has no implementation attached,
    /// or the cached output it was resolved to reuse failed to load.
    Skipped { error: RunError },
}

impl NodeOutcome {
    /// The run error the node carries — a failed run, or a node skipped for an error.
    const fn error(&self) -> Option<&RunError> {
        match self {
            Self::Failed { error, .. } | Self::Skipped { error } => Some(error),
            _ => None,
        }
    }
}

#[derive(Default, Debug)]
pub(crate) struct Executor {
    pub(crate) ctx_manager: CtxManager,
    /// Per-*invoke* scratch: the node's resolved inputs, refilled for each node that runs.
    inputs: Vec<DynamicValue>,
    /// The run's mutable copy of the resolved live binding counts; the resolved demand stays as
    /// the sweep left it.
    remaining_reads: RemainingOutputReads,
    /// Per-run outcome per node, aligned to the program's dense node vector.
    outcomes: Col<NodeIdx, NodeOutcome>,
    /// Each node's cross-run lambda state, aligned to the installed program by
    /// [`reconcile`](Self::reconcile).
    pub(super) nodes: Col<NodeIdx, NodeRuntime>,
}

/// Everything one run borrows from the engine. `'r` is the reporter's own lifetime — see
/// [`ExecFrame`].
#[derive(Debug)]
pub(crate) struct RunRequest<'a, 'r> {
    pub(crate) program: &'a CompiledGraph,
    /// The schedule, planned against `program` and then resolved by
    /// [`ExecEngine::execute`](crate::exec::engine); the debug
    /// [`validate`](RunSchedule::validate) after that pass catches a schedule spanning some other
    /// program.
    pub(crate) schedule: &'a RunSchedule,
    pub(crate) cache: &'a mut RuntimeCache,
    /// Live per-node feedback, published ahead of the final outcome.
    pub(crate) reporter: &'a mut (dyn RunReporter + 'r),
    pub(crate) cancel: CancelToken,
}

/// What [`Executor::collect_outcome`] reduces a run against: the program and
/// schedule the loop walked, and the RAM each node was left holding once the
/// engine released what the run left dead.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CollectRequest<'a> {
    pub(crate) program: &'a CompiledGraph,
    pub(crate) schedule: &'a RunSchedule,
    pub(crate) node_ram: &'a Col<NodeIdx, RamUsage>,
}

#[derive(Default, Debug)]
struct RemainingOutputReads {
    counts: Col<OutputIdx, u32>,
}

impl RemainingOutputReads {
    /// Take this run's starting counts from the schedule the loop is about to walk.
    fn seed(&mut self, schedule: &RunSchedule) {
        self.counts.clone_from(&schedule.outputs.readers);
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

    fn node_drained(&self, program: &CompiledGraph, node_idx: NodeIdx) -> bool {
        self.counts[program[node_idx].outputs]
            .iter()
            .all(|remaining| *remaining == 0)
    }
}

impl Executor {
    /// Move the nodes' lambda state onto a newly installed program ([`CompiledGraph::realign`]),
    /// dropping the state of a node whose implementation changed.
    pub(crate) fn reconcile(&mut self, prev: &CompiledGraph, program: &CompiledGraph) {
        program.realign(
            prev,
            &mut self.nodes,
            |node_idx, mut node| {
                node.reown(program[node_idx].func_ref);
                node
            },
            |node_idx| NodeRuntime::new(program[node_idx].func_ref),
        );
    }

    /// Drop every node's lambda state, for an engine left with no program.
    pub(crate) fn clear(&mut self) {
        self.nodes.clear();
    }

    /// Walk `schedule.process_order` (producer-first), giving each node one turn
    /// ([`ExecFrame::run_node`]).
    pub(crate) async fn run(&mut self, request: RunRequest<'_, '_>, outcome: &mut ExecOutcome) {
        let RunRequest {
            program,
            schedule,
            cache,
            reporter,
            cancel,
        } = request;

        outcome.clear();
        // Lets lambdas poll the cancel flag inside off-thread work, and keeps the loop-top and
        // post-loop checks on one source.
        self.ctx_manager.cancel = cancel;
        self.ctx_manager.logs.clear();
        self.outcomes
            .reset(program.e_nodes.len(), NodeOutcome::Pending);
        self.remaining_reads.seed(schedule);

        {
            let mut frame = ExecFrame {
                program,
                schedule,
                cache,
                remaining_reads: &mut self.remaining_reads,
                inputs: &mut self.inputs,
                node_outcomes: &mut self.outcomes,
                nodes: &mut self.nodes,
                ctx: &mut self.ctx_manager,
                reporter,
                outcome,
            };

            // The producer-first schedule excludes unseeded disabled nodes; the
            // resolved run cuts cache-hidden and blocked cones.
            for (process_idx, &node_idx) in schedule.process_order.iter().enumerate() {
                // A schedule of sync-completing lambdas never suspends on its own, and would
                // starve everything sharing its thread (event-loop lambdas, other tasks under a
                // current-thread runtime).
                task::yield_now().await;

                if frame.ctx.cancel.is_cancelled() {
                    frame.retire_cancelled_tail(process_idx);
                    break;
                }
                frame.run_node(node_idx).await;
            }
        }

        self.ctx_manager.cur_node_id = None;
        outcome.logs.append(&mut self.ctx_manager.logs);
        outcome.cancelled = self.ctx_manager.cancel.is_cancelled();
    }

    /// Reduce this run's per-node column and the RAM it left resident into the caller's outcome —
    /// one [`NodeStatus`] row per node, which is also the row the host publishes. A node has no
    /// row when it neither produced a result nor holds a value: the cancelled run's tail, a node
    /// the planner excluded, a pruned cut.
    ///
    /// Separate from [`run`](Self::run) because `node_ram` can only be measured once the engine
    /// has released what the run left dead. It walks the program rather than `process_order`: a
    /// node the run never scheduled can still hold RAM from an earlier one.
    pub(crate) fn collect_outcome(&self, request: CollectRequest<'_>, outcome: &mut ExecOutcome) {
        let CollectRequest {
            program,
            schedule,
            node_ram,
        } = request;
        for (node_idx, node_outcome) in self.outcomes.iter_indexed() {
            let status = match node_outcome {
                // Available and not recomputed: reported cached.
                NodeOutcome::Reused | NodeOutcome::Cut { cached: true } => {
                    Some(NodeExecStatus::Cached)
                }
                NodeOutcome::Ran { secs } => {
                    outcome.executed_node_count += 1;
                    Some(NodeExecStatus::Executed {
                        elapsed_secs: *secs,
                    })
                }
                // A cancelled node didn't complete, so it has neither a run nor a failure of its
                // own — the run as a whole was cancelled. `Skipped` alongside `Failed`: a cancel
                // can land before the invoke, on the path walk that would have keyed it. A node
                // the cut left uncached ran nothing either.
                NodeOutcome::Failed {
                    error: RunError::Cancelled,
                    ..
                }
                | NodeOutcome::Skipped {
                    error: RunError::Cancelled,
                }
                | NodeOutcome::Cut { cached: false } => None,
                // A genuine failure did run, so it counts among the executed.
                NodeOutcome::Failed { error } => {
                    outcome.executed_node_count += 1;
                    Some(NodeExecStatus::Errored {
                        error: error.clone(),
                    })
                }
                NodeOutcome::Skipped { error } => Some(NodeExecStatus::Errored {
                    error: error.clone(),
                }),
                // The planner may have stopped at this node for want of an input — a verdict that
                // isn't the run loop's to give, since a node it never reached holds no outcome.
                NodeOutcome::Pending => missing_inputs(program, schedule, node_idx),
            };
            let ram = node_ram[node_idx];
            if status.is_none() && ram.total() == 0 {
                continue;
            }
            outcome.nodes.push(NodeStatus {
                node_id: program.node_ids[node_idx],
                status,
                ram,
            });
        }
    }
}

/// The unsatisfied input ports of a node the planner verdicted `MissingInputs`, or
/// `None` for any other node.
///
/// Recomputed from the schedule's own predicate rather than stored: only the rare missing node
/// pays for it.
fn missing_inputs(
    program: &CompiledGraph,
    schedule: &RunSchedule,
    node_idx: NodeIdx,
) -> Option<NodeExecStatus> {
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
    Some(NodeExecStatus::MissingInputs { ports })
}

/// One run's live state: the read-only schedule, the cache and per-invoke scratch it mutates, and
/// the sinks it reports through. Bundled so every step is a method taking a node index and the
/// disjoint-field borrows (cache vs. contexts vs. outcomes) are expressed once.
///
/// `'r` is the reporter's own lifetime, kept separate from `'a`: a `&mut dyn Trait` is invariant,
/// so sharing one lifetime would extend every borrow here to the caller's.
#[derive(Debug)]
struct ExecFrame<'a, 'r> {
    program: &'a CompiledGraph,
    schedule: &'a RunSchedule,
    cache: &'a mut RuntimeCache,
    remaining_reads: &'a mut RemainingOutputReads,
    inputs: &'a mut Vec<DynamicValue>,
    /// Per-node results for this run, distinct from the whole-run `outcome` below.
    node_outcomes: &'a mut Col<NodeIdx, NodeOutcome>,
    nodes: &'a mut Col<NodeIdx, NodeRuntime>,
    ctx: &'a mut CtxManager,
    reporter: &'a mut (dyn RunReporter + 'r),
    outcome: &'a mut ExecOutcome,
}

impl ExecFrame<'_, '_> {
    /// One node's turn. The resolved disposition decides which of the four things happens and is
    /// authoritative — a [`NodeState::Reuse`] is never re-derived here, since its producers may be
    /// pruned (see [`resolve::sweep`](crate::exec::resolve::sweep)).
    async fn run_node(&mut self, node_idx: NodeIdx) {
        let e_node = &self.program[node_idx];
        let demand = &self.schedule.outputs.demand[e_node.outputs];
        match self.schedule.states[node_idx] {
            // `process_order` and the state column are written by the same arm of the walk, so
            // a scheduled node without a settled state is a broken plan.
            NodeState::Unvisited => unreachable!("a scheduled node has a settled state"),
            // The planner excluded it: no result this run, outcome stays `Pending`.
            NodeState::Disabled | NodeState::MissingInputs => {}
            // Every consumer that would read this node reused a cache. Report only a current
            // resident value; unneeded disk blobs remain unprobed.
            NodeState::Cut => {
                self.node_outcomes[node_idx] = NodeOutcome::Cut {
                    cached: self.cache.is_resident_cur(node_idx),
                };
            }
            NodeState::Reuse => self.serve_reuse(node_idx, demand).await,
            // Reuse is settled *before* the errored-dependency check inside `invoke_node`: a
            // digest-valid cached value stays valid when an upstream re-ran for another consumer
            // and failed, so it must not be cleared as skipped.
            NodeState::Run => {
                if self.needs_invoke(node_idx, demand).await {
                    self.invoke_node(node_idx, demand).await;
                }
            }
        }
    }

    /// Coarse cancel: stop scheduling further nodes and retire the tail's reads. A node already
    /// mid-invoke isn't interrupted; unreached outcomes stay `Pending` and are omitted.
    fn retire_cancelled_tail(&mut self, from_process_idx: usize) {
        let schedule = self.schedule;
        for &node_idx in &schedule.process_order[from_process_idx..] {
            if schedule.states[node_idx] == NodeState::Run {
                self.abandon_input_reads(node_idx);
            }
        }
    }

    /// Serve a resolved reuse. The sweep only *probed* a disk hit, so the decode happens here at
    /// the node's own turn: producer-first order puts it ahead of every consumer, and the release
    /// below frees it on the same last-read bookkeeping a computed value gets.
    ///
    /// A blob that stopped loading since the probe cannot fall back to running — the cut
    /// already pruned this node's producers — so it fails the node and its consumers skip as
    /// errored-upstream.
    async fn serve_reuse(&mut self, node_idx: NodeIdx, demand: &[OutputDemand]) {
        let program = self.program;
        let hydrated = self.cache.hydrate_reuse(program, node_idx, demand).await;
        match hydrated {
            ReuseOutcome::Missed => {
                self.mark_skipped(node_idx, RunError::CacheLoadFailed);
            }
            ReuseOutcome::Served => {
                self.node_outcomes[node_idx] = NodeOutcome::Reused;
                self.release_drained_outputs(node_idx);
            }
        }
    }

    /// Whether this node's lambda still has to run, after the late second chance at reuse.
    ///
    /// The one verdict the loop *improves*: a `Run` whose stamped digest is `None` because it
    /// folds a Bind-delivered path value the sweep could not read yet (`hash_bound_fs_path`). Its
    /// producers settled earlier in this walk, so re-stamp it now and serve the cache on a hit. A
    /// genuinely uncacheable node (an impure cone) re-folds to `None` and runs as before.
    ///
    /// `true` — nothing was improvable, the improved digest still missed, or a declared path
    /// would not stamp: the node runs, in the last case uncached and with a warning against its
    /// name. `false` — its turn is already settled, whether *served* from cache or stopped by a
    /// cancel; which happened is in the node's outcome.
    ///
    /// Loading *before* retiring this node's input reads lets a failed load fall through to a
    /// normal invoke here, unlike [`serve_reuse`](Self::serve_reuse).
    async fn needs_invoke(&mut self, node_idx: NodeIdx, demand: &[OutputDemand]) -> bool {
        if self.cache[node_idx].cur_digest.is_some() {
            return true;
        }
        let program = self.program;
        let hydrated = self
            .cache
            .restamp_and_hydrate(Restamp {
                program,
                schedule: self.schedule,
                node_idx,
                demand,
                cancel: self.ctx.cancel.clone(),
            })
            .await;
        match hydrated {
            // Torn down: invoking would begin work the cancel exists to stop.
            Err(StampError::Cancelled) => {
                self.mark_skipped(node_idx, RunError::Cancelled);
                false
            }
            // A path that would not read denies this node a *cache key*, not the ability to run:
            // its digest stays `None`, as for every `Impure` node, so it runs uncached. Only the
            // lambda knows what the path was for — one that writes the file it declares succeeds,
            // one that needed to read it fails with its own message. This arm must not stay
            // quiet: a node that asked to be cached and never is, is otherwise invisible.
            Err(error) => {
                self.ctx.log_node(
                    program.node_ids[node_idx],
                    LogLevel::Warn,
                    format!(
                        "running uncached, a declared path could not be identified: {}",
                        ErrorChain::new(&error)
                    ),
                );
                true
            }
            Ok(ReuseOutcome::Missed) => true,
            Ok(ReuseOutcome::Served) => {
                // It came in as `Run`, so the sweep counted it as a reader of each producer;
                // handing those reads back lets a producer's value go the moment its last real
                // reader is gone. A `Reuse` node never owned them, hence `serve_reuse` has no
                // such line.
                self.abandon_input_reads(node_idx);
                self.release_drained_outputs(node_idx);
                self.node_outcomes[node_idx] = NodeOutcome::Reused;
                false
            }
        }
    }

    /// Invoke the node's lambda and record what came of it, persisting a success to disk right
    /// away so a long run's earlier caches survive a later failure or cancel.
    async fn invoke_node(&mut self, node_idx: NodeIdx, demand: &[OutputDemand]) {
        let program = self.program;
        let e_node = &program[node_idx];
        let node_id = program.node_ids[node_idx];

        if self.has_errored_dependency(node_idx) {
            self.abandon_input_reads(node_idx);
            self.mark_skipped(node_idx, RunError::SkippedUpstream);
            return;
        }

        // Release each producer whose last read this satisfies. A disk-reused producer was
        // decoded at its own earlier turn.
        self.collect_inputs(node_idx);

        debug_assert!(matches!(self.node_outcomes[node_idx], NodeOutcome::Pending));

        // Attribute logs this node emits to it (read by `CtxManager::log`).
        self.ctx.cur_node_id = Some(node_id);
        let invoke_start = Instant::now();
        self.reporter
            .progress(node_id, RunPhase::Started { at: invoke_start });

        let result = if let Some(refusal) = self.foreign_path_input(node_idx) {
            Err(refusal)
        } else {
            let node = &mut self.nodes[node_idx];
            e_node
                .lambda
                .invoke(Invocation {
                    ctx: self.ctx,
                    state: &mut node.state,
                    event_state: &node.event_state,
                    inputs: self.inputs,
                    demand,
                    outputs: self.cache[node_idx].invoke_outputs(e_node.outputs.len as usize),
                })
                .await
                .map_err(|e| match e {
                    // A lambda that bailed on cancel reports it truthfully.
                    InvokeError::Cancelled => RunError::Cancelled,
                    other => RunError::Invoke(Arc::new(other)),
                })
        };
        let run_time = invoke_start.elapsed().as_secs_f64();

        // A cancellable lambda reports a cancel itself. Safety net for one that doesn't poll the
        // token (a builtin, a single decode) but ran while the run was cancelled: its `Ok` comes
        // from an aborted run, so map it to `Cancelled` and don't cache it. A genuine error
        // stands on its own, even mid-cancel.
        let result = match result {
            Ok(()) if self.ctx.cancel.is_cancelled() => Err(RunError::Cancelled),
            Ok(()) => match self.cache[node_idx].unbound_demanded_outputs(demand) {
                outputs if outputs.is_empty() => Ok(()),
                outputs => Err(RunError::OutputsNotProduced { outputs }),
            },
            other => other,
        };
        let cancelled = matches!(&result, Err(RunError::Cancelled));
        let slot = &mut self.cache[node_idx];
        let succeeded = match result {
            // The fresh output corresponds to the node's current digest, so the next run's reuse
            // check is a RAM hit.
            Ok(()) => {
                slot.stamp_produced();
                self.node_outcomes[node_idx] = NodeOutcome::Ran { secs: run_time };
                true
            }
            Err(error) => {
                slot.clear_output();
                self.node_outcomes[node_idx] = NodeOutcome::Failed { error };
                false
            }
        };
        // No finish for the cancelled node — it didn't complete.
        if !cancelled {
            let phase = if succeeded {
                RunPhase::Succeeded {
                    elapsed_secs: run_time,
                }
            } else {
                RunPhase::Failed {
                    elapsed_secs: run_time,
                }
            };
            self.reporter.progress(node_id, phase);
        }
        if !succeeded {
            return;
        }

        if self.schedule.root_flags(node_idx).is_event_source() {
            self.collect_event_triggers(node_idx);
        }
        // Persist the moment the node finishes, not at the end of the run. The snapshot is taken
        // synchronously inside `store_node`; only the write awaits, so the cache borrow doesn't
        // cross it. The preceding reuse miss proves no blob can cover this result.
        //
        // The answer is dropped deliberately: nobody is waiting on it, so a blob that did not
        // land degrades into a later recompute; the flush sweeps are where a store's answer
        // reaches a human. The slot still records it, inside the store.
        let _ = self
            .cache
            .store_node(program, node_idx, StorePolicy::KnownMiss)
            .await;
        self.release_drained_outputs(node_idx);
    }

    /// Drop `node_idx` from this run: clear any stale cached output so it isn't served as this
    /// run's result, and record `error` — [`RunError::SkippedUpstream`] for an errored
    /// dependency, or [`RunError::CacheLoadFailed`] for a probed blob that no longer loads.
    fn mark_skipped(&mut self, node_idx: NodeIdx, error: RunError) {
        self.cache[node_idx].clear_output();
        self.node_outcomes[node_idx] = NodeOutcome::Skipped { error };
    }

    /// Whether any node this one binds an input to already failed or was skipped for an error.
    fn has_errored_dependency(&self, node_idx: NodeIdx) -> bool {
        let program = self.program;
        program.inputs[program[node_idx].inputs]
            .iter()
            .any(|input| {
                matches!(&input.binding, ExecBinding::Wire(addr) if self.node_outcomes[addr.node_idx].error().is_some())
            })
    }

    /// Hand the run's outcome the triggers a freshly initialized event source owns — only
    /// events that have a subscriber can fire.
    fn collect_event_triggers(&mut self, node_idx: NodeIdx) {
        let program = self.program;
        let node_id = program.node_ids[node_idx];
        let event_state = &self.nodes[node_idx].event_state;
        self.outcome.event_triggers.extend(
            program.events[program[node_idx].events]
                .iter()
                .enumerate()
                .filter(|(_, event)| !event.subscribers.is_empty())
                .map(|(event_idx, event)| EventTrigger {
                    event: EventPort { node_id, event_idx },
                    lambda: event.lambda.clone(),
                    state: event_state.clone(),
                }),
        );
    }

    /// The first path input of `node_idx` that holds a [`PathForm::Foreign`] path. It reads the
    /// values [`Self::collect_inputs`] resolved, so a const and a wire's value alike pass through.
    fn foreign_path_input(&self, node_idx: NodeIdx) -> Option<RunError> {
        let declared = &self.program.inputs[self.program[node_idx].inputs];
        declared
            .iter()
            .zip(self.inputs.iter())
            .enumerate()
            .filter(|(_, (input, _))| input.fs_path)
            .find_map(|(idx, (_, value))| {
                let path = value
                    .as_fs_paths()?
                    .iter()
                    .find(|path| PathForm::of(path) == PathForm::Foreign)?;
                Some(RunError::ForeignPath {
                    idx,
                    path: path.clone(),
                })
            })
    }

    fn collect_inputs(&mut self, node_idx: NodeIdx) {
        self.inputs.clear();
        for input in &self.program.inputs[self.program[node_idx].inputs] {
            let value = match self.schedule.delivery(input) {
                // No read was planned: a bound input reads as unbound only
                // when the run skips its producer.
                Delivery::Unbound => DynamicValue::Unbound,
                Delivery::Blocked => {
                    unreachable!(
                        "a node with a blocked input is verdicted missing, so it does not run"
                    )
                }
                Delivery::Const(value) => value.into(),
                Delivery::Wire(address) => {
                    let output_idx = self.program.output_idx(address);
                    let read = if self.remaining_reads.is_last(output_idx)
                        && !self.program[address.node_idx].cache.caches_in_ram()
                    {
                        OutputRead::Take
                    } else {
                        OutputRead::Clone
                    };
                    let value = self
                        .cache
                        .read_output_port(self.program, address, read)
                        .expect("a resolved producer output must be resident when consumed");
                    self.complete_planned_read(address);
                    value
                }
            };
            self.inputs.push(value);
        }
    }

    /// Abandons every bound-input read owned by a consumer that will not invoke, so non-RAM
    /// producer values release once their remaining readers disappear.
    fn abandon_input_reads(&mut self, consumer_idx: NodeIdx) {
        for input in &self.program.inputs[self.program[consumer_idx].inputs] {
            if let Delivery::Wire(address) = self.schedule.delivery(input) {
                self.complete_planned_read(address);
            }
        }
    }

    fn release_drained_outputs(&mut self, node_idx: NodeIdx) {
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
    use crate::exec::executor::Executor;
    use crate::exec::idx::OutputIdx;

    impl Executor {
        /// How many planned reads of one output the last run left uncompleted — zero once every
        /// consumer has read it or had its read retired.
        pub(crate) fn remaining_reads(&self, output_idx: OutputIdx) -> u32 {
            self.remaining_reads.counts[output_idx]
        }
    }
}

#[cfg(test)]
mod tests;
