use super::*;
use crate::async_lambda;
use crate::data::{DataType, StaticValue};
use crate::execution::cache::{RuntimeCache, ValueState};
use crate::execution::plan::NodeVerdict;
use crate::execution::program::{ExecutionInput, ExecutionNode, ExecutionPortAddress, NodeIdx};
use crate::graph::CacheMode;
use crate::graph::NodeId;
use crate::node::func_lambda::FuncLambda;
use crate::node::function::FuncId;
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
            .resize(outputs_start as usize + outputs as usize, DataType::Any);
        let idx = self.program.e_nodes.len();
        self.program.e_nodes.add(ExecutionNode {
            id: NodeId::from_u128(idx as u128 + 1),
            inited: true,
            func_id: FuncId::from_u128(idx as u128 + 1),
            inputs: Span::new(inputs_start, inputs.len() as u32),
            outputs: Span::new(outputs_start, outputs),
            lambda,
            // `CacheMode` now defaults to `None`; these tests assume outputs are
            // retained (`Ram`) unless a case flips it via `set_cache`.
            cache: CacheMode::Ram,
            ..Default::default()
        });
        idx
    }

    /// Override a node's [`CacheMode`] (nodes default to `Ram`). Drives the mid-run
    /// output-release tests, which turn on the non-RAM modes.
    fn set_cache(&mut self, idx: usize, cache: CacheMode) {
        self.program.e_nodes[idx].cache = cache;
    }
}

