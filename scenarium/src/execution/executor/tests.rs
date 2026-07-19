use std::sync::Arc;

use tokio::sync::mpsc;

use super::*;
use crate::async_lambda;
use crate::execution::cache::{OutputSnapshot, RuntimeCache, ValueState};
use crate::execution::plan::NodeVerdict;
use crate::execution::program::{ExecutionInput, ExecutionNode, ExecutionPortAddress, OutputIdx};
use crate::execution::resolve::{Disposition, ResolvedOutputs, ResolvedRun, Resolver};
use crate::execution::resource::RunResourceStamps;
use crate::graph::CacheMode;
use crate::graph::NodeId;
use crate::node::definition::{FuncBehavior, FuncId};
use crate::node::lambda::{FuncLambda, OutputDemand};
use crate::{DataType, StaticValue};
use common::Span;

/// Hand-built program with real lambdas. Node `idx` gets id `from_u128(idx+1)`,
/// so `bind` lines up. Inputs are all optional here (the planner gates required
/// ones; these tests drive the executor directly).
#[derive(Default)]
struct Prog {
    program: ExecutionProgram,
}

impl Prog {
    fn node(&mut self, inputs: &[ExecutionBinding], outputs: u32, lambda: FuncLambda) -> NodeId {
        let inputs_start = self.program.inputs.len() as u32;
        for binding in inputs {
            self.program.inputs.push(ExecutionInput {
                required: false,
                stamper: None,
                binding: binding.clone(),
            });
        }
        let outputs_start = self.program.output_types.len() as u32;
        self.program
            .output_types
            .resize(outputs_start as usize + outputs as usize, DataType::Any);
        self.program
            .output_pinned
            .resize(outputs_start as usize + outputs as usize, false);
        let idx = self.program.e_nodes.len();
        let node_id = NodeId::from_u128(idx as u128 + 1);
        self.program.e_nodes.insert(
            node_id,
            ExecutionNode {
                func_id: FuncId::from_u128(idx as u128 + 1),
                inputs: Span::new(inputs_start, inputs.len() as u32),
                outputs: Span::new(outputs_start, outputs),
                lambda,
                // `CacheMode` now defaults to `None`; these tests assume outputs are
                // retained (`Ram`) unless a case flips it via `set_cache`.
                cache: CacheMode::Ram,
                ..Default::default()
            },
        );
        node_id
    }

    /// Override a node's [`CacheMode`] (nodes default to `Ram`). Drives the mid-run
    /// output-release tests, which turn on the non-RAM modes.
    fn set_cache(&mut self, node_id: NodeId, cache: CacheMode) {
        self.program.e_nodes.get_mut(&node_id).unwrap().cache = cache;
    }

    /// Flip node `idx`'s output `port`'s pinned flag (both default `false`).
    fn set_output_pinned(&mut self, node_id: NodeId, port: usize, pinned: bool) {
        let start = self.program.e_nodes[&node_id].outputs.start as usize;
        self.program.output_pinned[start + port] = pinned;
    }
}

/// A `FlattenMap` where every node in `program` maps to itself as a top-level
/// leaf — mirrors how the real flattener leaves top-level ids unchanged, so
/// `flatten.interior(id) == Some(id)` for every node. `Prog` fixtures bypass
/// the flattener entirely; this is the minimal stand-in the pinned-output
/// push needs (it looks up `interior` only, never the composite-instance
/// attribution chain `RunProgress` uses).
fn self_mapped_flatten(program: &ExecutionProgram) -> FlattenMap {
    let mut flatten = FlattenMap::default();
    // `reset` seeds the root scope (index 0) that every leaf below points at —
    // without it `scopes` stays empty and `attribution`'s walk (used for the
    // `RunProgress` Started/Finished sends, not just the pinned-output push)
    // indexes out of bounds.
    flatten.reset();
    for node_id in program.e_nodes.keys().copied() {
        flatten.set_leaf(node_id, 0, node_id);
    }
    flatten
}

#[derive(Debug)]
struct TestRun {
    plan: ExecutionPlan,
    resolved: ResolvedRun,
}

