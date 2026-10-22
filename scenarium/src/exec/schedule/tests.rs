//! The structural pass over the [`RunSchedule`](crate::exec::schedule::RunSchedule),
//! over bare programs, and the queries a planned schedule answers.

use crate::data::const_value::ConstValue;
use crate::exec::compile::compiled_graph::ExecBinding;
use crate::exec::error::ExecError;
use crate::exec::idx::{InputIdx, NodeIdx, OutputAddr, OutputIdx};
use crate::exec::schedule::planner::Planner;
use crate::exec::schedule::{Delivery, NodeState, RootFlags, RunSchedule};
use crate::exec::seeds::RunSeeds;
use crate::graph::func::lambda::OutputDemand;
use crate::graph::identity::{EventPort, NodeId};
use crate::internals::program::ProgramBuilder;

#[test]
#[cfg(debug_assertions)]
fn reader_overflow_trips_the_debug_invariant() {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use crate::exec::schedule::ResolvedOutputs;

    let mut outputs = ResolvedOutputs::default();
    outputs.reset(1);
    outputs.readers[OutputIdx(0)] = u32::MAX;

    assert!(
        catch_unwind(AssertUnwindSafe(|| outputs.add_reader(OutputIdx(0)))).is_err(),
        "a graph cannot have more readers than the counter represents"
    );
}

/// The planner opens the output half too: a schedule it just filled carries a
/// zeroed demand and reader column spanning the program's output pool, so no
/// previous run's counts can survive into one the sweep has not touched.
#[test]
fn planning_resets_the_output_half_to_the_programs_pool() {
    let mut prog = ProgramBuilder::default();
    let a = prog.node().outputs(2).add();
    prog.node().sink().input(a.out(0)).outputs(1).add();

    let mut planner = Planner::default();
    let mut schedule = RunSchedule::default();
    // A stale resolution from an earlier, differently shaped run.
    schedule.outputs.reset(1);
    schedule.outputs.add_reader(OutputIdx(0));

    planner
        .plan(prog.program(), &RunSeeds::sinks(), &mut schedule)
        .expect("no cycle");

    assert_eq!(
        schedule.outputs.demand.len(),
        3,
        "two outputs plus the sink's"
    );
    assert_eq!(schedule.outputs.readers.len(), 3);
    assert!(
        schedule
            .outputs
            .demand
            .iter()
            .all(|demand| *demand == OutputDemand::Skip)
    );
    assert!(schedule.outputs.readers.iter().all(|readers| *readers == 0));
}

#[test]
fn chain_orders_deps_before_consumers_and_schedules_all() {
    // A → B → C (C sink). Every reachable node is scheduled — the planner is
    // structural, so nothing is pruned as "cached" here (that's the executor's call).
    let mut prog = ProgramBuilder::default();
    let a = prog.node().outputs(1).add();
    let b = prog.node().input(a.out(0)).outputs(1).add();
    let c = prog.node().sink().input(b.out(0)).outputs(1).add();

    let mut p = prog.plan_sinks();
    p.validate(prog.program()).unwrap();
    assert_eq!(
        p.process_order,
        [a, b, c].map(|node| node.node_idx),
        "post-order: deps first"
    );
    for idx in [a, b, c] {
        assert!(p.states[idx.node_idx].is_runnable());
        assert!(!p.states[idx.node_idx].missing_required_inputs());
    }

    p.process_order.swap(0, 1);
    assert_eq!(
        p.validate(prog.program()).unwrap_err().to_string(),
        format!(
            "execution node {:?} appears before dependency {:?}",
            b.node_id, a.node_id
        )
    );
    p.states[a.node_idx] = NodeState::Disabled;
    assert_eq!(
        p.validate(prog.program()).unwrap_err().to_string(),
        format!(
            "execution node {:?} appears before dependency {:?}",
            b.node_id, a.node_id
        ),
        "a disabled verdict cannot hide an enabled dependency"
    );
    p.process_order.swap(0, 1);
    // Back to what the planner actually wrote — not `default()`, which is the
    // `Unvisited` fill and would leave a scheduled node undecided.
    p.states[a.node_idx] = NodeState::Cut;
    p.validate(prog.program())
        .expect("restored to a valid plan");

    // Scheduling a node and deciding its state are one act, so a state may not
    // be decided for a node the schedule left out.
    let dropped = p
        .process_order
        .pop()
        .expect("the chain scheduled three nodes");
    assert_eq!(dropped, c.node_idx);
    assert_eq!(
        p.validate(prog.program()).unwrap_err().to_string(),
        format!("unscheduled node {:?} was decided Cut", c.node_id)
    );
    p.process_order.push(dropped);

    // The validator reports corruption rather than faulting on it: a binding
    // target past the last node must not index `seen_in_order` out of range.
    let past_the_end = NodeIdx(prog.program_mut().e_nodes.len() as u32);
    let b_input = prog.program()[b.node_idx].inputs.nth(0);
    prog.program_mut().inputs[b_input].binding = ExecBinding::Wire(OutputAddr {
        node_idx: past_the_end,
        port_idx: 0,
    });
    assert_eq!(
        p.validate(prog.program()).unwrap_err().to_string(),
        format!("execution order contains an out-of-range node index: {past_the_end:?}")
    );

    // Likewise for a set that no longer spans the program.
    prog.program_mut().inputs[b_input].binding = a.out(0);
    p.root_flags_mut().reset(0, RootFlags::default());
    assert_eq!(
        p.validate(prog.program()).unwrap_err().to_string(),
        "schedule root flags spans 0 entries, not the program's 3"
    );
}