/// A `straight_plan` with an explicit per-output consumer count (indexed by output-pool
/// index, so its length is `n_outputs`), instead of the all-`1` default. Lets a test claim
/// more consumers than actually read (to prove the release waits for the full count) or none
/// (a terminal sink, released the instant it runs).
fn plan_with_usage(program: &ExecutionProgram, output_usage: Vec<u32>) -> ExecutionPlan {
    assert_eq!(output_usage.len(), program.n_outputs());
    ExecutionPlan {
        output_usage,
        ..straight_plan(program)
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

async fn run(program: &ExecutionProgram, plan: &ExecutionPlan) -> (RuntimeCache, ExecutionStats) {
    // `RuntimeCache::default()` has a memory-only `DiskStore`, so no disk cache is in play.
    let mut cache = RuntimeCache::default();
    cache.reconcile(&program.e_nodes);
    let mut executor = Executor::default();
    // Every node needed — drive the run loop directly, not the cut.
    let needed: NodeColumn<bool> = vec![true; program.e_nodes.len()].into();
    let stats = executor
        .run(
            program,
            plan,
            &needed,
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

/// A `None`-cache producer's RAM output is dropped the moment its last consumer reads it.
/// `Executor::run` does no end-of-run eviction, so an emptied slot here is the *mid-run*
/// release and nothing else. A(None) → B(Ram): once B has read A, A is `Empty` while B keeps
/// its own value.
#[tokio::test]
async fn frees_none_cache_output_once_last_consumer_reads() {
    let mut p = Prog::default();
    let producer = async_lambda!(|_ctx, _s, _ev, _inputs, _usage, outputs| {
        outputs[0] = DynamicValue::Static(StaticValue::Int(7));
        Ok(())
    });
    let consumer = async_lambda!(|_ctx, _s, _ev, inputs, _usage, outputs| {
        let v = inputs[0].value.as_i64().unwrap();
        outputs[0] = DynamicValue::Static(StaticValue::Int(v + 1));
        Ok(())
    });
    let a = p.node(&[], 1, producer);
    let b = p.node(&[bind(a, 0)], 1, consumer);
    p.set_cache(a, CacheMode::None);
    p.set_cache(b, CacheMode::Ram);

    // A's one output has one consumer (B); B's output has a phantom consumer, so B never drains.
    let plan = plan_with_usage(&p.program, vec![1, 1]);
    let (cache, _stats) = run(&p.program, &plan).await;

    assert!(
        matches!(cache.slots[a].value, ValueState::Empty),
        "A (None) is freed the moment its last consumer B reads it: {:?}",
        cache.slots[a].value
    );
    assert_eq!(
        cache.slots[b].output_values().unwrap()[0].as_i64(),
        Some(8),
        "B (Ram) keeps its own output (7+1)"
    );
}

/// The release only reclaims modes that don't retain RAM: a `Ram` producer stays resident
/// even after every consumer has read it. A(Ram) → B — A survives B's read.
#[tokio::test]
async fn keeps_ram_cache_output_after_all_consumers_read() {
    let mut p = Prog::default();
    let producer = async_lambda!(|_ctx, _s, _ev, _inputs, _usage, outputs| {
        outputs[0] = DynamicValue::Static(StaticValue::Int(7));
        Ok(())
    });
    let consumer = async_lambda!(|_ctx, _s, _ev, inputs, _usage, outputs| {
        outputs[0] = DynamicValue::Static(StaticValue::Int(inputs[0].value.as_i64().unwrap()));
        Ok(())
    });
    let a = p.node(&[], 1, producer);
    let b = p.node(&[bind(a, 0)], 1, consumer);
    p.set_cache(a, CacheMode::Ram);
    p.set_cache(b, CacheMode::Ram);

    // A has one consumer (B, which reads it) and B has none (usage 0).
    let plan = plan_with_usage(&p.program, vec![1, 0]);
    let (cache, _stats) = run(&p.program, &plan).await;

    assert_eq!(
        cache.slots[a].output_values().unwrap()[0].as_i64(),
        Some(7),
        "A (Ram) is kept hot for the next run even though B has fully drained it"
    );
}

/// The release waits for *every* counted consumer, not the first read. A(None) is claimed to
/// have two consumers but only B actually reads it: the count never reaches zero, so the
/// value is held (end-of-run eviction, not modeled by `Executor::run`, would reclaim it). This
/// is the safety margin — a consumer that never runs can't cause a premature free.
#[tokio::test]
async fn holds_output_until_every_counted_consumer_reads() {
    let mut p = Prog::default();
    let producer = async_lambda!(|_ctx, _s, _ev, _inputs, _usage, outputs| {
        outputs[0] = DynamicValue::Static(StaticValue::Int(7));
        Ok(())
    });
    let consumer = async_lambda!(|_ctx, _s, _ev, inputs, _usage, outputs| {
        outputs[0] = DynamicValue::Static(StaticValue::Int(inputs[0].value.as_i64().unwrap()));
        Ok(())
    });
    let a = p.node(&[], 1, producer);
    let _b = p.node(&[bind(a, 0)], 1, consumer);
    p.set_cache(a, CacheMode::None);

    // Claim A has TWO consumers though only B reads it: its count settles at 1, never 0.
    let plan = plan_with_usage(&p.program, vec![2, 0]);
    let (cache, _stats) = run(&p.program, &plan).await;

    assert_eq!(
        cache.slots[a].output_values().map(|v| v[0].as_i64()),
        Some(Some(7)),
        "A (None) is still resident: one of its two counted reads never happened"
    );
}

/// A node no one consumes (a terminal sink, usage 0) is released the instant it finishes,
/// not held to end-of-run: a `None` output is dropped, a `Ram` output kept hot.
#[tokio::test]
async fn frees_zero_consumer_output_right_after_it_runs() {
    let mut p = Prog::default();
    let producer = || {
        async_lambda!(|_ctx, _s, _ev, _inputs, _usage, outputs| {
            outputs[0] = DynamicValue::Static(StaticValue::Int(7));
            Ok(())
        })
    };
    let a = p.node(&[], 1, producer());
    let b = p.node(&[], 1, producer());
    p.set_cache(a, CacheMode::None);
    p.set_cache(b, CacheMode::Ram);

    // Neither output is consumed.
    let plan = plan_with_usage(&p.program, vec![0, 0]);
    let (cache, _stats) = run(&p.program, &plan).await;

    assert!(
        matches!(cache.slots[a].value, ValueState::Empty),
        "A (None, no consumers) is freed right after it runs: {:?}",
        cache.slots[a].value
    );
    assert_eq!(
        cache.slots[b].output_values().unwrap()[0].as_i64(),
        Some(7),
        "B (Ram, no consumers) is kept hot"
    );
}
