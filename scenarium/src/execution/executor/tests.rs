//! The run loop over programs [`ProgramBuilder`] states directly.
//!
//! Every node here declares its [`CacheMode`] explicitly: which values survive
//! which read *is* the subject, so a fixture that left retention to a default
//! would be asserting on something it never said.

use std::sync::Arc;

use super::*;
use crate::containers::column::{Column, Idx};
use crate::execution::identity::OutputIdx;
use crate::execution::schedule::NodeState;
use crate::graph::func::lambda::internals;
use crate::graph::func::lambda::{FuncLambda, Invocation};
use crate::graph::identity::NodeId;
use crate::graph::node::CacheMode;
use crate::testing::program::ProgramBuilder;
use crate::{ConstValue, DynamicValue, async_lambda};

fn value(value: i64) -> DynamicValue {
    DynamicValue::Static(ConstValue::Int(value))
}

fn producer() -> FuncLambda {
    async_lambda!(|Invocation { outputs, .. }| {
        outputs[0] = DynamicValue::Static(ConstValue::Int(7));
        Ok(())
    })
}

/// Writes its first input straight back out.
fn relay() -> FuncLambda {
    async_lambda!(|Invocation {
                       inputs, outputs, ..
                   }| {
        outputs[0] = DynamicValue::Static(ConstValue::Int(inputs[0].as_i64().unwrap()));
        Ok(())
    })
}

/// Writes its first input plus one — so a consumer's output says which value
/// reached it, not merely that one did.
fn increment() -> FuncLambda {
    async_lambda!(|Invocation {
                       inputs, outputs, ..
                   }| {
        let v = inputs[0].as_i64().unwrap();
        outputs[0] = DynamicValue::Static(ConstValue::Int(v + 1));
        Ok(())
    })
}

#[test]
#[cfg(debug_assertions)]
fn debug_assertions_reject_invalid_output_indexes_and_reader_counts() {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    let mut prog = ProgramBuilder::default();
    let node = prog.node().outputs(1).add();
    let program = prog.program();
    assert!(
        catch_unwind(AssertUnwindSafe(|| program.output_idx(node.addr(1)))).is_err(),
        "a node-local output outside its compiled range must trip in debug"
    );

    if let Ok(index) = usize::try_from(u64::from(u32::MAX) + 1) {
        assert!(
            catch_unwind(|| OutputIdx::from_idx(index)).is_err(),
            "the output pool cannot exceed its u32 index representation"
        );
    }

    let mut reads = RemainingOutputReads {
        counts: Column::from(vec![0]),
    };
    assert!(
        catch_unwind(AssertUnwindSafe(|| reads.consume(OutputIdx(0)))).is_err(),
        "a reader count cannot be consumed below zero"
    );
}

#[tokio::test]
async fn runs_in_order_resolving_binds_and_storing_outputs() {
    let mut prog = ProgramBuilder::default();
    let a = prog
        .node()
        .cache(CacheMode::Ram)
        .outputs(1)
        .lambda(producer())
        .add();
    let b = prog
        .node()
        .cache(CacheMode::Ram)
        .input(a.out(0))
        .outputs(1)
        .lambda(increment())
        .add();

    let mut run = prog.runs();
    run.go().await;

    assert_eq!(run.output_i64(a, 0), Some(7), "producer wrote 7");
    assert_eq!(
        run.output_i64(b, 0),
        Some(8),
        "consumer read 7 and wrote 7+1"
    );
    assert_eq!(run.ran_count(), 2);
    assert!(run.error(a).is_none() && run.error(b).is_none());
}

#[tokio::test]
async fn upstream_error_retires_skipped_reads_without_harming_live_readers() {
    let mut prog = ProgramBuilder::default();
    let failing = async_lambda!(|_| { Err(internals::failure("boom")) });
    let skipped = async_lambda!(|Invocation { outputs, .. }| {
        outputs[0] = DynamicValue::Static(ConstValue::Int(1));
        Ok(())
    });
    // `healthy` retains nothing, so an emptied slot below is the mid-run
    // release and not an eviction the loop does not perform.
    let healthy = prog.node().outputs(1).lambda(producer()).add();
    let failed = prog
        .node()
        .cache(CacheMode::Ram)
        .outputs(1)
        .lambda(failing)
        .add();
    let blocked = prog
        .node()
        .cache(CacheMode::Ram)
        .input(healthy.out(0))
        .input(failed.out(0))
        .outputs(1)
        .lambda(skipped)
        .add();
    let surviving = prog
        .node()
        .cache(CacheMode::Ram)
        .input(healthy.out(0))
        .outputs(1)
        .lambda(relay())
        .add();

    let mut run = prog.runs().readers([2, 1, 0, 0]);
    run.go().await;

    assert!(
        run.outputs(failed).is_none(),
        "an errored node's output is dropped (so it re-runs)"
    );
    assert!(
        run.outputs(blocked).is_none(),
        "the dependent is skipped, producing nothing"
    );
    assert_eq!(
        run.output_i64(surviving, 0),
        Some(7),
        "retiring the blocked read leaves the healthy value for its live reader"
    );
    assert!(
        run.outputs(healthy).is_none(),
        "the healthy non-RAM producer is reclaimed after the live reader lands"
    );
    assert!(run.error(failed).unwrap().to_string().contains("boom"));
    assert!(run.error(blocked).unwrap().to_string().contains("upstream"));
}

