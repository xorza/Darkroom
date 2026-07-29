use crate::execution::error::Error;
use crate::execution::identity::{ExecutionEventPort, ExecutionNodeId};
use crate::execution::plan::{ExecutionPlan, NodeState, Planner, input_missing};
use crate::execution::program::index::{NodeColumn, NodeIdx, OutputAddr};
use crate::execution::program::{
    ExecutionBinding, ExecutionEvent, ExecutionInput, ExecutionNode, ExecutionOutput, Program,
};
use crate::execution::seeds::RunSeeds;
use crate::node::definition::FuncId;

/// Hand-built program for planner tests — scheduling is structural, so it
/// needs no compile artifact and no authoring attribution. Inputs are
/// `(required, binding)`.
#[derive(Default)]
struct Fix {
    program: Program,
}

impl Fix {
    fn node(
        &mut self,
        sink: bool,
        inputs: &[(bool, ExecutionBinding)],
        outputs: u32,
    ) -> ExecutionNodeId {
        let program = &mut self.program;
        let inputs = program
            .inputs
            .append(inputs.iter().map(|(required, binding)| ExecutionInput {
                required: *required,
                stamps_fs_path: false,
                binding: binding.clone(),
            }));
        let outputs = program
            .outputs
            .append((0..outputs).map(|_| ExecutionOutput::default()));
        let idx = program.e_nodes.len();
        let id = ExecutionNodeId::from_u128(idx as u128 + 1);
        program.push(
            id,
            ExecutionNode {
                sink,
                func_id: FuncId::from_u128(idx as u128 + 1),
                inputs,
                outputs,
                ..Default::default()
            },
        );
        id
    }
}

/// The fixture's id ↔ index invariant: ids are assigned `from_u128(idx + 1)`
/// in push order, so a node's dense index is recoverable from its id.
fn nx(e_node_id: ExecutionNodeId) -> NodeIdx {
    NodeIdx(e_node_id.as_uuid().as_u128() as u32 - 1)
}

fn bind(e_node_id: ExecutionNodeId, port: usize) -> ExecutionBinding {
    ExecutionBinding::Bind(OutputAddr {
        node_idx: nx(e_node_id),
        port_idx: port as u32,
    })
}

/// Plan `sinks` over `fix`. Purely structural — no cache state; the executor
/// decides cached-vs-recompute at run time.
fn plan(fix: &Fix) -> ExecutionPlan {
    let mut planner = Planner::default();
    let mut plan = ExecutionPlan::default();
    let seeds = RunSeeds {
        sinks: true,
        ..Default::default()
    };
    planner
        .plan(&fix.program, &seeds, &mut plan)
        .expect("no cycle");
    plan
}

#[test]
fn chain_orders_deps_before_consumers_and_schedules_all() {
    // A → B → C (C sink). Every reachable node is scheduled — the planner is
    // structural, so nothing is pruned as "cached" here (that's the executor's call).
    let mut f = Fix::default();
    let a = f.node(false, &[], 1);
    let b = f.node(false, &[(false, bind(a, 0))], 1);
    let c = f.node(true, &[(false, bind(b, 0))], 1);

    let mut p = plan(&f);
    p.validate(&f.program).unwrap();
    assert_eq!(p.process_order, [a, b, c].map(nx), "post-order: deps first");
    for idx in [a, b, c] {
        assert!(p.states[nx(idx)].is_runnable());
        assert!(!p.states[nx(idx)].missing_required_inputs());
    }

    p.process_order.swap(0, 1);
    assert_eq!(
        p.validate(&f.program).unwrap_err().to_string(),
        format!("execution node {b:?} appears before dependency {a:?}")
    );
    p.states[nx(a)] = NodeState::Disabled;
    assert_eq!(
        p.validate(&f.program).unwrap_err().to_string(),
        format!("execution node {b:?} appears before dependency {a:?}"),
        "a disabled verdict cannot hide an enabled dependency"
    );
    p.process_order.swap(0, 1);
    // Back to what the planner actually wrote — not `default()`, which is the
    // `Unvisited` fill and would leave a scheduled node undecided.
    p.states[nx(a)] = NodeState::Cut;
    p.validate(&f.program).expect("restored to a valid plan");

    // Scheduling a node and deciding its state are one act, so a state may not
    // be decided for a node the schedule left out.
    let dropped = p
        .process_order
        .pop()
        .expect("the chain scheduled three nodes");
    assert_eq!(dropped, nx(c));
    assert_eq!(
        p.validate(&f.program).unwrap_err().to_string(),
        format!("unscheduled node {c:?} was decided Cut")
    );
    p.process_order.push(dropped);

    // The validator reports corruption rather than faulting on it: a binding
    // target past the last node used to index `seen_in_order` out of range.
    let past_the_end = NodeIdx(f.program.e_nodes.len() as u32);
    let b_input = f.program[nx(b)].inputs.start as usize;
    f.program.inputs[b_input].binding = ExecutionBinding::Bind(OutputAddr {
        node_idx: past_the_end,
        port_idx: 0,
    });
    assert_eq!(
        p.validate(&f.program).unwrap_err().to_string(),
        format!("execution order contains an out-of-range node index: {past_the_end:?}")
    );

    // Likewise for a set that no longer spans the program.
    f.program.inputs[b_input].binding = bind(a, 0);
    p.seeded.reset(0);
    assert_eq!(
        p.validate(&f.program).unwrap_err().to_string(),
        "plan seeded spans 0 nodes, not the program's 3"
    );
}

