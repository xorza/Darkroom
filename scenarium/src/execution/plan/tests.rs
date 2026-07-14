use super::*;
use crate::DataType;
use crate::execution::identity::NodeAddress;
use crate::execution::program::{ExecutionInput, ExecutionNode, ExecutionPortAddress, OutputIdx};
use crate::graph::NodeId;
use crate::node::definition::FuncId;
use common::Span;
use std::sync::Arc;

/// Hand-built compile artifact for planner tests (an empty flatten map — every
/// node is "top-level", so seed ids resolve directly). Node `idx` gets id
/// `from_u128(idx+1)`. Inputs are `(required, binding)`.
#[derive(Default)]
struct Fix {
    compiled: CompiledGraph,
}

impl Fix {
    fn node(&mut self, sink: bool, inputs: &[(bool, ExecutionBinding)], outputs: u32) -> NodeIdx {
        let program = &mut self.compiled.program;
        if program.e_nodes.is_empty() {
            Arc::get_mut(&mut self.compiled.flatten_map)
                .unwrap()
                .reset();
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
        let id = NodeId::from_u128(idx as u128 + 1);
        program.e_nodes.add(ExecutionNode {
            id,
            sink,
            func_id: FuncId::from_u128(idx as u128 + 1),
            inputs: Span::new(inputs_start, inputs.len() as u32),
            outputs: Span::new(outputs_start, outputs),
            ..Default::default()
        });
        Arc::get_mut(&mut self.compiled.flatten_map)
            .unwrap()
            .set_leaf(id, 0, id);
        idx.into()
    }
}

fn bind(idx: NodeIdx, port: usize) -> ExecutionBinding {
    ExecutionBinding::Bind(ExecutionPortAddress {
        target_idx: idx,
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

fn demand(plan: &ExecutionPlan, output_idx: usize) -> OutputDemand {
    plan.outputs.demand[OutputIdx::from(output_idx)]
}

fn readers(plan: &ExecutionPlan, output_idx: usize) -> u32 {
    plan.outputs.readers[OutputIdx::from(output_idx)]
}

#[test]
fn chain_orders_deps_before_consumers_and_schedules_all() {
    // A → B → C (C sink). Every reachable node is scheduled — the planner is
    // structural, so nothing is pruned as "cached" here (that's the executor's call).
    let mut f = Fix::default();
    let a = f.node(false, &[], 1);
    let b = f.node(false, &[(false, bind(a, 0))], 1);
    let c = f.node(true, &[(false, bind(b, 0))], 1);

    let p = plan(&f);
    assert_eq!(p.process_order, vec![a, b, c], "post-order: deps first");
    for idx in [a, b, c] {
        assert!(p.verdicts[idx].wants_execute());
        assert!(!p.verdicts[idx].missing_required_inputs());
    }
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
            p.verdicts[idx].missing_required_inputs(),
            "node {idx:?} missing"
        );
        assert!(
            !p.verdicts[idx].wants_execute(),
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
    assert!(!p.verdicts[a].missing_required_inputs());
    assert!(p.verdicts[a].wants_execute());
    assert_eq!(p.process_order, vec![a]);
}

#[test]
fn fan_out_counts_each_executing_consumer() {
    // A feeds both B and C (both sink) ⇒ A's output is needed twice.
    let mut f = Fix::default();
    let a = f.node(false, &[], 1);
    f.node(true, &[(false, bind(a, 0))], 1);
    f.node(true, &[(false, bind(a, 0))], 1);

    let p = plan(&f);
    assert_eq!(demand(&p, 0), OutputDemand::Produce);
    assert_eq!(readers(&p, 0), 2, "A.0 read by two consumers");
}

#[test]
fn pinned_port_demands_output_without_creating_a_reader() {
    // A has two outputs, neither structurally consumed by anything. Only port 1
    // is flagged pinned (e.g. a GUI inspector reading it live) — the planner's
    // demand must include exactly that one without inventing a binding reader.
    let mut f = Fix::default();
    f.node(true, &[], 2);
    f.compiled.program.output_pinned[1] = true;

    let p = plan(&f);
    assert_eq!(demand(&p, 0), OutputDemand::Skip);
    assert_eq!(demand(&p, 1), OutputDemand::Produce);
    assert_eq!(readers(&p, 0), 0);
    assert_eq!(readers(&p, 1), 0, "host pins are not binding readers");
}

#[test]
fn overlapping_pin_sources_leave_reader_count_at_zero() {
    // A's only output is BOTH a pinned root (node-seeded) and individually
    // pinned, with no real consumer — the two sources must not stack into 2.
    let mut f = Fix::default();
    let a = f.node(false, &[], 1);
    f.compiled.program.output_pinned[0] = true;

    let mut planner = Planner::default();
    let mut p = ExecutionPlan::default();
    let seeds = RunSeeds {
        nodes: vec![NodeAddress::root(f.compiled.program.e_nodes[a].id)],
        ..Default::default()
    };
    planner.plan(&f.compiled, &seeds, &mut p).expect("no cycle");

    assert_eq!(p.pinned, vec![a]);
    assert_eq!(demand(&p, 0), OutputDemand::Produce);
    assert_eq!(readers(&p, 0), 0);
}

#[test]
fn dependency_cycle_is_rejected() {
    // A binds B, B binds A (A sink) — the planner must error, not loop.
    let mut f = Fix::default();
    f.node(true, &[(false, bind(NodeIdx(1), 0))], 1); // A (idx 0) binds B
    f.node(false, &[(false, bind(NodeIdx(0), 0))], 1); // B (idx 1) binds A

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
    // scheduled consumer, so pinning demands it without adding a reader.
    let mut f = Fix::default();
    let a = f.node(false, &[], 1);
    let b = f.node(false, &[(false, bind(a, 0))], 1);
    let c = f.node(true, &[(false, bind(b, 0))], 1);

    let mut planner = Planner::default();
    let mut p = ExecutionPlan::default();
    let seeds = RunSeeds {
        nodes: vec![NodeAddress::root(f.compiled.program.e_nodes[b].id)],
        ..Default::default()
    };
    planner.plan(&f.compiled, &seeds, &mut p).expect("no cycle");

    assert_eq!(p.process_order, vec![a, b], "only B's cone, deps first");
    assert_eq!(p.roots, vec![b]);
    assert_eq!(p.pinned, vec![b]);
    assert!(p.verdicts[a].wants_execute());
    assert!(p.verdicts[b].wants_execute());
    assert!(!p.verdicts[c].wants_execute(), "C never verdicted");
    assert_eq!(demand(&p, 0), OutputDemand::Produce);
    assert_eq!(readers(&p, 0), 1, "A.0 read by B");
    assert_eq!(demand(&p, 1), OutputDemand::Produce);
    assert_eq!(readers(&p, 1), 0, "B.0 is host-demanded but unconsumed");

    // Node seeds combine with sinks: the same seed plus `sinks` schedules
    // everything, and B stays pinned.
    let seeds = RunSeeds {
        sinks: true,
        nodes: vec![NodeAddress::root(f.compiled.program.e_nodes[b].id)],
        ..Default::default()
    };
    planner.plan(&f.compiled, &seeds, &mut p).expect("no cycle");
    assert_eq!(p.process_order, vec![a, b, c]);
    assert_eq!(p.pinned, vec![b]);
    assert_eq!(demand(&p, 1), OutputDemand::Produce);
    assert_eq!(readers(&p, 1), 1, "only C is a binding reader of B.0");

    // A seed id absent from the program is inconsistent caller state — a hard failure,
    // not a silent skip.
    let bogus = NodeId::from_u128(0xdead_beef);
    let seeds = RunSeeds {
        nodes: vec![NodeAddress::root(bogus)],
        ..Default::default()
    };
    let err = planner.plan(&f.compiled, &seeds, &mut p).unwrap_err();
    assert!(
        matches!(err, Error::NodeSeedNotFound { address } if address == NodeAddress::root(bogus))
    );
}
