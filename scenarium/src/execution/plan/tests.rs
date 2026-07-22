use crate::DataType;
use crate::execution::compile::CompiledGraph;
use crate::execution::identity::{ExecutionNodeId, ExecutionOutputPort};
use crate::execution::plan::{ExecutionPlan, NodeVerdict, Planner};
use crate::execution::program::{ExecutionBinding, ExecutionInput, ExecutionNode};
use crate::execution::{Error, NodeSet, RunSeeds};
use crate::graph::NodeId;
use crate::node::definition::FuncId;
use common::Span;

/// Hand-built compile artifact for planner tests. Every node gets a root
/// attribution leaf. Inputs are `(required, binding)`.
#[derive(Default)]
struct Fix {
    compiled: CompiledGraph,
}

impl Fix {
    fn node(
        &mut self,
        sink: bool,
        inputs: &[(bool, ExecutionBinding)],
        outputs: u32,
    ) -> ExecutionNodeId {
        let program = &mut self.compiled.program;
        if program.e_nodes.is_empty() {
            self.compiled.flatten_map.reset();
        }
        let inputs_start = program.inputs.len() as u32;
        for (required, binding) in inputs {
            program.inputs.push(ExecutionInput {
                required: *required,
                stamper: None,
                binding: binding.clone(),
            });
        }
        let outputs_start = program.output_types.len() as u32;
        program
            .output_types
            .resize(outputs_start as usize + outputs as usize, DataType::Any);
        // Kept in lockstep with `output_types` (same index space) so
        // `output_pinned.len() == n_outputs` holds here exactly as
        // `Flattener::build` guarantees for a real compile — the planner's fold
        // over both pools relies on this, not just a defensive fallback.
        program
            .output_pinned
            .resize(outputs_start as usize + outputs as usize, false);
        let idx = program.e_nodes.len();
        let id = ExecutionNodeId::from_u128(idx as u128 + 1);
        program.e_nodes.insert(
            id,
            ExecutionNode {
                sink,
                func_id: FuncId::from_u128(idx as u128 + 1),
                inputs: Span::new(inputs_start, inputs.len() as u32),
                outputs: Span::new(outputs_start, outputs),
                ..Default::default()
            },
        );
        self.compiled
            .flatten_map
            .set_leaf(id, 0, NodeId::from(id.as_uuid()));
        id
    }
}