/// A `straight_run` with an explicit per-output consumer count (indexed by output-pool
/// index, so its length is `n_outputs`), instead of the all-`1` default. Lets a test claim
/// more consumers than actually read (to prove the release waits for the full count) or none
/// (a sink, released the instant it runs).
fn run_with_readers(program: &ExecutionProgram, readers: Vec<u32>) -> TestRun {
    assert_eq!(readers.len(), program.n_outputs());
    let demand: Vec<OutputDemand> = readers
        .iter()
        .map(|count| {
            if *count == 0 {
                OutputDemand::Skip
            } else {
                OutputDemand::Produce
            }
        })
        .collect();
    TestRun {
        plan: structural_plan(program),
        resolved: ResolvedRun {
            disposition: program
                .e_nodes
                .keys()
                .copied()
                .map(|node_id| (node_id, Disposition::Run))
                .collect(),
            outputs: ResolvedOutputs {
                demand: OutputColumn::from(demand),
                readers: OutputColumn::from(readers),
            },
        },
    }
}

fn demand_output(program: &ExecutionProgram, run: &mut TestRun, node_id: NodeId, port_idx: usize) {
    let output_idx = program.output_idx(node_id, port_idx);
    run.resolved.outputs.demand[output_idx] = OutputDemand::Produce;
}

fn bind(node_id: NodeId, port: usize) -> ExecutionBinding {
    ExecutionBinding::Bind(ExecutionPortAddress {
        target: node_id,
        port_idx: port,
    })
}

/// A resolved run that runs every node in index order, each output marked needed. These tests
/// drive the run loop directly with an all-`needed` mask (the reuse/cut logic is
/// unit-tested in `resolve.rs`), so `roots` is irrelevant here.
fn straight_run(program: &ExecutionProgram) -> TestRun {
    run_with_readers(program, vec![1; program.n_outputs()])
}

fn structural_plan(program: &ExecutionProgram) -> ExecutionPlan {
    let process_order: Vec<_> = (0..program.e_nodes.len())
        .map(|idx| NodeId::from_u128(idx as u128 + 1))
        .collect();
    let verdicts = process_order
        .iter()
        .copied()
        .map(|node_id| (node_id, NodeVerdict::Execute))
        .collect();
    ExecutionPlan {
        process_order: process_order.clone(),
        verdicts,
        roots: process_order.into_iter().collect(),
        pinned: NodeSet::new(),
    }
}

#[test]
#[cfg(debug_assertions)]
fn debug_assertions_reject_invalid_output_indexes_and_reader_counts() {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    let mut p = Prog::default();
    let node_id = p.node(&[], 1, FuncLambda::default());
    assert!(
        catch_unwind(AssertUnwindSafe(|| p.program.output_idx(node_id, 1))).is_err(),
        "a node-local output outside its compiled span must trip in debug"
    );

    if let Ok(index) = usize::try_from(u64::from(u32::MAX) + 1) {
        assert!(
            catch_unwind(|| OutputIdx::from(index)).is_err(),
            "the output pool cannot exceed its u32 index representation"
        );
    }

    let mut reads = RemainingOutputReads {
        counts: OutputColumn::from(vec![0]),
    };
    assert!(
        catch_unwind(AssertUnwindSafe(|| reads.consume(OutputIdx(0)))).is_err(),
        "a reader count cannot be consumed below zero"
    );
}

async fn run(program: &ExecutionProgram, run: &TestRun) -> (RuntimeCache, ExecutionStats) {
    // `RuntimeCache::default()` has a memory-only `DiskStore`, so no disk cache is in play.
    let mut cache = RuntimeCache::default();
    cache.reconcile(program);
    let mut executor = Executor::default();
    let mut resource_stamps = RunResourceStamps::default();
    let stats = executor
        .run(
            program,
            &run.plan,
            &run.resolved,
            &mut cache,
            &mut resource_stamps,
            &FlattenMap::default(),
            None,
            CancelToken::never(),
        )
        .await;
    (cache, stats)
}

/// Like [`run`] but over a caller-owned cache, for multi-run tests (a reuse hit
/// needs the prior run's stamped digests and resident values).
async fn run_with(
    program: &ExecutionProgram,
    plan: &ExecutionPlan,
    cache: &mut RuntimeCache,
) -> ExecutionStats {
    let mut executor = Executor::default();
    // Resolve dispositions like the engine does. `straight_run` roots every node, so
    // the cut prunes nothing here — the cut itself is unit-tested in `resolve.rs`.
    let mut resolver = Resolver::default();
    let mut resource_stamps = RunResourceStamps::default();
    resolver.resolve(program, plan, cache, &resource_stamps);
    executor
        .run(
            program,
            plan,
            &resolver.run,
            cache,
            &mut resource_stamps,
            &FlattenMap::default(),
            None,
            CancelToken::never(),
        )
        .await
}

