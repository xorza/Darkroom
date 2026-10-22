//! The cache-aware sweep over planned schedules, each primed through the
//! [`Sweep`](crate::internals::program::sweep::Sweep) fixture.

use crate::data::dynamic_value::DynamicValue;
use crate::exec::compile::compiled_graph::ExecBinding;
use crate::exec::schedule::NodeState;
use crate::graph::func::lambda::OutputDemand;
use crate::internals::program::ProgramBuilder;

#[tokio::test]
async fn exact_demand_accepts_narrow_producer_cache_and_ignores_reused_reader() {
    let mut prog = ProgramBuilder::default();
    let source = prog.node().pure().outputs(2).add();
    let cached = prog.node().pure().input(source.out(1)).outputs(1).add();
    let live = prog.node().pure().input(source.out(0)).outputs(1).add();
    let sink = prog
        .node()
        .pure()
        .input(cached.out(0))
        .input(live.out(0))
        .outputs(0)
        .add();

    let run = prog
        .sweep()
        .root(sink)
        .cached(source, [ProgramBuilder::value(7), DynamicValue::Unbound])
        .cached(cached, [ProgramBuilder::value(8)])
        .run()
        .await;

    assert_eq!(run.state(source), NodeState::Reuse);
    assert_eq!(run.state(cached), NodeState::Reuse);
    assert_eq!(run.state(live), NodeState::Run);
    assert_eq!(run.state(sink), NodeState::Run);
    assert_eq!(
        run.demand(source),
        &[OutputDemand::Produce, OutputDemand::Skip]
    );
    assert_eq!(run.readers(source), &[1, 0]);
}

#[tokio::test]
async fn missing_input_stops_liveness_before_its_producer() {
    let mut prog = ProgramBuilder::default();
    let source = prog.node().pure().outputs(1).add();
    let blocked = prog
        .node()
        .pure()
        .input(source.out(0))
        .required(ExecBinding::None)
        .outputs(0)
        .add();

    let run = prog.sweep().root(blocked).missing(blocked).run().await;

    assert_eq!(run.state(source), NodeState::Cut);
    assert_eq!(
        run.state(blocked),
        NodeState::MissingInputs,
        "a blocked root keeps the planner's verdict — the sweep refines only \
         runnable nodes, so the reason it did not run survives to the outcome"
    );
    assert_eq!(run.demand(source), &[OutputDemand::Skip]);
    assert_eq!(run.readers(source), &[0]);
}

/// A node seed demands every output it has, without any consumer reading them —
/// the "run to this node" semantic, distinct from demand arriving through a
/// binding.
#[tokio::test]
async fn a_node_seed_demands_every_output_without_readers() {
    let mut prog = ProgramBuilder::default();
    let unseeded = prog.node().pure().outputs(2).add();
    let seeded = prog.node().pure().outputs(2).add();

    let run = prog
        .sweep()
        .root(unseeded)
        .root(seeded)
        .seeded(seeded)
        .run()
        .await;

    assert_eq!(
        run.demand(unseeded),
        &[OutputDemand::Skip, OutputDemand::Skip],
        "a root nobody reads and nobody seeded produces nothing"
    );
    assert_eq!(
        run.demand(seeded),
        &[OutputDemand::Produce, OutputDemand::Produce]
    );
    assert!(
        run.schedule
            .outputs
            .readers
            .iter()
            .all(|readers| *readers == 0)
    );
}

#[tokio::test]
async fn cone_reachable_only_through_a_reuse_hit_is_fully_pruned() {
    let mut prog = ProgramBuilder::default();
    let deep = prog.node().pure().outputs(1).add();
    let source = prog.node().pure().input(deep.out(0)).outputs(1).add();
    let cached = prog.node().pure().input(source.out(0)).outputs(1).add();
    let sink = prog.node().pure().input(cached.out(0)).outputs(0).add();

    let run = prog
        .sweep()
        .root(sink)
        .cached(cached, [ProgramBuilder::value(1)])
        .run()
        .await;

    assert_eq!(run.state(deep), NodeState::Cut);
    assert_eq!(run.state(source), NodeState::Cut);
    assert_eq!(run.state(cached), NodeState::Reuse);
    assert_eq!(run.state(sink), NodeState::Run);
    assert_eq!(
        run.readers(source),
        &[0],
        "a cut producer owns no read for the reuse that replaced it"
    );
}