#[tokio::test]
async fn cancellation_retires_reads_owned_by_the_unreached_tail() {
    let mut prog = ProgramBuilder::default();
    let cancel = async_lambda!(|Invocation { ctx, .. }| {
        ctx.cancel_flag().cancel();
        Ok(())
    });
    let pending = async_lambda!(|_| { panic!("a cancelled tail consumer must not run") });
    let source = prog.node().outputs(1).lambda(producer()).add();
    prog.node().outputs(0).lambda(cancel).add();
    prog.node()
        .input(source.out(0))
        .outputs(0)
        .lambda(pending)
        .add();

    let mut run = prog.runs().readers([1]).cancel(CancelToken::new());
    run.go().await;

    assert!(run.cancelled());
    assert_eq!(
        run.remaining_reads(source, 0),
        0,
        "the pending consumer's read was retired"
    );
    assert!(
        run.outputs(source).is_none(),
        "tail retirement reclaims the source before the engine's final sweep"
    );
}

#[tokio::test]
async fn unbound_output_errors_only_when_demanded() {
    let mut prog = ProgramBuilder::default();
    let silent = async_lambda!(|_| { Ok(()) });
    let a = prog
        .node()
        .cache(CacheMode::Ram)
        .outputs(2)
        .lambda(silent)
        .add();
    let b = prog
        .node()
        .cache(CacheMode::Ram)
        .input(a.out(0))
        .input(a.out(1))
        .outputs(1)
        .lambda(async_lambda!(|Invocation { outputs, .. }| {
            outputs[0] = DynamicValue::Static(ConstValue::Int(1));
            Ok(())
        }))
        .add();

    let mut run = prog.runs().readers([1, 1, 0]);
    run.go().await;

    assert!(run.outputs(a).is_none());
    assert!(run.outputs(b).is_none());
    assert!(matches!(
        run.error(a),
        Some(RunError::OutputsNotProduced { outputs, .. }) if outputs == &[0, 1]
    ));
    assert!(matches!(
        run.error(b),
        Some(RunError::SkippedUpstream { .. })
    ));

    // Undemanded, the same silent lambda is no error at all: the port stays
    // `Unbound` and the node is reported clean.
    let mut prog = ProgramBuilder::default();
    let skipped = prog
        .node()
        .cache(CacheMode::Ram)
        .outputs(1)
        .lambda(async_lambda!(|_| { Ok(()) }))
        .add();

    let mut run = prog.runs().readers([0]);
    run.go().await;

    assert!(run.error(skipped).is_none());
    assert!(matches!(
        run.outputs(skipped).unwrap(),
        [DynamicValue::Unbound]
    ));
}

/// Retention after a run is the cache mode's call, not the reader count's.
/// `Executor::run` does no end-of-run eviction, so an emptied slot here is the
/// *mid-run* release and nothing else: a `None` producer is dropped the moment
/// its last consumer reads it — and, with no consumer at all, the moment it
/// finishes — while a `Ram` producer survives both.
///
/// `consumed` picks the shape, since a reader count has to match the wiring it
/// describes: wired is `a(mode) → b(Ram)` with readers `[1, 1]` (b reads a, and
/// a phantom consumer keeps b resident), unwired is two independent producers
/// with readers `[0, 0]`.
#[tokio::test]
async fn ram_retention_survives_the_last_read_and_none_does_not() {
    for (mode, consumed) in [
        (CacheMode::None, true),
        (CacheMode::Ram, true),
        (CacheMode::None, false),
        (CacheMode::Ram, false),
    ] {
        let mut prog = ProgramBuilder::default();
        let a = prog.node().cache(mode).outputs(1).lambda(producer()).add();
        let b = prog.node().cache(CacheMode::Ram).outputs(1);
        // A consumer reads a's 7 and writes 7+1, so b's value says the read
        // actually landed rather than merely that b ran.
        let (b, readers, expected_b) = if consumed {
            (b.input(a.out(0)).lambda(increment()), [1, 1], 8)
        } else {
            (b.lambda(producer()), [0, 0], 7)
        };
        let b = b.add();

        let mut run = prog.runs().readers(readers);
        run.go().await;

        assert_eq!(
            run.output_i64(a, 0).is_some(),
            mode.caches_in_ram(),
            "{mode:?} producer, consumed={consumed}: residency must equal caches_in_ram()"
        );
        if mode.caches_in_ram() {
            assert_eq!(run.output_i64(a, 0), Some(7), "{mode:?} keeps its own 7");
        }
        assert_eq!(
            run.output_i64(b, 0),
            Some(expected_b),
            "{mode:?}, consumed={consumed}: B (Ram) is kept hot regardless"
        );
    }
}