fn bind(e_node_id: ExecutionNodeId, port: usize) -> ExecutionBinding {
    ExecutionBinding::Bind(ExecutionOutputPort {
        e_node_id,
        port_idx: port,
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
        .plan(&fix.compiled, &seeds, &mut plan)
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
    p.validate(&f.compiled.program).unwrap();
    assert_eq!(p.process_order, vec![a, b, c], "post-order: deps first");
    for idx in [a, b, c] {
        assert!(p.verdicts[&idx].wants_execute());
        assert!(!p.verdicts[&idx].missing_required_inputs());
    }

    p.process_order.swap(0, 1);
    assert_eq!(
        p.validate(&f.compiled.program).unwrap_err().to_string(),
        format!("execution node {b:?} appears before dependency {a:?}")
    );
    *p.verdicts.get_mut(&a).unwrap() = NodeVerdict::Disabled;
    assert_eq!(
        p.validate(&f.compiled.program).unwrap_err().to_string(),
        format!("execution node {b:?} appears before dependency {a:?}"),
        "a disabled verdict cannot hide an enabled dependency"
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
            p.verdicts[&idx].missing_required_inputs(),
            "node {idx:?} missing"
        );
        assert!(
            !p.verdicts[&idx].wants_execute(),
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
    assert!(!p.verdicts[&a].missing_required_inputs());
    assert!(p.verdicts[&a].wants_execute());
    assert_eq!(p.process_order, vec![a]);
}

#[test]
fn explicit_seed_overrides_disabled_dependency_for_this_run() {
    let mut f = Fix::default();
    let producer = f.node(false, &[], 1);
    f.compiled
        .program
        .e_nodes
        .get_mut(&producer)
        .unwrap()
        .disabled = true;
    let required = f.node(true, &[(true, bind(producer, 0))], 1);
    let optional = f.node(true, &[(false, bind(producer, 0))], 1);

    let mut planner = Planner::default();
    let mut plan = ExecutionPlan::default();
    planner
        .plan(
            &f.compiled,
            &RunSeeds {
                sinks: true,
                ..Default::default()
            },
            &mut plan,
        )
        .unwrap();
    assert_eq!(plan.verdicts[&producer], NodeVerdict::Disabled);
    assert_eq!(plan.verdicts[&required], NodeVerdict::MissingInputs);
    assert_eq!(plan.verdicts[&optional], NodeVerdict::Execute);

    planner
        .plan(
            &f.compiled,
            &RunSeeds {
                sinks: true,
                nodes: vec![producer],
                ..Default::default()
            },
            &mut plan,
        )
        .unwrap();
    for e_node_id in [producer, required, optional] {
        assert_eq!(
            plan.verdicts[&e_node_id],
            NodeVerdict::Execute,
            "the explicit producer seed makes every consumer runnable"
        );
    }
}

#[test]
fn node_seed_is_both_a_root_and_pinned() {
    let mut f = Fix::default();
    let a = f.node(false, &[], 1);
    f.compiled.program.output_pinned[0] = true;

    let mut planner = Planner::default();
    let mut p = ExecutionPlan::default();
    let seeds = RunSeeds {
        nodes: vec![a],
        ..Default::default()
    };
    planner.plan(&f.compiled, &seeds, &mut p).expect("no cycle");

    assert_eq!(p.pinned, NodeSet::from([a]));
    assert_eq!(p.roots, NodeSet::from([a]));

    let seeds = RunSeeds {
        nodes: vec![a, a],
        ..Default::default()
    };
    planner.plan(&f.compiled, &seeds, &mut p).expect("no cycle");
    assert_eq!(p.pinned, NodeSet::from([a]));
    assert_eq!(p.roots, NodeSet::from([a]));
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
    let result = planner.plan(&f.compiled, &seeds, &mut plan);
    assert!(matches!(result, Err(Error::CycleDetected { .. })));
}

#[test]
fn node_seed_schedules_only_its_cone_and_pins_it() {
    // A → B → C (C sink). Seeding node B (by authoring id — top-level ids resolve
    // straight against the program) schedules only [A, B] — C is upstream of nothing
    // seeded — and records B as both a root and a pinned node. B's output has no
    // scheduled consumer.
    let mut f = Fix::default();
    let a = f.node(false, &[], 1);
    let b = f.node(false, &[(false, bind(a, 0))], 1);
    let c = f.node(true, &[(false, bind(b, 0))], 1);

    let mut planner = Planner::default();
    let mut p = ExecutionPlan::default();
    let seeds = RunSeeds {
        nodes: vec![b],
        ..Default::default()
    };
    planner.plan(&f.compiled, &seeds, &mut p).expect("no cycle");

    assert_eq!(p.process_order, vec![a, b], "only B's cone, deps first");
    assert_eq!(p.roots, NodeSet::from([b]));
    assert_eq!(p.pinned, NodeSet::from([b]));
    assert!(p.verdicts[&a].wants_execute());
    assert!(p.verdicts[&b].wants_execute());
    assert!(!p.verdicts[&c].wants_execute(), "C never verdicted");

    // Node seeds combine with sinks: the same seed plus `sinks` schedules
    // everything, and B stays pinned.
    let seeds = RunSeeds {
        sinks: true,
        nodes: vec![b],
        ..Default::default()
    };
    planner.plan(&f.compiled, &seeds, &mut p).expect("no cycle");
    assert_eq!(p.process_order, vec![a, b, c]);
    assert_eq!(p.pinned, NodeSet::from([b]));

    // A seed id absent from the program is inconsistent caller state — a hard failure,
    // not a silent skip.
    let bogus = ExecutionNodeId::from_u128(0xdead_beef);
    let seeds = RunSeeds {
        nodes: vec![bogus],
        ..Default::default()
    };
    let err = planner.plan(&f.compiled, &seeds, &mut p).unwrap_err();
    assert!(matches!(err, Error::NodeSeedNotFound { e_node_id } if e_node_id == bogus));
}
