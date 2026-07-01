//! The run loop and its transient state. The `Executor` owns the shared
//! `ctx_manager` and the invoke scratch; the per-node cross-run cache lives in
//! the [`Cache`](crate::execution::cache::Cache). Given an immutable
//! [`ExecutionProgram`](crate::execution::program::ExecutionProgram), a prepared
//! [`ExecutionPlan`](crate::execution::plan::ExecutionPlan), and that `Cache`,
//! [`Executor::run`] invokes each scheduled node's lambda and gathers stats.
//! Per-run results (errors, timings) are columns local to the run, not cache.

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
use crate::function::FuncBehavior;
use crate::graph::InputPort;

use crate::execution::RunError;
use crate::execution::cache::Cache;
use crate::execution::output_cache::OutputCache;
use crate::execution::plan::{ExecutionPlan, input_missing};
use crate::execution::program::{ExecutionBinding, ExecutionProgram, NodeColumn, NodeIdx};

/// What became of a node this run — the runtime early-cutoff state, set as each node
/// settles. Merges the two mutually-exclusive facts a node carries: `Ran` (it
/// recomputed — the "dirty" signal a consumer's pre-check reads to know a producer
/// changed) and `Reused` (its pre-check reported unchanged, so its prior output was
/// served — counted as cached, not executed). `Pending` (default) is a node not
/// reached this run: cached, or below a cancel/error. "Both dirty and reused" can't
/// be built.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum NodeRun {
    #[default]
    Pending,
    Ran,
    Reused,
}

impl NodeRun {
    /// The node recomputed — its consumers must treat its output as changed.
    fn is_dirty(self) -> bool {
        matches!(self, NodeRun::Ran)
    }
    /// The node reused its prior output (pre-check unchanged).
    fn is_reused(self) -> bool {
        matches!(self, NodeRun::Reused)
    }
}

#[derive(Default, Debug)]
pub(crate) struct Executor {
    pub(crate) ctx_manager: ContextManager,
    /// Per-node invoke scratch, refilled per executed node.
    inputs: Vec<InvokeInput>,
    output_usage: Vec<OutputUsage>,
    /// Per-run result columns, indexed by `e_node_idx`. Reused across runs and
    /// reset to node count at each run's start.
    errors: NodeColumn<Option<RunError>>,
    run_times: NodeColumn<f64>,
    /// Per-run early-cutoff outcome per node (see [`NodeRun`]): a producer's `Ran` is
    /// the "dirty" signal a consumer's pre-check reads; `Reused` makes stats count the
    /// node as cached.
    outcomes: NodeColumn<NodeRun>,
}