/// Like [`run`] but wires a live [`RunEvent`] channel through the executor
/// (with a [`self_mapped_flatten`] map, since this fixture bypasses the real
/// flattener) and drains every `PinnedOutputs` it sent, for tests asserting
/// exactly what the pinned-output push sends (and, via the returned cache,
/// what it leaves resident afterward).
async fn run_with_pinned(
    program: &ExecutionProgram,
    run: &TestRun,
) -> (RuntimeCache, ExecutionStats, Vec<PinnedOutputs>) {
    let mut cache = RuntimeCache::default();
    cache.reconcile(program);
    let mut executor = Executor::default();
    let mut resource_stamps = RunResourceStamps::default();
    let flatten = self_mapped_flatten(program);
    let (tx, mut rx) = mpsc::unbounded_channel::<RunEvent>();
    let stats = executor
        .run(
            program,
            &run.plan,
            &run.resolved,
            &mut cache,
            &mut resource_stamps,
            &flatten,
            Some(&tx),
            CancelToken::never(),
        )
        .await;
    drop(tx);
    let mut pushes = Vec::new();
    while let Some(event) = rx.recv().await {
        if let RunEvent::PinnedOutputs(p) = event {
            pushes.push(p);
        }
    }
    (cache, stats, pushes)
}

#[tokio::test]
async fn runs_in_order_resolving_binds_and_storing_outputs() {
    let mut p = Prog::default();
    let producer = async_lambda!(|_ctx, _state, _ev, _inputs, _demand, outputs| {
        outputs[0] = DynamicValue::Static(StaticValue::Int(7));
        Ok(())
    });
    let consumer = async_lambda!(|_ctx, _state, _ev, inputs, _demand, outputs| {
        let v = inputs[0].value.as_i64().unwrap();
        outputs[0] = DynamicValue::Static(StaticValue::Int(v + 1));
        Ok(())
    });
    let a = p.node(&[], 1, producer);
    let b = p.node(&[bind(a, 0)], 1, consumer);

    let plan = straight_run(&p.program);
    let (cache, stats) = run(&p.program, &plan).await;

    assert_eq!(
        cache.slots[&a].output_values().unwrap()[0].as_i64(),
        Some(7),
        "producer wrote 7"
    );
    assert_eq!(
        cache.slots[&b].output_values().unwrap()[0].as_i64(),
        Some(8),
        "consumer read 7 and wrote 7+1"
    );
    assert_eq!(stats.executed_nodes.len(), 2);
    assert!(stats.node_errors.is_empty());
}

#[tokio::test]
async fn upstream_error_skips_dependents_and_clears_output() {
    let mut p = Prog::default();
    let failing = async_lambda!(|_ctx, _state, _ev, _inputs, _demand, _outputs| {
        Err(anyhow::anyhow!("boom").into())
    });
    let downstream = async_lambda!(|_ctx, _state, _ev, _inputs, _demand, outputs| {
        outputs[0] = DynamicValue::Static(StaticValue::Int(1));
        Ok(())
    });
    let a = p.node(&[], 1, failing);
    let b = p.node(&[bind(a, 0)], 1, downstream);

    let plan = straight_run(&p.program);
    let (cache, stats) = run(&p.program, &plan).await;

    assert!(
        cache.slots[&a].output_values().is_none(),
        "an errored node's output is dropped (so it re-runs)"
    );
    assert!(
        cache.slots[&b].output_values().is_none(),
        "the dependent is skipped, producing nothing"
    );
    let error_of = |node_id: NodeId| {
        stats
            .node_errors
            .iter()
            .find(|e| e.node_id == node_id)
            .map(|e| e.error.to_string())
    };
    assert!(error_of(a).unwrap().contains("boom"));
    assert!(error_of(b).unwrap().contains("upstream"));
}