/// The other direction of the same invariant: reading a state the walk never
/// settled is a broken schedule, so it fails loudly instead of answering as if
/// the node were merely blocked. Checked on `input_missing`, the one such read
/// that takes the column directly.
#[test]
fn reading_an_unvisited_producer_panics() {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    let mut prog = ProgramBuilder::default();
    let producer = prog.node().outputs(1).add();
    let consumer = prog.node().input(producer.out(0)).outputs(1).add();

    let mut schedule = RunSchedule::default();
    schedule.reset_for_program(prog.program());
    let consumer_input = &prog.program().inputs[prog.program()[consumer.node_idx].inputs][0];

    assert!(
        catch_unwind(AssertUnwindSafe(
            || schedule.input_missing(prog.program(), consumer_input)
        ))
        .is_err(),
        "an unvisited producer is a broken schedule, not an unsatisfied input"
    );
}

/// What an input delivers is decided by its binding, its producer's state
/// and its overrider, and `input_missing` reads the same answer: a bound
/// input follows its producer, a disabled producer leaves it unbound, a
/// blocked one blocks it, and an overrider that delivers sets its target
/// aside — never missing, though the target is required.
#[test]
fn delivery_follows_binding_producer_state_and_override() {
    fn answer<'a>(
        prog: &'a ProgramBuilder,
        schedule: &RunSchedule,
        port: InputIdx,
    ) -> (Delivery<'a>, bool) {
        let input = &prog.program().inputs[port];
        (
            schedule.delivery(prog.program(), input),
            schedule.input_missing(prog.program(), input),
        )
    }

    let mut prog = ProgramBuilder::default();
    let producer = prog.node().outputs(1).add();
    let consumer = prog
        .node()
        .required(producer.out(0))
        .required(ExecBinding::Const(5i64.into()))
        .input(ExecBinding::None)
        .required(ExecBinding::None)
        .outputs(1)
        .add();
    let ports = prog.program()[consumer.node_idx].inputs;
    prog.program_mut().inputs[ports.nth(1)].overridden_by = Some(ports.nth(2));
    let addr = OutputAddr {
        node_idx: producer.node_idx,
        port_idx: 0,
    };
    let five = ConstValue::Int(5);

    let mut schedule = RunSchedule::default();
    schedule.reset_for_program(prog.program());

    // The bound input against each settled producer state.
    for (state, expected) in [
        (NodeState::Cut, (Delivery::Wire(addr), false)),
        (NodeState::Reuse, (Delivery::Wire(addr), false)),
        (NodeState::Run, (Delivery::Wire(addr), false)),
        (NodeState::Disabled, (Delivery::Unbound, true)),
        (NodeState::MissingInputs, (Delivery::Blocked, true)),
    ] {
        schedule.states[producer.node_idx] = state;
        assert_eq!(
            answer(&prog, &schedule, ports.nth(0)),
            expected,
            "{state:?}"
        );
    }
    assert_eq!(
        answer(&prog, &schedule, ports.nth(3)),
        (Delivery::Unbound, true)
    );

    // The target against each overrider: only a delivered value sets it
    // aside — an explicit `Null`, or a producer the run skips, does not.
    schedule.states[producer.node_idx] = NodeState::Run;
    for (overrider, expected) in [
        (ExecBinding::None, Delivery::Const(&five)),
        (ExecBinding::Const(ConstValue::Null), Delivery::Const(&five)),
        (ExecBinding::Const(ConstValue::Int(1)), Delivery::SetAside),
        (producer.out(0), Delivery::SetAside),
    ] {
        prog.program_mut().inputs[ports.nth(2)].binding = overrider.clone();
        assert_eq!(
            answer(&prog, &schedule, ports.nth(1)),
            (expected, false),
            "{overrider:?}"
        );
    }
    schedule.states[producer.node_idx] = NodeState::Disabled;
    assert_eq!(
        answer(&prog, &schedule, ports.nth(1)),
        (Delivery::Const(&five), false),
        "a disabled overrider delivers nothing"
    );
}

