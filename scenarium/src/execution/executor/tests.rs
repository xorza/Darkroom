use std::sync::Arc;

use super::*;
use crate::async_lambda;
use crate::execution::cache::runtime::RuntimeCache;
use crate::execution::cache::slot::{OutputSnapshot, ValueState};
use crate::execution::identity::ExecutionNodeId;
use crate::execution::plan::NodeVerdict;
use crate::execution::program::index::{
    NodeColumn, NodeIdx, NodeSet, OutputAddr, OutputColumn, OutputIdx,
};
use crate::execution::program::{ExecutionBinding, ExecutionInput, ExecutionNode, ExecutionOutput};
use crate::execution::report::internals::DiscardedReports;
use crate::execution::resolve::{Disposition, ResolvedOutputs, Resolver};
use crate::graph::CacheMode;
use crate::node::definition::{FuncBehavior, FuncId};
use crate::node::lambda::Invocation;
use crate::node::lambda::internals;
use crate::node::lambda::{FuncLambda, OutputDemand};
use crate::{DynamicValue, StaticValue};

/// Hand-built program with real lambdas. Inputs are all optional here (the
/// planner gates required ones; these tests drive the executor directly).
#[derive(Default)]
struct Prog {
    program: Program,
}

impl Prog {
    fn node(
        &mut self,
        inputs: &[ExecutionBinding],
        outputs: u32,
        lambda: FuncLambda,
    ) -> ExecutionNodeId {
        let inputs = self
            .program
            .inputs
            .append(inputs.iter().map(|binding| ExecutionInput {
                required: false,
                stamps_fs_path: false,
                binding: binding.clone(),
            }));
        let outputs = self
            .program
            .outputs
            .append((0..outputs).map(|_| ExecutionOutput::default()));
        let idx = self.program.e_nodes.len();
        let e_node_id = ExecutionNodeId::from_u128(idx as u128 + 1);
        self.program.push(
            e_node_id,
            ExecutionNode {
                func_id: FuncId::from_u128(idx as u128 + 1),
                inputs,
                outputs,
                lambda,
                // `CacheMode` now defaults to `None`; these tests assume outputs are
                // retained (`Ram`) unless a case flips it via `set_cache`.
                cache: CacheMode::Ram,
                ..Default::default()
            },
        );
        e_node_id
    }

    /// Override a node's [`CacheMode`] (nodes default to `Ram`). Drives the mid-run
    /// output-release tests, which turn on the non-RAM modes.
    fn set_cache(&mut self, e_node_id: ExecutionNodeId, cache: CacheMode) {
        self.program.by_id_mut(e_node_id).cache = cache;
    }

    /// Override a node's [`FuncBehavior`] (nodes default to `Impure`, which has no digest
    /// and so can never be reused).
    fn set_behavior(&mut self, e_node_id: ExecutionNodeId, behavior: FuncBehavior) {
        self.program.by_id_mut(e_node_id).behavior = behavior;
    }
}

#[derive(Debug)]
struct TestRun {
    plan: ExecutionPlan,
    resolver: Resolver,
}

/// A `straight_run` with an explicit per-output consumer count (indexed by output-pool
/// index, so its length is `n_outputs`), instead of the all-`1` default. Lets a test claim
/// more consumers than actually read (to prove the release waits for the full count) or none
/// (a sink, released the instant it runs).
fn run_with_readers(program: &Program, readers: Vec<u32>) -> TestRun {
    assert_eq!(readers.len(), program.outputs.len());
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
    let mut disposition = NodeColumn::default();
    disposition.reset(program.e_nodes.len(), Disposition::Run);
    TestRun {
        plan: structural_plan(program),
        resolver: Resolver {
            disposition,
            outputs: ResolvedOutputs {
                demand: OutputColumn::from(demand),
                readers: OutputColumn::from(readers),
            },
        },
    }
}

fn demand_output(program: &Program, run: &mut TestRun, address: OutputAddr) {
    let output_idx = program.output_idx(address);
    run.resolver.outputs.demand[output_idx] = OutputDemand::Produce;
}

/// These tests name nodes by their stable id; the program owns the id ↔ index
/// mapping the production paths carry directly.
fn nx(program: &Program, e_node_id: ExecutionNodeId) -> NodeIdx {
    program.e_node_index[&e_node_id]
}