#[tokio::test]
async fn unbound_output_errors_only_when_demanded() {
    let mut p = Prog::default();
    let producer = async_lambda!(|_ctx, _state, _ev, _inputs, _demand, _outputs| { Ok(()) });
    let consumer = async_lambda!(|_ctx, _state, _ev, _inputs, _demand, outputs| {
        outputs[0] = DynamicValue::Static(StaticValue::Int(1));
        Ok(())
    });
    let a = p.node(&[], 2, producer);
    let b = p.node(&[bind(a, 0), bind(a, 1)], 1, consumer);

    let plan = run_with_readers(&p.program, vec![1, 1, 0]);
    let (cache, stats) = run(&p.program, &plan).await;
    let error_of = |node_id: NodeId| {
        stats
            .node_errors
            .iter()
            .find(|error| error.node_id == node_id)
            .map(|error| &error.error)
    };

    assert!(cache.slots[&a].output_values().is_none());
    assert!(cache.slots[&b].output_values().is_none());
    assert!(matches!(
        error_of(a),
        Some(RunError::OutputsNotProduced { outputs, .. }) if outputs == &[0, 1]
    ));
    assert!(matches!(
        error_of(b),
        Some(RunError::SkippedUpstream { .. })
    ));

    let mut p = Prog::default();
    let skipped = p.node(
        &[],
        1,
        async_lambda!(|_ctx, _state, _ev, _inputs, _demand, _outputs| { Ok(()) }),
    );
    let plan = run_with_readers(&p.program, vec![0]);
    let (cache, stats) = run(&p.program, &plan).await;

    assert!(stats.node_errors.is_empty());
    assert!(matches!(
        cache.slots[&skipped].output_values().unwrap().as_slice(),
        [DynamicValue::Unbound]
    ));
}

/// A `None`-cache producer's RAM output is dropped the moment its last consumer reads it.
/// `Executor::run` does no end-of-run eviction, so an emptied slot here is the *mid-run*
/// release and nothing else. A(None) → B(Ram): once B has read A, A is `Empty` while B keeps
/// its own value.
#[tokio::test]
async fn frees_none_cache_output_once_last_consumer_reads() {
    let mut p = Prog::default();
    let producer = async_lambda!(|_ctx, _s, _ev, _inputs, _demand, outputs| {
        outputs[0] = DynamicValue::Static(StaticValue::Int(7));
        Ok(())
    });
    let consumer = async_lambda!(|_ctx, _s, _ev, inputs, _demand, outputs| {
        let v = inputs[0].value.as_i64().unwrap();
        outputs[0] = DynamicValue::Static(StaticValue::Int(v + 1));
        Ok(())
    });
    let a = p.node(&[], 1, producer);
    let b = p.node(&[bind(a, 0)], 1, consumer);
    p.set_cache(a, CacheMode::None);
    p.set_cache(b, CacheMode::Ram);

    // A's one output has one consumer (B); B's output has a phantom consumer, so B never drains.
    let plan = run_with_readers(&p.program, vec![1, 1]);
    let (cache, _stats) = run(&p.program, &plan).await;

    assert!(
        matches!(cache.slots[&a].value, ValueState::Empty),
        "A (None) is freed the moment its last consumer B reads it: {:?}",
        cache.slots[&a].value
    );
    assert_eq!(
        cache.slots[&b].output_values().unwrap()[0].as_i64(),
        Some(8),
        "B (Ram) keeps its own output (7+1)"
    );
}

/// A host pin demands and receives the value before the downstream binding reads it, but
/// does not delay release after that final real reader.
#[tokio::test]
async fn pinned_delivery_does_not_create_a_reader() {
    let mut p = Prog::default();
    let producer = async_lambda!(|_ctx, _s, _ev, _inputs, _demand, outputs| {
        outputs[0] = DynamicValue::Static(StaticValue::Int(7));
        Ok(())
    });
    let consumer = async_lambda!(|_ctx, _s, _ev, inputs, _demand, outputs| {
        let v = inputs[0].value.as_i64().unwrap();
        outputs[0] = DynamicValue::Static(StaticValue::Int(v + 1));
        Ok(())
    });
    let a = p.node(&[], 1, producer);
    let b = p.node(&[bind(a, 0)], 1, consumer);
    p.set_cache(a, CacheMode::None);
    p.set_cache(b, CacheMode::Ram);
    p.set_output_pinned(a, 0, true);

    let plan = run_with_readers(&p.program, vec![1, 1]);
    let (cache, _stats, pushes) = run_with_pinned(&p.program, &plan).await;

    assert!(
        matches!(cache.slots[&a].value, ValueState::Empty),
        "A is released after its only binding reader despite host delivery: {:?}",
        cache.slots[&a].value
    );
    assert_eq!(pushes.len(), 1);
    assert_eq!(pushes[0].values[0].value.as_i64(), Some(7));
}

