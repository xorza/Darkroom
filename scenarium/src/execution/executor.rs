//! The run loop and its transient state. The `Executor` owns the shared
//! `ctx_manager` and the invoke scratch; the per-node cross-run cache lives in
//! the [`Cache`](crate::execution::cache::Cache). Given an immutable
//! [`ExecutionProgram`](crate::execution::program::ExecutionProgram), a prepared
//! [`ExecutionPlan`](crate::execution::plan::ExecutionPlan), and that `Cache`,
//! [`Executor::run`] invokes each scheduled node's lambda and gathers stats.
//! Per-run results (errors, timings) are columns local to the run, not cache.

use std::time::Instant;

use tokio::sync::mpsc::UnboundedSender;

use common::CancelToken;

use crate::context::ContextManager;
use crate::data::DynamicValue;
use crate::execution_stats::{
    ExecutedNodeStats, ExecutionStats, FlattenMap, NodeError, RunPhase, RunProgress,
};
use crate::func_lambda::{InvokeError, InvokeInput};
use crate::graph::InputPort;

use crate::execution::cache::Cache;
use crate::execution::plan::ExecutionPlan;
use crate::execution::program::{ExecutionBinding, ExecutionProgram};
use crate::execution::{Error, OutputUsage};

#[derive(Default, Debug)]
pub(crate) struct Executor {
    pub(crate) ctx_manager: ContextManager,
    /// Per-node invoke scratch, refilled per executed node.
    inputs: Vec<InvokeInput>,
    output_usage: Vec<OutputUsage>,
    /// Per-run result columns, indexed by `e_node_idx`. Reused across runs and
    /// reset to node count at each run's start.
    errors: Vec<Option<Error>>,
    run_times: Vec<f64>,
}

impl Executor {
    /// Execute every node in `plan.execute_order`, invoking its lambda with the
    /// collected inputs. Mutates the `cache` (output values + digests) and stamps
    /// each executed node's `output_digest` from its `current_digest`. The
    /// `program` and `plan` are read-only. Returns per-run stats.
    pub(crate) async fn run(
        &mut self,
        program: &ExecutionProgram,
        plan: &ExecutionPlan,
        cache: &mut Cache,
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
        errors.clear();
        errors.resize(n_nodes, None);
        run_times.clear();
        run_times.resize(n_nodes, 0.0);

        // How far down `execute_order` the run got. Stays the full length on a
        // normal run; on cancel it's the count reached before bailing, so the
        // stats report only the nodes that actually ran (not the unrun tail).
        let mut executed_count = plan.execute_order.len();
        // The node (if any) that was mid-invoke when the run was cancelled. Its
        // result is untrustworthy (a cancellable lambda bails with Ok + partial
        // output), so it's dropped from the cache and the stats below.
        let mut cancelled_in_flight: Option<usize> = None;
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
                cache.slots[e_node_idx].output_values = None;
                errors[e_node_idx] = Some(Error::Invoke {
                    func_id,
                    message: "Skipped due to upstream error".to_string(),
                });
                continue;
            }

            collect_inputs(program, cache, e_node_idx, &mut inputs);
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
                let slot = &mut cache.slots[e_node_idx];
                let outputs = slot
                    .output_values
                    .get_or_insert_with(|| vec![DynamicValue::Unbound; output_count]);
                lambda
                    .invoke(
                        &mut self.ctx_manager,
                        &mut slot.state,
                        &event_state,
                        &inputs,
                        &output_usage,
                        outputs,
                    )
                    .await
                    .map_err(|e| match e {
                        // A lambda that bailed on cancel reports it truthfully;
                        // surface it as a cancel rather than a generic invoke error.
                        InvokeError::Cancelled => Error::Cancelled { func_id },
                        other => Error::Invoke {
                            func_id,
                            message: other.to_string(),
                        },
                    })
            };

            let run_time = invoke_start.elapsed().as_secs_f64();
            // A cancellable lambda reports a cancel itself (→ `Error::Cancelled`
            // above). This is the safety net for the rest: a lambda that doesn't
            // poll the token (a builtin, a single decode) but ran while the run
            // was cancelled returns `Ok` with a result from an aborted run — map
            // that to `Cancelled` too so its output isn't cached. A genuine
            // error stands on its own, even mid-cancel.
            let result = match result {
                Ok(()) if self.ctx_manager.cancel.is_cancelled() => {
                    Err(Error::Cancelled { func_id })
                }
                other => other,
            };
            let cancelled = matches!(&result, Err(Error::Cancelled { .. }));
            if cancelled {
                cancelled_in_flight = Some(e_node_idx);
            }
            run_times[e_node_idx] = run_time;
            let slot = &mut cache.slots[e_node_idx];
            match result {
                // The fresh output now corresponds to this node's current digest;
                // record it so the planner's next cache check is a RAM hit.
                Ok(()) => slot.output_digest = slot.current_digest,
                Err(err) => {
                    errors[e_node_idx] = Some(err);
                    slot.output_values = None;
                    slot.output_digest = None;
                }
            }
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
        }

        self.ctx_manager.current_node = None;
        let mut stats = collect_execution_stats(
            program,
            plan,
            &errors,
            &run_times,
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
        stats
    }
}