fn output(program: &Program, e_node_id: ExecutionNodeId, port_idx: usize) -> OutputAddr {
    OutputAddr {
        node_idx: nx(program, e_node_id),
        port_idx: port_idx as u32,
    }
}

fn bind(program: &Program, e_node_id: ExecutionNodeId, port: usize) -> ExecutionBinding {
    ExecutionBinding::Bind(output(program, e_node_id, port))
}

/// A resolved run that runs every node in index order, each output marked needed. These tests
/// drive the run loop directly with an all-`needed` mask (the reuse/cut logic is
/// unit-tested in `resolve.rs`), so `roots` is irrelevant here.
fn straight_run(program: &Program) -> TestRun {
    run_with_readers(program, vec![1; program.outputs.len()])
}

fn structural_plan(program: &Program) -> ExecutionPlan {
    let process_order: Vec<_> = (0..program.e_nodes.len())
        .map(|idx| NodeIdx(idx as u32))
        .collect();
    let mut verdicts = NodeColumn::default();
    verdicts.reset(program.e_nodes.len(), NodeVerdict::Execute);
    let mut roots = NodeSet::default();
    roots.reset(program.e_nodes.len());
    for &node_idx in &process_order {
        roots.insert(node_idx);
    }
    let mut seeded = NodeSet::default();
    seeded.reset(program.e_nodes.len());
    let mut event_sources = NodeSet::default();
    event_sources.reset(program.e_nodes.len());
    ExecutionPlan {
        process_order,
        verdicts,
        roots,
        seeded,
        event_sources,
    }
}

#[test]
#[cfg(debug_assertions)]
fn debug_assertions_reject_invalid_output_indexes_and_reader_counts() {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    let mut p = Prog::default();
    let e_node_id = p.node(&[], 1, FuncLambda::default());
    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            p.program.output_idx(output(&p.program, e_node_id, 1))
        }))
        .is_err(),
        "a node-local output outside its compiled range must trip in debug"
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

async fn run(program: &Program, run: &TestRun) -> (RuntimeCache, ExecutionOutcome) {
    // `RuntimeCache::default()` has a memory-only `DiskStore`, so no disk cache is in play.
    let mut cache = RuntimeCache::default();
    cache.reconcile_fresh(program);
    let mut executor = Executor::default();
    let mut stats = ExecutionOutcome::default();
    executor
        .run(
            RunRequest {
                program,
                plan: &run.plan,
                resolver: &run.resolver,
                cache: &mut cache,
                reporter: &mut DiscardedReports,
                cancel: CancelToken::never(),
            },
            &mut stats,
        )
        .await;
    (cache, stats)
}

/// Like [`run`] but over a caller-owned cache, for multi-run tests (a reuse hit
/// needs the prior run's stamped digests and resident values).
async fn run_with(
    program: &Program,
    plan: &ExecutionPlan,
    cache: &mut RuntimeCache,
) -> ExecutionOutcome {
    let mut executor = Executor::default();
    // Resolve dispositions like the engine does. `straight_run` roots every node, so
    // the cut prunes nothing here — the cut itself is unit-tested in `resolve.rs`.
    let mut resolver = Resolver::default();
    resolver.resolve(program, plan, cache).await;
    let mut outcome = ExecutionOutcome::default();
    executor
        .run(
            RunRequest {
                program,
                plan,
                resolver: &resolver,
                cache,
                reporter: &mut DiscardedReports,
                cancel: CancelToken::never(),
            },
            &mut outcome,
        )
        .await;
    outcome
}

#[tokio::test]
async fn runs_in_order_resolving_binds_and_storing_outputs() {
    let mut p = Prog::default();
    let producer = async_lambda!(|Invocation { outputs, .. }| {
        outputs[0] = DynamicValue::Static(StaticValue::Int(7));
        Ok(())
    });
    let consumer = async_lambda!(|Invocation {
                                      inputs, outputs, ..
                                  }| {
        let v = inputs[0].as_i64().unwrap();
        outputs[0] = DynamicValue::Static(StaticValue::Int(v + 1));
        Ok(())
    });
    let a = p.node(&[], 1, producer);
    let b = p.node(&[bind(&p.program, a, 0)], 1, consumer);

    let plan = straight_run(&p.program);
    let (cache, stats) = run(&p.program, &plan).await;

    assert_eq!(
        cache.slots[nx(&p.program, a)].output_values().unwrap()[0].as_i64(),
        Some(7),
        "producer wrote 7"
    );
    assert_eq!(
        cache.slots[nx(&p.program, b)].output_values().unwrap()[0].as_i64(),
        Some(8),
        "consumer read 7 and wrote 7+1"
    );
    assert_eq!(stats.executed_nodes.len(), 2);
    assert!(stats.node_errors.is_empty());
}