/// The other direction of the same invariant: reading a state the walk never
/// settled is a broken schedule, so it fails loudly instead of answering as if
/// the node were merely blocked. Checked on `input_missing`, the one such read
/// that takes the column directly.
#[test]
fn reading_an_unvisited_producer_panics() {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    let mut f = Fix::default();
    let producer = f.node(false, &[], 1);
    let consumer = f.node(false, &[(false, bind(producer, 0))], 1);

    let mut states = NodeColumn::default();
    states.reset(f.program.e_nodes.len(), NodeState::Unvisited);
    let consumer_input = &f.program.inputs[f.program[nx(consumer)].inputs][0];

    assert!(
        catch_unwind(AssertUnwindSafe(|| input_missing(consumer_input, &states))).is_err(),
        "an unvisited producer is a broken schedule, not an unsatisfied input"
    );
}

#[test]
fn missing_required_input_blocks_node_and_dependents() {
    // A has a required *unbound* input ⇒ missing; B binds A ⇒ inherits missing.
    let mut f = Fix::default();
    let a = f.node(false, &[(true, ExecutionBinding::None)], 1);
    let b = f.node(true, &[(false, bind(a, 0))], 1);

    let p = plan(&f);
    for idx in [a, b] {
        assert!(
            p.states[nx(idx)].missing_required_inputs(),
            "node {idx:?} missing"
        );
        assert!(
            !p.states[nx(idx)].is_runnable(),
            "node {idx:?} not runnable"
        );
    }
}

#[test]
fn optional_unbound_input_does_not_block() {
    // An *optional* unbound input is fine — the node still runs.
    let mut f = Fix::default();
    let a = f.node(true, &[(false, ExecutionBinding::None)], 1);

    let p = plan(&f);
    assert!(!p.states[nx(a)].missing_required_inputs());
    assert!(p.states[nx(a)].is_runnable());
    assert_eq!(p.process_order, [a].map(nx));
}

#[test]
fn explicit_seed_overrides_disabled_dependency_for_this_run() {
    let mut f = Fix::default();
    let producer = f.node(false, &[], 1);
    f.program.by_id_mut(producer).disabled = true;
    let required = f.node(true, &[(true, bind(producer, 0))], 1);
    let optional = f.node(true, &[(false, bind(producer, 0))], 1);

    let mut planner = Planner::default();
    let mut plan = ExecutionPlan::default();
    planner
        .plan(
            &f.program,
            &RunSeeds {
                sinks: true,
                ..Default::default()
            },
            &mut plan,
        )
        .unwrap();
    assert_eq!(plan.states[nx(producer)], NodeState::Disabled);
    assert_eq!(plan.states[nx(required)], NodeState::MissingInputs);
    assert_eq!(plan.states[nx(optional)], NodeState::Cut);

    planner
        .plan(
            &f.program,
            &RunSeeds {
                sinks: true,
                e_node_ids: vec![producer],
                ..Default::default()
            },
            &mut plan,
        )
        .unwrap();
    for e_node_id in [producer, required, optional] {
        assert_eq!(
            plan.states[nx(e_node_id)],
            NodeState::Cut,
            "the explicit producer seed makes every consumer runnable"
        );
    }
}

#[test]
fn node_seed_is_both_a_root_and_seeded() {
    let mut f = Fix::default();
    let a = f.node(false, &[], 1);

    let mut planner = Planner::default();
    let mut p = ExecutionPlan::default();
    let seeds = RunSeeds {
        e_node_ids: vec![a],
        ..Default::default()
    };
    planner.plan(&f.program, &seeds, &mut p).expect("no cycle");

    assert_eq!(p.seeded.iter().collect::<Vec<_>>(), vec![nx(a)]);
    assert_eq!(p.roots.iter().collect::<Vec<_>>(), vec![nx(a)]);

    let seeds = RunSeeds {
        e_node_ids: vec![a, a],
        ..Default::default()
    };
    planner.plan(&f.program, &seeds, &mut p).expect("no cycle");
    assert_eq!(p.seeded.iter().collect::<Vec<_>>(), vec![nx(a)]);
    assert_eq!(p.roots.iter().collect::<Vec<_>>(), vec![nx(a)]);
}