impl Executor {
    /// Execute every node in `plan.execute_order`, invoking its lambda with the
    /// collected inputs. Mutates the `cache` (output values + digests) and stamps
    /// each executed node's `output_digest` from its `current_digest`, then persists
    /// it to `output_cache` right away (so a long run's earlier caches survive a later
    /// failure or cancel). The `program` and `plan` are read-only. Returns per-run stats.
    #[allow(clippy::too_many_arguments)] // an orchestration entry point; each arg is a distinct collaborator
    pub(crate) async fn run(
        &mut self,
        program: &ExecutionProgram,
        plan: &ExecutionPlan,
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
        let mut output_usage = std::mem::take(&mut self.output_usage);
        let mut errors = std::mem::take(&mut self.errors);
        let mut run_times = std::mem::take(&mut self.run_times);
        let mut outcomes = std::mem::take(&mut self.outcomes);
        errors.reset(n_nodes, None);
        run_times.reset(n_nodes, 0.0);
        outcomes.reset(n_nodes, NodeRun::Pending);

        // How far down `execute_order` the run got. Stays the full length on a
        // normal run; on cancel it's the count reached before bailing, so the
        // stats report only the nodes that actually ran (not the unrun tail).
        let mut executed_count = plan.execute_order.len();
        // The node (if any) that was mid-invoke when the run was cancelled. Its
        // result is untrustworthy (a cancellable lambda bails with Ok + partial
        // output), so it's dropped from the cache and the stats below.
        let mut cancelled_in_flight: Option<NodeIdx> = None;

        for (pos, e_node_idx) in plan.execute_order.iter().copied().enumerate() {
            // Coarse cancel: stop scheduling further nodes. A node already
            // mid-invoke isn't interrupted (it finishes), but nothing after
            // it starts. The unrun nodes simply don't appear in the stats.
            if self.ctx_manager.cancel.is_cancelled() {
                executed_count = pos;
                break;
            }
            if program.e_nodes[e_node_idx].lambda.is_none() {
                continue;
            }

            let func_id = program.e_nodes[e_node_idx].func_id;

            if has_errored_dependency(program, &errors, e_node_idx) {
                cache.slots[e_node_idx].clear_output();
                errors[e_node_idx] = Some(RunError::SkippedUpstream { func_id });
                continue;
            }

            collect_inputs(program, cache, e_node_idx, &mut inputs);

            // Runtime early-cutoff. A node reuses its prior output — skipping its
            // lambda and staying "clean" (`dirty` unset) so its own consumers can skip
            // in turn — when its inputs are wholly unchanged: every Bind-producer stayed
            // clean this run (topological order ⇒ all decided), AND its own local inputs
            // match, AND it can vouch its output is then unchanged (a `Pure` node by
            // determinism, or any node with a pre-check). Requires a prior output.
            //
            // A pre-check contributes a precise, execution-time content digest of what
            // the func actually reads; it overrides the coarse plan-time local digest,
            // so both the reuse comparison and the produced stamp key on it — an
            // irrelevant file its fingerprint ignores can't block reuse — and, being a
            // content key, it's what a disk cache would key on.
            let e_node = &program.e_nodes[e_node_idx];
            let has_pre_check = if let Some(hash) = e_node
                .pre_check
                .check(&mut cache.slots[e_node_idx].state, &inputs)
            {
                cache.slots[e_node_idx].current_local = hash;
                true
            } else {
                false
            };
            let upstream_dirty = program.inputs[e_node.inputs.range()].iter().any(|input| {
                matches!(&input.binding, ExecutionBinding::Bind(addr) if outcomes[addr.target_idx].is_dirty())
            });
            let can_reuse = cache.slots[e_node_idx].output_values().is_some()
                && !upstream_dirty
                && cache.slots[e_node_idx].local_unchanged();
            let unchanged = can_reuse && (has_pre_check || e_node.behavior == FuncBehavior::Pure);
            if unchanged {
                outcomes[e_node_idx] = NodeRun::Reused;
                continue;
            }
            outcomes[e_node_idx] = NodeRun::Ran;

            collect_output_usage(program, plan, e_node_idx, &mut output_usage);

            let output_count = program.e_nodes[e_node_idx].outputs.len as usize;
            let event_state = cache.slots[e_node_idx].event_state.clone();
            assert!(errors[e_node_idx].is_none());

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
                        &output_usage,
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
            run_times[e_node_idx] = run_time;
            let slot = &mut cache.slots[e_node_idx];
            let succeeded = match result {
                // The fresh output now corresponds to this node's current digest;
                // record it so the planner's next cache check is a RAM hit.
                Ok(()) => {
                    slot.stamp_produced();
                    true
                }
                Err(err) => {
                    errors[e_node_idx] = Some(err);
                    slot.clear_output();
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
            &errors,
            &run_times,
            &outcomes,
            start,
            executed_count,
            cancelled_in_flight,
        );
        stats.logs = std::mem::take(&mut self.ctx_manager.logs);
        stats.cancelled = self.ctx_manager.cancel.is_cancelled();
        self.inputs = inputs;
        self.output_usage = output_usage;
        self.errors = errors;
        self.run_times = run_times;
        self.outcomes = outcomes;
        stats
    }
}

fn has_errored_dependency(
    program: &ExecutionProgram,
    errors: &NodeColumn<Option<RunError>>,
    e_node_idx: NodeIdx,
) -> bool {
    let span = program.e_nodes[e_node_idx].inputs;
    program.inputs[span.range()].iter().any(|input| {
        matches!(&input.binding, ExecutionBinding::Bind(addr) if errors[addr.target_idx].is_some())
    })
}

fn collect_inputs(
    program: &ExecutionProgram,
    cache: &Cache,
    e_node_idx: NodeIdx,
    inputs: &mut Vec<InvokeInput>,
) {
    inputs.clear();
    let span = program.e_nodes[e_node_idx].inputs;
    for input in &program.inputs[span.range()] {
        let value = match &input.binding {
            ExecutionBinding::None => DynamicValue::Unbound,
            ExecutionBinding::Const(v) => v.into(),
            ExecutionBinding::Bind(addr) => {
                // Invariant: any node in `execute_order` has every bound upstream
                // producing output — the planner gates a consumer
                // (`missing_required_inputs`) whenever a bind target can't run,
                // *including* optional binds, so a `None` here is a planner bug,
                // not a graph the user can author.
                let outputs = cache.slots[addr.target_idx]
                    .output_values()
                    .expect("missing output values");
                assert_eq!(
                    outputs.len(),
                    program.e_nodes[addr.target_idx].outputs.len as usize
                );
                outputs[addr.port_idx].clone()
            }
        };
        inputs.push(InvokeInput { value });
    }
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

#[allow(clippy::too_many_arguments)] // stats projection over several distinct per-run columns
fn collect_execution_stats(
    program: &ExecutionProgram,
    plan: &ExecutionPlan,
    errors: &NodeColumn<Option<RunError>>,
    run_times: &NodeColumn<f64>,
    outcomes: &NodeColumn<NodeRun>,
    start: Instant,
    executed_count: usize,
    cancelled_in_flight: Option<NodeIdx>,
) -> ExecutionStats {
    let mut executed_nodes = Vec::with_capacity(executed_count);
    let mut missing_inputs = Vec::new();
    let mut cached_nodes = Vec::new();
    let mut node_errors = Vec::new();

    for &idx in &plan.execute_order[..executed_count] {
        // The node interrupted mid-invoke by a cancel didn't complete — omit it
        // so the consumer doesn't paint it as executed.
        if Some(idx) == cancelled_in_flight {
            continue;
        }
        let e = &program.e_nodes[idx];
        // A node whose pre-check reported unchanged reused its prior output rather
        // than running, so it reads as cached, not executed.
        if outcomes[idx].is_reused() {
            cached_nodes.push(e.id);
            continue;
        }
        executed_nodes.push(ExecutedNodeStats {
            node_id: e.id,
            elapsed_secs: run_times[idx],
        });
    }

    for &idx in &plan.process_order {
        let e = &program.e_nodes[idx];
        let verdict = plan.verdicts[idx];
        if verdict.missing_required_inputs() {
            // Recompute which ports are unsatisfied (shares `input_missing` with
            // the planner) — only for the rare missing node, so it isn't worth a
            // stored column.
            for (i, pool_idx) in e.inputs.range().enumerate() {
                if input_missing(&program.inputs[pool_idx], &plan.verdicts) {
                    missing_inputs.push(InputPort::new(e.id, i));
                }
            }
        }
        if verdict.is_cached() {
            cached_nodes.push(e.id);
        }
        if let Some(err) = &errors[idx] {
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
mod tests {
    use super::*;
    use crate::async_lambda;
    use crate::data::StaticValue;
    use crate::execution::cache::Cache;
    use crate::execution::plan::NodeVerdict;
    use crate::execution::program::{ExecutionInput, ExecutionNode, ExecutionPortAddress, NodeIdx};
    use crate::func_lambda::FuncLambda;
    use crate::graph::NodeId;
    use crate::prelude::FuncId;
    use common::Span;

    /// Hand-built program with real lambdas. Node `idx` gets id `from_u128(idx+1)`,
    /// so `bind` lines up. Inputs are all optional here (the planner gates required
    /// ones; these tests drive the executor directly).
    #[derive(Default)]
    struct Prog {
        program: ExecutionProgram,
    }

    impl Prog {
        fn node(&mut self, inputs: &[ExecutionBinding], outputs: u32, lambda: FuncLambda) -> usize {
            let inputs_start = self.program.inputs.len() as u32;
            for binding in inputs {
                self.program.inputs.push(ExecutionInput {
                    required: false,
                    binding: binding.clone(),
                });
            }
            let outputs_start = self.program.n_outputs as u32;
            self.program.n_outputs += outputs as usize;
            let idx = self.program.e_nodes.len();
            self.program.e_nodes.add(ExecutionNode {
                id: NodeId::from_u128(idx as u128 + 1),
                inited: true,
                func_id: FuncId::from_u128(idx as u128 + 1),
                inputs: Span::new(inputs_start, inputs.len() as u32),
                outputs: Span::new(outputs_start, outputs),
                lambda,
                ..Default::default()
            });
            idx
        }
    }

    fn bind(idx: usize, port: usize) -> ExecutionBinding {
        ExecutionBinding::Bind(ExecutionPortAddress {
            target_idx: idx.into(),
            port_idx: port,
        })
    }

    /// A plan that runs every node in index order, each output marked needed.
    fn straight_plan(program: &ExecutionProgram) -> ExecutionPlan {
        let n = program.e_nodes.len();
        ExecutionPlan {
            process_order: (0..n).map(NodeIdx::from).collect(),
            execute_order: (0..n).map(NodeIdx::from).collect(),
            verdicts: vec![NodeVerdict::Execute; n].into(),
            output_usage: vec![1; program.n_outputs],
        }
    }

    async fn run(program: &ExecutionProgram, plan: &ExecutionPlan) -> (Cache, ExecutionStats) {
        let mut cache = Cache::default();
        cache.reconcile(&program.e_nodes);
        let output_cache = OutputCache::default();
        let mut executor = Executor::default();
        let stats = executor
            .run(
                program,
                plan,
                &mut cache,
                &output_cache,
                &FlattenMap::default(),
                None,
                CancelToken::never(),
            )
            .await;
        (cache, stats)
    }

    #[tokio::test]
    async fn runs_in_order_resolving_binds_and_storing_outputs() {
        let mut p = Prog::default();
        let producer = async_lambda!(|_ctx, _state, _ev, _inputs, _usage, outputs| {
            outputs[0] = DynamicValue::Static(StaticValue::Int(7));
            Ok(())
        });
        let consumer = async_lambda!(|_ctx, _state, _ev, inputs, _usage, outputs| {
            let v = inputs[0].value.as_i64().unwrap();
            outputs[0] = DynamicValue::Static(StaticValue::Int(v + 1));
            Ok(())
        });
        let a = p.node(&[], 1, producer);
        let b = p.node(&[bind(a, 0)], 1, consumer);

        let plan = straight_plan(&p.program);
        let (cache, stats) = run(&p.program, &plan).await;

        assert_eq!(
            cache.slots[a].output_values().unwrap()[0].as_i64(),
            Some(7),
            "producer wrote 7"
        );
        assert_eq!(
            cache.slots[b].output_values().unwrap()[0].as_i64(),
            Some(8),
            "consumer read 7 and wrote 7+1"
        );
        assert_eq!(stats.executed_nodes.len(), 2);
        assert!(stats.node_errors.is_empty());
    }

    #[tokio::test]
    async fn upstream_error_skips_dependents_and_clears_output() {
        let mut p = Prog::default();
        let failing = async_lambda!(|_ctx, _state, _ev, _inputs, _usage, _outputs| {
            Err(anyhow::anyhow!("boom").into())
        });
        let downstream = async_lambda!(|_ctx, _state, _ev, _inputs, _usage, outputs| {
            outputs[0] = DynamicValue::Static(StaticValue::Int(1));
            Ok(())
        });
        let a = p.node(&[], 1, failing);
        let b = p.node(&[bind(a, 0)], 1, downstream);

        let plan = straight_plan(&p.program);
        let (cache, stats) = run(&p.program, &plan).await;

        assert!(
            cache.slots[a].output_values().is_none(),
            "an errored node's output is dropped (so it re-runs)"
        );
        assert!(
            cache.slots[b].output_values().is_none(),
            "the dependent is skipped, producing nothing"
        );
        let error_of = |idx: usize| {
            stats
                .node_errors
                .iter()
                .find(|e| e.node_id == NodeId::from_u128(idx as u128 + 1))
                .map(|e| e.error.to_string())
        };
        assert!(error_of(a).unwrap().contains("boom"));
        assert!(error_of(b).unwrap().contains("upstream"));
    }
}