/// An unbound input blocks its node only when the declaration called it
/// required — and when it does, the verdict travels down every consumer.
/// Optionality is the only difference between the two rows, so it is the
/// only thing that can decide them.
#[test]
fn only_a_required_unbound_input_blocks_the_node_and_its_dependents() {
    for required in [true, false] {
        let mut prog = ProgramBuilder::default();
        let node = prog.node().outputs(1);
        let a = if required {
            node.required(ExecBinding::None)
        } else {
            node.input(ExecBinding::None)
        }
        .add();
        let b = prog.node().sink().input(a.out(0)).outputs(1).add();

        let p = prog.plan_sinks();
        for idx in [a, b] {
            assert_eq!(
                p.states[idx.node_idx].missing_required_inputs(),
                required,
                "node {idx:?} with required={required}"
            );
            assert_eq!(
                p.states[idx.node_idx].is_runnable(),
                !required,
                "node {idx:?} with required={required}"
            );
        }
        if !required {
            assert_eq!(
                p.process_order,
                [a, b].map(|node| node.node_idx),
                "an optional unbound input still schedules the whole chain"
            );
        }
    }
}

#[test]
fn explicit_seed_overrides_disabled_dependency_for_this_run() {
    let mut prog = ProgramBuilder::default();
    let producer = prog.node().outputs(1).add();
    prog.program_mut().by_id_mut(producer.node_id).disabled = true;
    let required = prog
        .node()
        .sink()
        .required(producer.out(0))
        .outputs(1)
        .add();
    let optional = prog.node().sink().input(producer.out(0)).outputs(1).add();

    let mut planner = Planner::default();
    let mut plan = RunSchedule::default();
    planner
        .plan(prog.program(), &RunSeeds::sinks(), &mut plan)
        .unwrap();
    assert_eq!(plan.states[producer.node_idx], NodeState::Disabled);
    assert_eq!(plan.states[required.node_idx], NodeState::MissingInputs);
    assert_eq!(plan.states[optional.node_idx], NodeState::Cut);

    planner
        .plan(
            prog.program(),
            &RunSeeds {
                node_ids: vec![producer.node_id],
                ..RunSeeds::sinks()
            },
            &mut plan,
        )
        .unwrap();
    for node_id in [producer, required, optional] {
        assert_eq!(
            plan.states[node_id.node_idx],
            NodeState::Cut,
            "the explicit producer seed makes every consumer runnable"
        );
    }
}

#[test]
#[should_panic(expected = "a compiled program is acyclic")]
fn a_dependency_cycle_is_a_broken_contract() {
    // A binds B, B binds A (A sink) — the planner must stop, not loop.
    // The forward reference is spelled by index, since the node it names
    // does not exist until the line after.
    let at = |node_idx: u32| {
        ExecBinding::Wire(OutputAddr {
            node_idx: NodeIdx(node_idx),
            port_idx: 0,
        })
    };
    let mut prog = ProgramBuilder::default();
    prog.node().sink().input(at(1)).outputs(1).add();
    prog.node().input(at(0)).outputs(1).add();

    prog.try_plan(&RunSeeds::sinks()).ok();
}