#[test]
fn dependency_cycle_is_rejected() {
    // A binds B, B binds A (A sink) — the planner must error, not loop.
    let mut f = Fix::default();
    f.node(true, &[(false, bind(ExecutionNodeId::from_u128(2), 0))], 1);
    f.node(false, &[(false, bind(ExecutionNodeId::from_u128(1), 0))], 1);

    let mut planner = Planner::default();
    let mut plan = ExecutionPlan::default();
    let seeds = RunSeeds {
        sinks: true,
        ..Default::default()
    };
    let result = planner.plan(&f.program, &seeds, &mut plan);
    assert!(matches!(result, Err(Error::CycleDetected { .. })));
}

#[test]
fn node_seed_schedules_only_its_cone_and_pins_it() {
    // A → B → C (C sink). Seeding node B (by authoring id — top-level ids resolve
    // straight against the program) schedules only [A, B] — C is upstream of nothing
    // seeded — and records B as both a root and a seeded node. B's output has no
    // scheduled consumer.
    let mut f = Fix::default();
    let a = f.node(false, &[], 1);
    let b = f.node(false, &[(false, bind(a, 0))], 1);
    let c = f.node(true, &[(false, bind(b, 0))], 1);

    let mut planner = Planner::default();
    let mut p = ExecutionPlan::default();
    let seeds = RunSeeds {
        e_node_ids: vec![b],
        ..Default::default()
    };
    planner.plan(&f.program, &seeds, &mut p).expect("no cycle");

    assert_eq!(p.process_order, [a, b].map(nx), "only B's cone, deps first");
    assert_eq!(p.roots.iter().collect::<Vec<_>>(), vec![nx(b)]);
    assert_eq!(p.seeded.iter().collect::<Vec<_>>(), vec![nx(b)]);
    assert!(p.states[nx(a)].is_runnable());
    assert!(p.states[nx(b)].is_runnable());
    assert_eq!(
        p.states[nx(c)],
        NodeState::Unvisited,
        "C is upstream of nothing seeded, so the walk never reached it"
    );

    // Node seeds combine with sinks: the same seed plus `sinks` schedules
    // everything, and B stays seeded.
    let seeds = RunSeeds {
        sinks: true,
        e_node_ids: vec![b],
        ..Default::default()
    };
    planner.plan(&f.program, &seeds, &mut p).expect("no cycle");
    assert_eq!(p.process_order, [a, b, c].map(nx));
    assert_eq!(p.seeded.iter().collect::<Vec<_>>(), vec![nx(b)]);

    // A seed id absent from the program is inconsistent caller state — a hard failure,
    // not a silent skip.
    let bogus = ExecutionNodeId::from_u128(0xdead_beef);
    let seeds = RunSeeds {
        e_node_ids: vec![bogus],
        ..Default::default()
    };
    let err = planner.plan(&f.program, &seeds, &mut p).unwrap_err();
    assert!(matches!(err, Error::NodeSeedNotFound { e_node_id } if e_node_id == bogus));
}

#[test]
fn event_seed_schedules_subscribers_and_rejects_missing_ports() {
    let mut f = Fix::default();
    let emitter = f.node(false, &[], 0);
    let subscriber = f.node(false, &[], 0);
    let events = f.program.events.append([ExecutionEvent {
        subscribers: vec![nx(subscriber)],
        ..Default::default()
    }]);
    f.program.by_id_mut(emitter).events = events;

    let event = ExecutionEventPort {
        e_node_id: emitter,
        event_idx: 0,
    };
    let mut planner = Planner::default();
    let mut plan = ExecutionPlan::default();
    planner
        .plan(
            &f.program,
            &RunSeeds {
                events: vec![event],
                ..Default::default()
            },
            &mut plan,
        )
        .unwrap();
    assert_eq!(plan.roots.iter().collect::<Vec<_>>(), vec![nx(subscriber)]);
    assert_eq!(plan.process_order, [subscriber].map(nx));
    assert!(plan.event_sources.iter().next().is_none());

    planner
        .plan(
            &f.program,
            &RunSeeds {
                event_sources: true,
                ..Default::default()
            },
            &mut plan,
        )
        .unwrap();
    assert_eq!(plan.roots.iter().collect::<Vec<_>>(), vec![nx(emitter)]);
    assert_eq!(
        plan.event_sources.iter().collect::<Vec<_>>(),
        vec![nx(emitter)]
    );
    assert_eq!(plan.process_order, [emitter].map(nx));

    let invalid = [
        ExecutionEventPort {
            e_node_id: ExecutionNodeId::from_u128(0xdead_beef),
            event_idx: 0,
        },
        ExecutionEventPort {
            e_node_id: emitter,
            event_idx: 1,
        },
    ];
    for event in invalid {
        let error = planner
            .plan(
                &f.program,
                &RunSeeds {
                    events: vec![event],
                    ..Default::default()
                },
                &mut plan,
            )
            .unwrap_err();
        assert!(
            matches!(error, Error::EventSeedNotFound { event: actual } if actual == event),
            "unexpected error for {event:?}: {error:?}"
        );
    }
}