#[tokio::test]
async fn upstream_error_retires_skipped_reads_without_harming_live_readers() {
    let mut p = Prog::default();
    let producer = async_lambda!(|Invocation { outputs, .. }| {
        outputs[0] = DynamicValue::Static(StaticValue::Int(7));
        Ok(())
    });
    let failing = async_lambda!(|_| { Err(internals::failure("boom")) });
    let skipped = async_lambda!(|Invocation { outputs, .. }| {
        outputs[0] = DynamicValue::Static(StaticValue::Int(1));
        Ok(())
    });
    let live = async_lambda!(|Invocation {
                                  inputs, outputs, ..
                              }| {
        outputs[0] = DynamicValue::Static(StaticValue::Int(inputs[0].as_i64().unwrap()));
        Ok(())
    });
    let healthy = p.node(&[], 1, producer);
    let failed = p.node(&[], 1, failing);
    let blocked = p.node(
        &[bind(&p.program, healthy, 0), bind(&p.program, failed, 0)],
        1,
        skipped,
    );
    let surviving = p.node(&[bind(&p.program, healthy, 0)], 1, live);
    p.set_cache(healthy, CacheMode::None);

    let plan = run_with_readers(&p.program, vec![2, 1, 0, 0]);
    let (cache, stats) = run(&p.program, &plan).await;

    assert!(
        cache.slots[nx(&p.program, failed)]
            .output_values()
            .is_none(),
        "an errored node's output is dropped (so it re-runs)"
    );
    assert!(
        cache.slots[nx(&p.program, blocked)]
            .output_values()
            .is_none(),
        "the dependent is skipped, producing nothing"
    );
    assert_eq!(
        cache.slots[nx(&p.program, surviving)]
            .output_values()
            .unwrap()[0]
            .as_i64(),
        Some(7),
        "retiring the blocked read leaves the healthy value for its live reader"
    );
    assert!(
        matches!(
            cache.slots[nx(&p.program, healthy)].value,
            ValueState::Empty
        ),
        "the healthy non-RAM producer is reclaimed after the live reader lands"
    );
    let error_of = |e_node_id: ExecutionNodeId| {
        stats
            .node_errors
            .iter()
            .find(|e| e.e_node_id == e_node_id)
            .map(|e| e.error.to_string())
    };
    assert!(error_of(failed).unwrap().contains("boom"));
    assert!(error_of(blocked).unwrap().contains("upstream"));
}

#[tokio::test]
async fn cancellation_retires_reads_owned_by_the_unreached_tail() {
    let mut p = Prog::default();
    let producer = async_lambda!(|Invocation { outputs, .. }| {
        outputs[0] = DynamicValue::Static(StaticValue::Int(7));
        Ok(())
    });
    let cancel = async_lambda!(|Invocation { ctx, .. }| {
        ctx.cancel_flag().cancel();
        Ok(())
    });
    let pending = async_lambda!(|_| { panic!("a cancelled tail consumer must not run") });
    let source = p.node(&[], 1, producer);
    p.node(&[], 0, cancel);
    p.node(&[bind(&p.program, source, 0)], 0, pending);
    p.set_cache(source, CacheMode::None);

    let run = run_with_readers(&p.program, vec![1]);
    let mut cache = RuntimeCache::default();
    cache.reconcile_fresh(&p.program);
    let mut executor = Executor::default();
    let mut stats = ExecutionOutcome::default();
    executor
        .run(
            RunRequest {
                program: &p.program,
                plan: &run.plan,
                resolver: &run.resolver,
                cache: &mut cache,
                reporter: &mut DiscardedReports,
                cancel: CancelToken::new(),
            },
            &mut stats,
        )
        .await;

    assert!(stats.cancelled);
    assert_eq!(
        executor.remaining_reads.counts[p.program.output_idx(output(&p.program, source, 0))],
        0,
        "the pending consumer's read was retired"
    );
    assert!(
        matches!(cache.slots[nx(&p.program, source)].value, ValueState::Empty),
        "tail retirement reclaims the source before the engine's final sweep"
    );
}