#[test]
fn node_seed_schedules_only_its_cone_and_pins_it() {
    // A → B → C (C sink). Seeding node B (by authoring id, which resolves
    // straight against the program) schedules only [A, B] — C is upstream of nothing
    // seeded — and records B as both a root and a seeded node. B's output has no
    // scheduled consumer.
    let mut prog = ProgramBuilder::default();
    let a = prog.node().outputs(1).add();
    let b = prog.node().input(a.out(0)).outputs(1).add();
    let c = prog.node().sink().input(b.out(0)).outputs(1).add();

    // One planner and one schedule across the calls below: each plan has
    // to start from nothing, whatever the last one left.
    let mut planner = Planner::default();
    let mut p = RunSchedule::default();
    planner
        .plan(prog.program(), &RunSeeds::nodes(vec![b.node_id]), &mut p)
        .expect("no cycle");

    assert_eq!(
        p.process_order,
        [a, b].map(|node| node.node_idx),
        "only B's cone, deps first"
    );
    assert_eq!(p.roots().collect::<Vec<_>>(), [b.node_idx]);
    assert_eq!(p.seeded_roots(), vec![b.node_idx]);
    assert!(p.states[a.node_idx].is_runnable());
    assert!(p.states[b.node_idx].is_runnable());
    assert_eq!(
        p.states[c.node_idx],
        NodeState::Unvisited,
        "C is upstream of nothing seeded, so the walk never reached it"
    );

    // The same seed named twice is still one root: the seed pass marks, it
    // does not append.
    let seeds = RunSeeds::nodes(vec![b.node_id, b.node_id]);
    planner
        .plan(prog.program(), &seeds, &mut p)
        .expect("no cycle");
    assert_eq!(
        p.roots().collect::<Vec<_>>(),
        [b.node_idx],
        "a repeated seed is one root"
    );
    assert_eq!(p.seeded_roots(), vec![b.node_idx]);
    assert_eq!(
        p.process_order,
        [a, b].map(|node| node.node_idx),
        "and schedules the same cone"
    );

    // Seeds named against the program's order still list their roots
    // ascending, with no sinks or event sources to sweep: the order the
    // host named them in must not reach the schedule.
    let seeds = RunSeeds::nodes(vec![b.node_id, a.node_id]);
    planner
        .plan(prog.program(), &seeds, &mut p)
        .expect("no cycle");
    assert_eq!(p.roots().collect::<Vec<_>>(), [a.node_idx, b.node_idx]);
    assert_eq!(p.seeded_roots(), vec![a.node_idx, b.node_idx]);

    // Node seeds combine with sinks: the same seed plus `sinks` schedules
    // everything, and B stays seeded.
    let seeds = RunSeeds {
        node_ids: vec![b.node_id],
        ..RunSeeds::sinks()
    };
    planner
        .plan(prog.program(), &seeds, &mut p)
        .expect("no cycle");
    assert_eq!(p.process_order, [a, b, c].map(|node| node.node_idx));
    assert_eq!(p.seeded_roots(), vec![b.node_idx]);

    // Seeding the sink itself is the case two root passes reach one node: the
    // seed pass marks C seeded, then the sinks sweep marks it a root again.
    // The second visit must *add* to what the first left — it is one root
    // carrying both facts, so C is listed once and stays seeded.
    let seeds = RunSeeds {
        node_ids: vec![c.node_id],
        ..RunSeeds::sinks()
    };
    planner
        .plan(prog.program(), &seeds, &mut p)
        .expect("no cycle");
    assert_eq!(
        p.roots().collect::<Vec<_>>(),
        [c.node_idx],
        "reached twice, listed once"
    );
    assert_eq!(
        p.seeded_roots(),
        vec![c.node_idx],
        "the sinks sweep must not drop the seed the node already carried"
    );

    // A seed id absent from the program is inconsistent caller state — a hard failure,
    // not a silent skip.
    let bogus = NodeId::from_u128(0xdead_beef);
    let seeds = RunSeeds::nodes(vec![bogus]);
    let err = planner.plan(prog.program(), &seeds, &mut p).unwrap_err();
    assert!(matches!(err, ExecError::NodeSeedNotFound { node_id } if node_id == bogus));
}

#[test]
fn event_seed_schedules_subscribers_and_rejects_missing_ports() {
    let mut prog = ProgramBuilder::default();
    let subscriber = prog.node().outputs(0).add();
    let emitter = prog.node().outputs(0).event([subscriber]).add();

    let event = EventPort {
        node_id: emitter.node_id,
        event_idx: 0,
    };
    let mut planner = Planner::default();
    let mut plan = RunSchedule::default();
    planner
        .plan(prog.program(), &RunSeeds::events(vec![event]), &mut plan)
        .unwrap();
    assert_eq!(plan.roots().collect::<Vec<_>>(), [subscriber.node_idx]);
    assert_eq!(plan.process_order, [subscriber].map(|node| node.node_idx));
    assert!(plan.event_source_roots().is_empty());

    planner
        .plan(
            prog.program(),
            &RunSeeds {
                event_sources: true,
                ..Default::default()
            },
            &mut plan,
        )
        .unwrap();
    assert_eq!(plan.roots().collect::<Vec<_>>(), [emitter.node_idx]);
    assert_eq!(plan.event_source_roots(), vec![emitter.node_idx]);
    assert_eq!(plan.process_order, [emitter].map(|node| node.node_idx));

    // Seeded *and* an event source: the two properties are independent, so a
    // node reached as both keeps both — one root carrying two facts.
    planner
        .plan(
            prog.program(),
            &RunSeeds {
                event_sources: true,
                node_ids: vec![emitter.node_id],
                ..Default::default()
            },
            &mut plan,
        )
        .unwrap();
    assert_eq!(plan.roots().collect::<Vec<_>>(), [emitter.node_idx]);
    assert_eq!(plan.seeded_roots(), vec![emitter.node_idx]);
    assert_eq!(plan.event_source_roots(), vec![emitter.node_idx]);
}