/// A lambda can name the execution node it is running as — the hook a host-side
/// sink (an editor value view, a per-node logger) needs to attribute what it
/// receives. Set per invoke, so two nodes sharing one lambda still report
/// distinct identities, and cleared once the run is over.
#[tokio::test]
async fn a_lambda_reads_the_execution_node_it_is_running_as() {
    use std::sync::Mutex;

    let seen: Arc<Mutex<Vec<NodeId>>> = Arc::new(Mutex::new(Vec::new()));
    let mut prog = ProgramBuilder::default();
    let announce = |seen: &Arc<Mutex<Vec<NodeId>>>, wrote: i64| {
        let probe = Arc::clone(seen);
        async_lambda!(
            move |Invocation { ctx, outputs, .. }| { seen = Arc::clone(&probe) } => {
                seen.lock().unwrap().push(ctx.current_node());
                outputs[0] = DynamicValue::Static(ConstValue::Int(wrote));
                Ok(())
            }
        )
    };
    let a = prog.node().outputs(1).lambda(announce(&seen, 1)).add();
    let b = prog.node().outputs(1).lambda(announce(&seen, 2)).add();

    prog.runs().go().await;

    // Index order is id order, and placement mints ascending ids, so `a` runs first.
    assert_eq!(*seen.lock().unwrap(), vec![a.node_id, b.node_id]);
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
    let mut prog = ProgramBuilder::default();
    let a = prog
        .node()
        .outputs(1)
        .lambda(async_lambda!(
            move |Invocation { demand, outputs, .. }| { seen = Arc::clone(&probe_seen) } => {
                *seen.lock().unwrap() = Some(demand[0]);
                outputs[0] = DynamicValue::Static(ConstValue::Int(7));
                Ok(())
            }
        ))
        .add();

    // Unseeded root, no consumers: the lambda reads `Skip` and the slot is
    // reclaimed the instant it's stored.
    let mut unseeded = prog.runs().readers([0]);
    unseeded.go().await;
    assert_eq!(*seen.lock().unwrap(), Some(OutputDemand::Skip));
    assert!(
        unseeded.outputs(a).is_none(),
        "unseeded Skip root is drained at store time: {:?}",
        unseeded.outputs(a)
    );

    let mut seeded = prog.runs().readers([0]).demand(a, 0).seeded(a);
    seeded.go().await;
    assert_eq!(*seen.lock().unwrap(), Some(OutputDemand::Produce));
    assert!(
        seeded.outputs(a).is_none(),
        "targeting controls demand, not RAM retention: {:?}",
        seeded.outputs(a)
    );
}

/// A reused node whose demanded output no consumer reads is released as soon as
/// it is served, rather than held to end-of-run eviction.
#[tokio::test]
async fn a_reused_output_with_no_consumers_is_reclaimed_immediately() {
    let mut prog = ProgramBuilder::default();
    // Only a `Pure` node earns a digest, and the run loop serves a `Reuse` by asking the
    // cache for the value — so the fixture has to be the coherent state a resolver `Reuse`
    // implies: a resident snapshot produced under the node's current digest.
    let a = prog
        .node()
        .pure()
        .cache(CacheMode::Disk)
        .outputs(1)
        .lambda(async_lambda!(|_| {
            panic!("a reused node must not invoke its lambda")
        }))
        .add();

    let mut run = prog
        .runs()
        .readers([0])
        .demand(a, 0)
        .state(a, NodeState::Reuse)
        .cached(a, [value(7)]);
    run.go().await;

    assert!(run.reused(a));
    assert!(
        run.outputs(a).is_none(),
        "the reused value is released as soon as it is served"
    );
}