/// A pinned (node-seeded preview) root's output survives even in `CacheMode::None`,
/// unlike an unpinned `Skip` root on the same program. Demand controls production;
/// `run.plan.pinned` independently controls retention.
#[tokio::test]
async fn pinned_root_sees_demand_and_survives_drain() {
    use std::sync::Mutex;

    let seen: Arc<Mutex<Option<OutputDemand>>> = Arc::new(Mutex::new(None));
    let probe_seen = Arc::clone(&seen);
    let mut p = Prog::default();
    let probe = async_lambda!(
        move |_ctx, _s, _ev, _inputs, demand, outputs| { seen = Arc::clone(&probe_seen) } => {
            *seen.lock().unwrap() = Some(demand[0]);
            outputs[0] = DynamicValue::Static(StaticValue::Int(7));
            Ok(())
        }
    );
    let a = p.node(&[], 1, probe);
    p.set_cache(a, CacheMode::None);

    // Unpinned root, no consumers: the lambda reads `Skip` and the slot is
    // reclaimed the instant it's stored.
    let plan = run_with_readers(&p.program, vec![0]);
    let (cache, _stats) = run(&p.program, &plan).await;
    assert_eq!(*seen.lock().unwrap(), Some(OutputDemand::Skip));
    assert!(
        matches!(cache.slots[&a].value, ValueState::Empty),
        "unpinned Skip root is drained at store time: {:?}",
        cache.slots[&a].value
    );

    let mut plan = run_with_readers(&p.program, vec![0]);
    demand_output(&p.program, &mut plan, a, 0);
    plan.plan.pinned.insert(a);
    let (cache, _stats) = run(&p.program, &plan).await;
    assert_eq!(*seen.lock().unwrap(), Some(OutputDemand::Produce));
    assert_eq!(
        cache.slots[&a].output_values().unwrap()[0].as_i64(),
        Some(7),
        "pinned root's value survives the run"
    );
}

/// An individually pinned output pushes its fresh value the instant its node
/// finishes running.
#[tokio::test]
async fn pinned_output_pushes_right_after_it_runs() {
    let mut p = Prog::default();
    let producer = async_lambda!(|_ctx, _s, _ev, _inputs, _demand, outputs| {
        outputs[0] = DynamicValue::Static(StaticValue::Int(7));
        Ok(())
    });
    let a = p.node(&[], 1, producer);
    p.set_output_pinned(a, 0, true);

    let plan = straight_run(&p.program);
    let (_cache, _stats, pushes) = run_with_pinned(&p.program, &plan).await;

    assert_eq!(pushes.len(), 1, "one push for the one finished node");
    assert_eq!(pushes[0].node.node_id, a);
    assert_eq!(pushes[0].values.len(), 1);
    assert_eq!(pushes[0].values[0].port_idx, 0);
    assert_eq!(pushes[0].values[0].value.as_i64(), Some(7));
}

/// A pinned output with zero real consumers and a non-RAM cache mode is reclaimed
/// immediately after delivery because the reader column starts drained.
#[tokio::test]
async fn pinned_output_with_no_consumers_is_reclaimed_right_after_the_push() {
    let mut p = Prog::default();
    let producer = async_lambda!(|_ctx, _s, _ev, _inputs, _demand, outputs| {
        outputs[0] = DynamicValue::Static(StaticValue::Int(7));
        Ok(())
    });
    let a = p.node(&[], 1, producer);
    p.set_cache(a, CacheMode::None); // not Ram — retention must not be why it survives
    p.set_output_pinned(a, 0, true);

    let mut plan = run_with_readers(&p.program, vec![0]);
    demand_output(&p.program, &mut plan, a, 0);
    let (cache, _stats, pushes) = run_with_pinned(&p.program, &plan).await;

    assert_eq!(pushes.len(), 1, "the value was still pushed");
    assert!(
        matches!(cache.slots[&a].value, ValueState::Empty),
        "reclaimed right after the push, not left resident to end-of-run eviction: {:?}",
        cache.slots[&a].value
    );
}