fn has_errored_dependency(
    program: &ExecutionProgram,
    errors: &[Option<Error>],
    e_node_idx: usize,
) -> bool {
    let span = program.e_nodes[e_node_idx].inputs;
    program.inputs[span.range()].iter().any(|input| {
        matches!(&input.binding, ExecutionBinding::Bind(addr) if errors[addr.target_idx].is_some())
    })
}

fn collect_inputs(
    program: &ExecutionProgram,
    cache: &Cache,
    e_node_idx: usize,
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
                    .output_values
                    .as_ref()
                    .expect("missing output values");
                assert_eq!(
                    outputs.len(),
                    program.e_nodes[addr.target_idx].outputs.len as usize
                );
                outputs[addr.port_idx].clone()
            }
        };
        // A node only runs on a cache miss, so its inputs are effectively fresh;
        // `changed` is vestigial (no lambda reads it).
        inputs.push(InvokeInput {
            changed: true,
            value,
        });
    }
}

fn collect_output_usage(
    program: &ExecutionProgram,
    plan: &ExecutionPlan,
    e_node_idx: usize,
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
    errors: &[Option<Error>],
    run_times: &[f64],
    start: Instant,
    executed_count: usize,
    cancelled_in_flight: Option<usize>,
) -> ExecutionStats {
    let mut executed_nodes = Vec::new();
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
        executed_nodes.push(ExecutedNodeStats {
            node_id: e.id,
            elapsed_secs: run_times[idx],
        });
    }

    for &idx in &plan.process_order {
        let e = &program.e_nodes[idx];
        let flags = plan.node_flags[idx];
        if flags.missing_required_inputs {
            // Recompute which ports are unsatisfied (mirrors the planner's
            // per-input check) — only for the rare missing node, so it isn't
            // worth a stored column.
            for (i, pool_idx) in e.inputs.range().enumerate() {
                let input = &program.inputs[pool_idx];
                let missing = match &input.binding {
                    ExecutionBinding::None => input.required,
                    ExecutionBinding::Const(_) => false,
                    ExecutionBinding::Bind(addr) => {
                        plan.node_flags[addr.target_idx].missing_required_inputs
                    }
                };
                if missing {
                    missing_inputs.push(InputPort {
                        node_id: e.id,
                        port_idx: i,
                    });
                }
            }
        }
        if flags.cached {
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
        flatten: FlattenMap::default(),
        // Set by `run` from the cancel flag after the loop.
        cancelled: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::async_lambda;
    use crate::data::{DataType, StaticValue};
    use crate::execution::cache::Cache;
    use crate::execution::plan::NodeFlags;
    use crate::execution::program::{ExecutionInput, ExecutionNode, ExecutionPortAddress};
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
                    data_type: DataType::Null,
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
            target_id: NodeId::from_u128(idx as u128 + 1),
            target_idx: idx,
            port_idx: port,
        })
    }

    /// A plan that runs every node in index order, each output marked needed.
    fn straight_plan(program: &ExecutionProgram) -> ExecutionPlan {
        let n = program.e_nodes.len();
        let mut plan = ExecutionPlan::default();
        plan.process_order = (0..n).collect();
        plan.execute_order = (0..n).collect();
        plan.node_flags = vec![
            NodeFlags {
                wants_execute: true,
                cached: false,
                missing_required_inputs: false,
            };
            n
        ];
        plan.output_usage = vec![1; program.n_outputs];
        plan
    }

    async fn run(program: &ExecutionProgram, plan: &ExecutionPlan) -> (Cache, ExecutionStats) {
        let mut cache = Cache::default();
        cache.reconcile(&program.e_nodes);
        let mut executor = Executor::default();
        let stats = executor
            .run(
                program,
                plan,
                &mut cache,
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
            cache.slots[a].output_values.as_ref().unwrap()[0].as_i64(),
            Some(7),
            "producer wrote 7"
        );
        assert_eq!(
            cache.slots[b].output_values.as_ref().unwrap()[0].as_i64(),
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
            cache.slots[a].output_values.is_none(),
            "an errored node's output is dropped (so it re-runs)"
        );
        assert!(
            cache.slots[b].output_values.is_none(),
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
        assert!(error_of(b).unwrap().contains("upstream error"));
    }
}