#[tokio::test]
async fn unbound_output_errors_only_when_demanded() {
    let mut p = Prog::default();
    let producer = async_lambda!(|_| { Ok(()) });
    let consumer = async_lambda!(|Invocation { outputs, .. }| {
        outputs[0] = DynamicValue::Static(StaticValue::Int(1));
        Ok(())
    });
    let a = p.node(&[], 2, producer);
    let b = p.node(
        &[bind(&p.program, a, 0), bind(&p.program, a, 1)],
        1,
        consumer,
    );

    let plan = run_with_readers(&p.program, vec![1, 1, 0]);
    let (cache, stats) = run(&p.program, &plan).await;
    let error_of = |e_node_id: ExecutionNodeId| {
        stats
            .node_errors
            .iter()
            .find(|error| error.e_node_id == e_node_id)
            .map(|error| &error.error)
    };

    assert!(cache.slots[nx(&p.program, a)].output_values().is_none());
    assert!(cache.slots[nx(&p.program, b)].output_values().is_none());
    assert!(matches!(
        error_of(a),
        Some(RunError::OutputsNotProduced { outputs, .. }) if outputs == &[0, 1]
    ));
    assert!(matches!(
        error_of(b),
        Some(RunError::SkippedUpstream { .. })
    ));

    let mut p = Prog::default();
    let skipped = p.node(&[], 1, async_lambda!(|_| { Ok(()) }));
    let plan = run_with_readers(&p.program, vec![0]);
    let (cache, stats) = run(&p.program, &plan).await;

    assert!(stats.node_errors.is_empty());
    assert!(matches!(
        cache.slots[nx(&p.program, skipped)]
            .output_values()
            .unwrap()
            .as_slice(),
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
    let producer = async_lambda!(|Invocation { outputs, .. }| {
        outputs[0] = DynamicValue::Static(StaticValue::Int(7));
        Ok(())
    });
    let consumer = async_lambda!(|Invocation {
                                      inputs, outputs, ..
                                  }| {
        let v = inputs[0].as_i64().unwrap();
        outputs[0] = DynamicValue::Static(StaticValue::Int(v + 1));
        Ok(())
    });
    let a = p.node(&[], 1, producer);
    let b = p.node(&[bind(&p.program, a, 0)], 1, consumer);
    p.set_cache(a, CacheMode::None);
    p.set_cache(b, CacheMode::Ram);

    // A's one output has one consumer (B); B's output has a phantom consumer, so B never drains.
    let plan = run_with_readers(&p.program, vec![1, 1]);
    let (cache, _stats) = run(&p.program, &plan).await;

    assert!(
        matches!(cache.slots[nx(&p.program, a)].value, ValueState::Empty),
        "A (None) is freed the moment its last consumer B reads it: {:?}",
        cache.slots[nx(&p.program, a)].value
    );
    assert_eq!(
        cache.slots[nx(&p.program, b)].output_values().unwrap()[0].as_i64(),
        Some(8),
        "B (Ram) keeps its own output (7+1)"
    );
}

/// A lambda can name the execution node it is running as — the hook a host-side
/// sink (an editor value view, a per-node logger) needs to attribute what it
/// receives. Set per invoke, so two nodes sharing one lambda still report
/// distinct identities, and cleared once the run is over.
#[tokio::test]
async fn a_lambda_reads_the_execution_node_it_is_running_as() {
    use std::sync::Mutex;

    let seen: Arc<Mutex<Vec<ExecutionNodeId>>> = Arc::new(Mutex::new(Vec::new()));
    let mut p = Prog::default();
    let probe_seen = Arc::clone(&seen);
    let first = async_lambda!(
        move |Invocation { ctx, outputs, .. }| { seen = Arc::clone(&probe_seen) } => {
            seen.lock().unwrap().push(ctx.current_node());
            outputs[0] = DynamicValue::Static(StaticValue::Int(1));
            Ok(())
        }
    );
    let probe_seen = Arc::clone(&seen);
    let second = async_lambda!(
        move |Invocation { ctx, outputs, .. }| { seen = Arc::clone(&probe_seen) } => {
            seen.lock().unwrap().push(ctx.current_node());
            outputs[0] = DynamicValue::Static(StaticValue::Int(2));
            Ok(())
        }
    );
    let a = p.node(&[], 1, first);
    let b = p.node(&[], 1, second);

    let plan = straight_run(&p.program);
    let (_cache, _stats) = run(&p.program, &plan).await;

    // Index order is id order, and `Prog` mints ascending ids, so `a` runs first.
    assert_eq!(*seen.lock().unwrap(), vec![a, b]);
    assert_ne!(a, b, "the two nodes are distinguishable at all");
}

/// A node seed ("run to this node") demands the node's output but does not
/// override `CacheMode::None` retention — targeting says *compute this*, not
/// *keep this*.
#[tokio::test]
async fn a_node_seed_demands_its_output_without_retaining_it() {
    use std::sync::Mutex;

    let seen: Arc<Mutex<Option<OutputDemand>>> = Arc::new(Mutex::new(None));
    let probe_seen = Arc::clone(&seen);
    let mut p = Prog::default();
    let probe = async_lambda!(
        move |Invocation { demand, outputs, .. }| { seen = Arc::clone(&probe_seen) } => {
            *seen.lock().unwrap() = Some(demand[0]);
            outputs[0] = DynamicValue::Static(StaticValue::Int(7));
            Ok(())
        }
    );
    let a = p.node(&[], 1, probe);
    p.set_cache(a, CacheMode::None);

    // Unseeded root, no consumers: the lambda reads `Skip` and the slot is
    // reclaimed the instant it's stored.
    let plan = run_with_readers(&p.program, vec![0]);
    let (cache, _stats) = run(&p.program, &plan).await;
    assert_eq!(*seen.lock().unwrap(), Some(OutputDemand::Skip));
    assert!(
        matches!(cache.slots[nx(&p.program, a)].value, ValueState::Empty),
        "unseeded Skip root is drained at store time: {:?}",
        cache.slots[nx(&p.program, a)].value
    );

    let mut plan = run_with_readers(&p.program, vec![0]);
    demand_output(&p.program, &mut plan, output(&p.program, a, 0));
    plan.plan.seeded.insert(nx(&p.program, a));
    let (cache, _stats) = run(&p.program, &plan).await;
    assert_eq!(*seen.lock().unwrap(), Some(OutputDemand::Produce));
    assert!(
        matches!(cache.slots[nx(&p.program, a)].value, ValueState::Empty),
        "targeting controls demand, not RAM retention: {:?}",
        cache.slots[nx(&p.program, a)].value
    );
}

/// A reused node whose demanded output no consumer reads is released as soon as
/// it is served, rather than held to end-of-run eviction.
#[tokio::test]
async fn a_reused_output_with_no_consumers_is_reclaimed_immediately() {
    let mut p = Prog::default();
    let producer = async_lambda!(|_| { panic!("a reused node must not invoke its lambda") });
    let a = p.node(&[], 1, producer);
    p.set_cache(a, CacheMode::Disk);
    // Only a `Pure` node earns a digest, and the run loop serves a `Reuse` by asking the
    // cache for the value — so the fixture has to be the coherent state a resolver `Reuse`
    // implies: a resident snapshot produced under the node's current digest.
    p.set_behavior(a, FuncBehavior::Pure);

    let mut run = run_with_readers(&p.program, vec![0]);
    demand_output(&p.program, &mut run, output(&p.program, a, 0));
    run.resolver.disposition[nx(&p.program, a)] = Disposition::Reuse;

    let mut cache = RuntimeCache::default();
    cache.reconcile_fresh(&p.program);
    cache.stamp_digest(&p.program, nx(&p.program, a));
    cache.slots[nx(&p.program, a)].value = ValueState::Resident {
        snapshot: OutputSnapshot::new(vec![DynamicValue::Static(StaticValue::Int(7))]),
        produced_under: cache.slots[nx(&p.program, a)].current_digest,
    };
    let mut executor = Executor::default();
    let mut stats = ExecutionOutcome::default();
    executor
        .run(
            RunRequest {
                program: &p.program,
                plan: &run.plan,
                resolver: &run.resolver,
                cache: &mut cache,
                reporter: &mut DiscardedReports,
                cancel: CancelToken::never(),
            },
            &mut stats,
        )
        .await;

    assert_eq!(stats.cached_nodes, vec![a]);
    assert!(
        matches!(cache.slots[nx(&p.program, a)].value, ValueState::Empty),
        "the reused value is released as soon as it is served"
    );
}

/// The release only reclaims modes that don't retain RAM: a `Ram` producer stays resident
/// even after every consumer has read it. A(Ram) → B — A survives B's read.
#[tokio::test]
async fn keeps_ram_cache_output_after_all_consumers_read() {
    let mut p = Prog::default();
    let producer = async_lambda!(|Invocation { outputs, .. }| {
        outputs[0] = DynamicValue::Static(StaticValue::Int(7));
        Ok(())
    });
    let consumer = async_lambda!(|Invocation {
                                      inputs, outputs, ..
                                  }| {
        outputs[0] = DynamicValue::Static(StaticValue::Int(inputs[0].as_i64().unwrap()));
        Ok(())
    });
    let a = p.node(&[], 1, producer);
    let b = p.node(&[bind(&p.program, a, 0)], 1, consumer);
    p.set_cache(a, CacheMode::Ram);
    p.set_cache(b, CacheMode::Ram);

    // A has one consumer (B, which reads it) and B has none (usage 0).
    let plan = run_with_readers(&p.program, vec![1, 0]);
    let (cache, _stats) = run(&p.program, &plan).await;

    assert_eq!(
        cache.slots[nx(&p.program, a)].output_values().unwrap()[0].as_i64(),
        Some(7),
        "A (Ram) is kept hot for the next run even though B has fully drained it"
    );
}

/// A reused consumer contributes no reader. The shared non-RAM producer is reclaimed as
/// soon as the one running consumer reads it, without waiting for end-of-run eviction.
#[tokio::test]
async fn reused_consumer_does_not_delay_last_read_reclamation() {
    let mut p = Prog::default();
    let producer = async_lambda!(|Invocation { outputs, .. }| {
        outputs[0] = DynamicValue::Static(StaticValue::Int(7));
        Ok(())
    });
    let consumer = async_lambda!(|Invocation {
                                      inputs, outputs, ..
                                  }| {
        outputs[0] = DynamicValue::Static(StaticValue::Int(inputs[0].as_i64().unwrap()));
        Ok(())
    });
    let a = p.node(&[], 1, producer);
    let live = p.node(&[bind(&p.program, a, 0)], 1, consumer.clone());
    let cached = p.node(&[bind(&p.program, a, 0)], 1, consumer);
    p.program.by_id_mut(a).behavior = FuncBehavior::Pure;
    p.program.by_id_mut(cached).behavior = FuncBehavior::Pure;
    p.set_cache(a, CacheMode::None);
    p.set_cache(live, CacheMode::None);

    let plan = structural_plan(&p.program);
    let mut cache = RuntimeCache::default();
    cache.reconcile_fresh(&p.program);
    let first = run_with(&p.program, &plan, &mut cache).await;
    assert_eq!(first.executed_nodes.len(), 3);

    let second = run_with(&p.program, &plan, &mut cache).await;
    assert!(
        second.cached_nodes.contains(&cached),
        "the pure RAM consumer reuses its first-run result"
    );
    assert!(
        second.executed_nodes.iter().any(|node| node.e_node_id == a)
            && second
                .executed_nodes
                .iter()
                .any(|node| node.e_node_id == live),
        "the producer and impure consumer still run"
    );
    assert!(
        matches!(cache.slots[nx(&p.program, a)].value, ValueState::Empty),
        "the producer is reclaimed immediately after its only live reader"
    );
}

/// A node no one consumes (a sink, usage 0) is released the instant it finishes,
/// not held to end-of-run: a `None` output is dropped, a `Ram` output kept hot.
#[tokio::test]
async fn frees_zero_consumer_output_right_after_it_runs() {
    let mut p = Prog::default();
    let producer = || {
        async_lambda!(|Invocation { outputs, .. }| {
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
        matches!(cache.slots[nx(&p.program, a)].value, ValueState::Empty),
        "A (None, no consumers) is freed right after it runs: {:?}",
        cache.slots[nx(&p.program, a)].value
    );
    assert_eq!(
        cache.slots[nx(&p.program, b)].output_values().unwrap()[0].as_i64(),
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
    let producer = async_lambda!(|Invocation { outputs, .. }| {
        outputs[0] = DynamicValue::Static(StaticValue::Int(7));
        Ok(())
    });
    let source = p.node(&[], 1, producer);
    let missing = p.node(&[bind(&p.program, source, 0)], 1, FuncLambda::None);
    let consumer = async_lambda!(|Invocation {
                                      inputs, outputs, ..
                                  }| {
        outputs[0] = DynamicValue::Static(StaticValue::Int(inputs[0].as_i64().unwrap()));
        Ok(())
    });
    let downstream = p.node(&[bind(&p.program, missing, 0)], 1, consumer);

    let mut plan = structural_plan(&p.program);
    plan.roots.reset(p.program.e_nodes.len());
    plan.roots.insert(nx(&p.program, downstream));
    let mut cache = RuntimeCache::default();
    cache.reconcile_fresh(&p.program);
    cache.slots[nx(&p.program, missing)].value = ValueState::Resident {
        snapshot: OutputSnapshot::new(vec![DynamicValue::Static(StaticValue::Int(9))]),
        produced_under: None,
    };
    let stats = run_with(&p.program, &plan, &mut cache).await;

    assert!(
        stats.executed_nodes.is_empty(),
        "the source is cut, the missing implementation errors, and its consumer skips"
    );
    assert!(
        cache.slots[nx(&p.program, missing)]
            .output_values()
            .is_none(),
        "the missing node's stale value is dropped, not served"
    );
    let error_of = |e_node_id: ExecutionNodeId| {
        stats
            .node_errors
            .iter()
            .find(|e| e.e_node_id == e_node_id)
            .map(|e| &e.error)
    };
    assert!(
        matches!(error_of(missing), Some(RunError::MissingLambda { .. })),
        "the node reports its missing implementation: {:?}",
        error_of(missing)
    );
    assert!(
        matches!(error_of(downstream), Some(RunError::SkippedUpstream { .. })),
        "the consumer skips as errored-upstream: {:?}",
        error_of(downstream)
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
        async_lambda!(|Invocation { state, outputs, .. }| {
            if state.get::<bool>().is_some() {
                return Err(internals::failure("transient failure"));
            }
            state.set(true);
            outputs[0] = DynamicValue::Static(StaticValue::Int(5));
            Ok(())
        }),
    );
    let consumer = || {
        async_lambda!(|Invocation {
                           inputs, outputs, ..
                       }| {
            let v = inputs[0].as_i64().unwrap();
            outputs[0] = DynamicValue::Static(StaticValue::Int(v + 1));
            Ok(())
        })
    };
    let b = p.node(&[bind(&p.program, a, 0)], 1, consumer());
    let c = p.node(&[bind(&p.program, a, 0)], 1, consumer());
    // Content-cacheable (the fixture default is `Impure` = no digest, never a hit).
    for e_node_id in [a, b, c] {
        p.program.by_id_mut(e_node_id).behavior = FuncBehavior::Pure;
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
    cache.reconcile_fresh(&p.program);

    // Run 1: A=5, B=C=6, everything computes.
    let stats1 = run_with(&p.program, &plan.plan, &mut cache).await;
    assert_eq!(stats1.executed_nodes.len(), 3);
    assert_eq!(
        cache.slots[nx(&p.program, b)].output_values().unwrap()[0].as_i64(),
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
        cache.slots[nx(&p.program, b)].output_values().unwrap()[0].as_i64(),
        Some(6),
        "B's valid cached value survives the sibling failure"
    );
    let errored: Vec<ExecutionNodeId> = stats2.node_errors.iter().map(|e| e.e_node_id).collect();
    assert!(errored.contains(&a_id), "A's own failure is reported");
    assert!(
        errored.contains(&c_id),
        "C is skipped for the errored upstream"
    );
    assert!(!errored.contains(&b_id), "B carries no error");
}