/// A reused consumer contributes no reader. The shared non-RAM producer is reclaimed as
/// soon as the one running consumer reads it, without waiting for end-of-run eviction.
#[tokio::test]
async fn reused_consumer_does_not_delay_last_read_reclamation() {
    let mut prog = ProgramBuilder::default();
    let a = prog.node().pure().outputs(1).lambda(producer()).add();
    // Impure and retaining nothing, so it recomputes every run…
    let live = prog.node().input(a.out(0)).outputs(1).lambda(relay()).add();
    // …while its pure, RAM-retaining sibling is a reuse hit on the second run.
    let cached = prog
        .node()
        .pure()
        .cache(CacheMode::Ram)
        .input(a.out(0))
        .outputs(1)
        .lambda(relay())
        .add();

    let mut run = prog.runs().resolved();
    run.go().await;
    assert_eq!(run.ran_count(), 3);

    run.go().await;
    assert!(
        run.reused(cached),
        "the pure RAM consumer reuses its first-run result"
    );
    assert!(
        run.ran(a) && run.ran(live),
        "the producer and impure consumer still run"
    );
    assert!(
        run.outputs(a).is_none(),
        "the producer is reclaimed immediately after its only live reader"
    );
}

/// A node whose func has no implementation attached can't execute: it's reported as
/// its own per-node [`RunError::MissingLambda`] (not silently skipped), any stale
/// cached value is dropped so it can't be served as this run's result, and its
/// consumers skip with the usual errored-upstream propagation.
#[tokio::test]
async fn missing_lambda_reports_error_and_skips_consumers() {
    let mut prog = ProgramBuilder::default();
    let source = prog
        .node()
        .cache(CacheMode::Ram)
        .outputs(1)
        .lambda(producer())
        .add();
    // No lambda at all — the declaration a library lost its implementation for.
    let missing = prog
        .node()
        .cache(CacheMode::Ram)
        .input(source.out(0))
        .outputs(1)
        .add();
    let downstream = prog
        .node()
        .cache(CacheMode::Ram)
        .input(missing.out(0))
        .outputs(1)
        .lambda(relay())
        .add();

    let mut run = prog
        .runs()
        .resolved()
        .only_root(downstream)
        .resident(missing, [value(9)]);
    run.go().await;

    assert_eq!(
        run.ran_count(),
        0,
        "the source is cut, the missing implementation errors, and its consumer skips"
    );
    assert!(
        run.outputs(missing).is_none(),
        "the missing node's stale value is dropped, not served"
    );
    assert!(
        matches!(run.error(missing), Some(RunError::MissingLambda { .. })),
        "the node reports its missing implementation: {:?}",
        run.error(missing)
    );
    assert!(
        matches!(
            run.error(downstream),
            Some(RunError::SkippedUpstream { .. })
        ),
        "the consumer skips as errored-upstream: {:?}",
        run.error(downstream)
    );
}

/// A consumer whose digest is unchanged serves its cached value even when the
/// shared upstream re-ran for a *different* consumer and failed: the reuse verdict
/// is checked before the errored-dependency skip, so the valid cache is neither
/// cleared nor blamed for the upstream failure.
#[tokio::test]
async fn reuse_survives_failed_upstream_rerun() {
    let mut prog = ProgramBuilder::default();
    // A succeeds once (with 5), then fails every later invocation. Content-cacheable
    // throughout — the fixture default is `Impure`, which has no digest and so could
    // never be the hit under test.
    let a = prog
        .node()
        .pure()
        .outputs(1)
        .lambda(async_lambda!(|Invocation { state, outputs, .. }| {
            if state.get::<bool>().is_some() {
                return Err(internals::failure("transient failure"));
            }
            state.set(true);
            outputs[0] = DynamicValue::Static(ConstValue::Int(5));
            Ok(())
        }))
        .add();
    // Only B retains RAM; A and C recompute every run.
    let b = prog
        .node()
        .pure()
        .cache(CacheMode::Ram)
        .input(a.out(0))
        .outputs(1)
        .lambda(increment())
        .add();
    let c = prog
        .node()
        .pure()
        .input(a.out(0))
        .outputs(1)
        .lambda(increment())
        .add();

    let mut run = prog.runs().resolved();

    // Run 1: A=5, B=C=6, everything computes.
    run.go().await;
    assert_eq!(run.ran_count(), 3);
    assert_eq!(run.output_i64(b, 0), Some(6));

    // Run 2: A re-runs (nothing cached it) and fails. B's digest is unchanged, so it
    // is served as cached — not skipped — and its resident 6 survives. C recomputes,
    // sees the errored upstream, and is skipped.
    run.go().await;
    assert!(run.reused(b), "B is a reuse hit despite A's failure");
    assert_eq!(
        run.output_i64(b, 0),
        Some(6),
        "B's valid cached value survives the sibling failure"
    );
    assert!(run.error(a).is_some(), "A's own failure is reported");
    assert!(
        run.error(c).is_some(),
        "C is skipped for the errored upstream"
    );
    assert!(run.error(b).is_none(), "B carries no error");
}