/// A pinned-root node (a node-seeded on-demand preview target) pushes *every*
/// output, not just an individually-pinned one — `run.plan.pinned` alone is
/// enough to qualify the whole node.
#[tokio::test]
async fn pinned_root_pushes_every_output() {
    let mut p = Prog::default();
    let producer = async_lambda!(|_ctx, _s, _ev, _inputs, _demand, outputs| {
        outputs[0] = DynamicValue::Static(StaticValue::Int(1));
        outputs[1] = DynamicValue::Static(StaticValue::Int(2));
        Ok(())
    });
    let a = p.node(&[], 2, producer);

    let mut plan = straight_run(&p.program);
    plan.plan.pinned.insert(a);
    let (_cache, _stats, pushes) = run_with_pinned(&p.program, &plan).await;

    assert_eq!(pushes.len(), 1);
    assert_eq!(pushes[0].node.node_id, a);
    assert_eq!(pushes[0].values.len(), 2);
    assert_eq!(pushes[0].values[0].port_idx, 0);
    assert_eq!(pushes[0].values[0].value.as_i64(), Some(1));
    assert_eq!(pushes[0].values[1].port_idx, 1);
    assert_eq!(pushes[0].values[1].value.as_i64(), Some(2));
}

/// Neither an individually-pinned port nor a pinned root: no push at all,
/// even though the node ran successfully.
#[tokio::test]
async fn non_pinned_node_pushes_nothing() {
    let mut p = Prog::default();
    let producer = async_lambda!(|_ctx, _s, _ev, _inputs, _demand, outputs| {
        outputs[0] = DynamicValue::Static(StaticValue::Int(7));
        Ok(())
    });
    p.node(&[], 1, producer);

    let plan = straight_run(&p.program);
    let (_cache, _stats, pushes) = run_with_pinned(&p.program, &plan).await;

    assert!(
        pushes.is_empty(),
        "no pinned output or pinned root ⇒ no push"
    );
}

/// A pinned output on a node whose lambda *fails* pushes nothing — only a
/// fresh, successful `Ran` outcome triggers the push (mirrors
/// `RunPhase::Finished`'s own cancelled/failed suppression).
#[tokio::test]
async fn failed_pinned_node_pushes_nothing() {
    let mut p = Prog::default();
    let failing = async_lambda!(|_ctx, _s, _ev, _inputs, _demand, _outputs| {
        Err(anyhow::anyhow!("boom").into())
    });
    let a = p.node(&[], 1, failing);
    p.set_output_pinned(a, 0, true);

    let plan = straight_run(&p.program);
    let (_cache, _stats, pushes) = run_with_pinned(&p.program, &plan).await;

    assert!(
        pushes.is_empty(),
        "a failed node's pinned output never pushes"
    );
}

/// The release only reclaims modes that don't retain RAM: a `Ram` producer stays resident
/// even after every consumer has read it. A(Ram) → B — A survives B's read.
#[tokio::test]
async fn keeps_ram_cache_output_after_all_consumers_read() {
    let mut p = Prog::default();
    let producer = async_lambda!(|_ctx, _s, _ev, _inputs, _demand, outputs| {
        outputs[0] = DynamicValue::Static(StaticValue::Int(7));
        Ok(())
    });
    let consumer = async_lambda!(|_ctx, _s, _ev, inputs, _demand, outputs| {
        outputs[0] = DynamicValue::Static(StaticValue::Int(inputs[0].value.as_i64().unwrap()));
        Ok(())
    });
    let a = p.node(&[], 1, producer);
    let b = p.node(&[bind(a, 0)], 1, consumer);
    p.set_cache(a, CacheMode::Ram);
    p.set_cache(b, CacheMode::Ram);

    // A has one consumer (B, which reads it) and B has none (usage 0).
    let plan = run_with_readers(&p.program, vec![1, 0]);
    let (cache, _stats) = run(&p.program, &plan).await;

    assert_eq!(
        cache.slots[&a].output_values().unwrap()[0].as_i64(),
        Some(7),
        "A (Ram) is kept hot for the next run even though B has fully drained it"
    );
}

