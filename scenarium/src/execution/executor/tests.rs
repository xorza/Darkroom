use super::*;
use crate::async_lambda;
use crate::data::{DataType, StaticValue};
use crate::execution::cache::Cache;
use crate::execution::plan::NodeVerdict;
use crate::execution::program::{ExecutionInput, ExecutionNode, ExecutionPortAddress, NodeIdx};
use crate::graph::NodeId;
use crate::node::func_lambda::FuncLambda;
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
        let outputs_start = self.program.output_types.len() as u32;
        self.program
            .output_types
            .resize(outputs_start as usize + outputs as usize, DataType::Null);
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

/// A plan that runs every node in index order, each output marked needed. These tests
/// drive the run loop directly with an all-`needed` mask (the reuse/cut logic is
/// unit-tested in `resolve.rs`), so `roots` is irrelevant here.
fn straight_plan(program: &ExecutionProgram) -> ExecutionPlan {
    let n = program.e_nodes.len();
    ExecutionPlan {
        process_order: (0..n).map(NodeIdx::from).collect(),
        verdicts: vec![NodeVerdict::Execute; n].into(),
        output_usage: vec![1; program.n_outputs()],
        roots: (0..n).map(NodeIdx::from).collect(),
    }
}

async fn run(program: &ExecutionProgram, plan: &ExecutionPlan) -> (Cache, ExecutionStats) {
    let mut cache = Cache::default();
    cache.reconcile(&program.e_nodes);
    let output_cache = OutputCache::default();
    let mut executor = Executor::default();
    // Every node needed — drive the run loop directly, not the cut.
    let needed: NodeColumn<bool> = vec![true; program.e_nodes.len()].into();
    let stats = executor
        .run(
            program,
            plan,
            &needed,
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