/// A reused consumer contributes no reader. The shared non-RAM producer is reclaimed as
/// soon as the one running consumer reads it, without waiting for end-of-run eviction.
#[tokio::test]
async fn reused_consumer_does_not_delay_last_read_reclamation() {
    let mut p = Prog::default();
    let producer = async_lambda!(|_ctx, _s, _ev, _inputs, _demand, outputs| {
        outputs[0] = DynamicValue::Static(StaticValue::Int(7));
        Ok(())
    });
    let consumer = async_lambda!(|_ctx, _s, _ev, inputs, _demand, outputs| {
        outputs[0] = DynamicValue::Static(StaticValue::Int(inputs[0].value.as_i64().unwrap()));
        Ok(())
    });
    let a = p.node(&[], 1, producer);
    let live = p.node(&[bind(a, 0)], 1, consumer.clone());
    let cached = p.node(&[bind(a, 0)], 1, consumer);
    p.program.e_nodes.get_mut(&a).unwrap().behavior = FuncBehavior::Pure;
    p.program.e_nodes.get_mut(&cached).unwrap().behavior = FuncBehavior::Pure;
    p.set_cache(a, CacheMode::None);
    p.set_cache(live, CacheMode::None);

    let plan = structural_plan(&p.program);
    let mut cache = RuntimeCache::default();
    cache.reconcile(&p.program);
    let first = run_with(&p.program, &plan, &mut cache).await;
    assert_eq!(first.executed_nodes.len(), 3);

    let second = run_with(&p.program, &plan, &mut cache).await;
    assert!(
        second.cached_nodes.contains(&cached),
        "the pure RAM consumer reuses its first-run result"
    );
    assert!(
        second.executed_nodes.iter().any(|node| node.node_id == a)
            && second
                .executed_nodes
                .iter()
                .any(|node| node.node_id == live),
        "the producer and impure consumer still run"
    );
    assert!(
        matches!(cache.slots[&a].value, ValueState::Empty),
        "the producer is reclaimed immediately after its only live reader"
    );
}

/// A node no one consumes (a sink, usage 0) is released the instant it finishes,
/// not held to end-of-run: a `None` output is dropped, a `Ram` output kept hot.
#[tokio::test]
async fn frees_zero_consumer_output_right_after_it_runs() {
    let mut p = Prog::default();
    let producer = || {
        async_lambda!(|_ctx, _s, _ev, _inputs, _demand, outputs| {
            outputs[0] = DynamicValue::Static(StaticValue::Int(7));
            Ok(())
        })
    };
    let a = p.node(&[], 1, producer());
    let b = p.node(&[], 1, producer());
    p.set_cache(a, CacheMode::None);
    p.set_cache(b, CacheMode::Ram);

    // Neither output is consumed.
    let plan = run_with_readers(&p.program, vec![0, 0]);
    let (cache, _stats) = run(&p.program, &plan).await;

    assert!(
        matches!(cache.slots[&a].value, ValueState::Empty),
        "A (None, no consumers) is freed right after it runs: {:?}",
        cache.slots[&a].value
    );
    assert_eq!(
        cache.slots[&b].output_values().unwrap()[0].as_i64(),
        Some(7),
        "B (Ram, no consumers) is kept hot"
    );
}

/// A node whose func has no implementation attached can't execute: it's reported as
/// its own per-node [`RunError::MissingLambda`] (not silently skipped), any stale
/// cached value is dropped so it can't be served as this run's result, and its
/// consumers skip with the usual errored-upstream propagation.
#[tokio::test]
async fn missing_lambda_reports_error_and_skips_consumers() {
    let mut p = Prog::default();
    let a = p.node(&[], 1, FuncLambda::None);
    let consumer = async_lambda!(|_ctx, _state, _ev, inputs, _demand, outputs| {
        outputs[0] = DynamicValue::Static(StaticValue::Int(inputs[0].value.as_i64().unwrap()));
        Ok(())
    });
    let b = p.node(&[bind(a, 0)], 1, consumer);

    let plan = straight_run(&p.program);
    let mut cache = RuntimeCache::default();
    cache.reconcile(&p.program);
    // A stale prior value on the lambda-less node must not be served as this run's result.
    cache.slots.get_mut(&a).unwrap().value = ValueState::Resident {
        snapshot: OutputSnapshot::new(vec![DynamicValue::Static(StaticValue::Int(9))]),
        produced_under: None,
    };
    let stats = run_with(&p.program, &plan.plan, &mut cache).await;

    assert!(
        stats.executed_nodes.is_empty(),
        "neither node ran: A has no lambda, B skips on the errored upstream"
    );
    assert!(
        cache.slots[&a].output_values().is_none(),
        "A's stale value is dropped, not served"
    );
    let error_of = |node_id: NodeId| {
        stats
            .node_errors
            .iter()
            .find(|e| e.node_id == node_id)
            .map(|e| &e.error)
    };
    assert!(
        matches!(error_of(a), Some(RunError::MissingLambda { .. })),
        "A reports its missing implementation: {:?}",
        error_of(a)
    );
    assert!(
        matches!(error_of(b), Some(RunError::SkippedUpstream { .. })),
        "B skips as errored-upstream: {:?}",
        error_of(b)
    );
}

/// A consumer whose digest is unchanged serves its cached value even when the
/// shared upstream re-ran for a *different* consumer and failed: the reuse verdict
/// is checked before the errored-dependency skip, so the valid cache is neither
/// cleared nor blamed for the upstream failure.
#[tokio::test]
async fn reuse_survives_failed_upstream_rerun() {
    let mut p = Prog::default();
    // A succeeds once (with 5), then fails every later invocation.
    let a = p.node(
        &[],
        1,
        async_lambda!(|_ctx, state, _ev, _inputs, _demand, outputs| {
            if state.get::<bool>().is_some() {
                return Err(anyhow::anyhow!("transient failure").into());
            }
            state.set(true);
            outputs[0] = DynamicValue::Static(StaticValue::Int(5));
            Ok(())
        }),
    );
    let consumer = || {
        async_lambda!(|_ctx, _state, _ev, inputs, _demand, outputs| {
            let v = inputs[0].value.as_i64().unwrap();
            outputs[0] = DynamicValue::Static(StaticValue::Int(v + 1));
            Ok(())
        })
    };
    let b = p.node(&[bind(a, 0)], 1, consumer());
    let c = p.node(&[bind(a, 0)], 1, consumer());
    // Content-cacheable (the fixture default is `Impure` = no digest, never a hit).
    for node_id in [a, b, c] {
        p.program.e_nodes.get_mut(&node_id).unwrap().behavior = FuncBehavior::Pure;
    }
    // A and C recompute every run; only B (the fixture default `Ram`) retains RAM.
    p.set_cache(a, CacheMode::None);
    p.set_cache(c, CacheMode::None);

    // A's one output has two consumers; B/C outputs are unread sinks. B's count 1 keeps
    // the release accounting off this test's path (Ram retains regardless); C's count 0
    // lets the store-time drain reclaim it — the executor harness has no end-of-run
    // eviction phase, and a `None` value left resident would serve as a reuse hit in
    // run 2 (residency is what the reuse check trusts), masking the skip under test.
    let plan = run_with_readers(&p.program, vec![2, 1, 0]);
    let mut cache = RuntimeCache::default();
    cache.reconcile(&p.program);

    // Run 1: A=5, B=C=6, everything computes.
    let stats1 = run_with(&p.program, &plan.plan, &mut cache).await;
    assert_eq!(stats1.executed_nodes.len(), 3);
    assert_eq!(
        cache.slots[&b].output_values().unwrap()[0].as_i64(),
        Some(6)
    );

    // Run 2: A re-runs (nothing cached it) and fails. B's digest is unchanged, so it
    // is served as cached — not skipped — and its resident 6 survives. C recomputes,
    // sees the errored upstream, and is skipped.
    let stats2 = run_with(&p.program, &plan.plan, &mut cache).await;
    let (a_id, b_id, c_id) = (a, b, c);
    assert!(
        stats2.cached_nodes.contains(&b_id),
        "B is a reuse hit despite A's failure"
    );
    assert_eq!(
        cache.slots[&b].output_values().unwrap()[0].as_i64(),
        Some(6),
        "B's valid cached value survives the sibling failure"
    );
    let errored: Vec<NodeId> = stats2.node_errors.iter().map(|e| e.node_id).collect();
    assert!(errored.contains(&a_id), "A's own failure is reported");
    assert!(
        errored.contains(&c_id),
        "C is skipped for the errored upstream"
    );
    assert!(!errored.contains(&b_id), "B carries no error");
}
