use super::*;
use crate::data::DataType;
use crate::execution::program::{ExecutionInput, ExecutionNode, ExecutionPortAddress};
use crate::graph::NodeId;
use crate::node::function::FuncId;
use common::Span;

/// Hand-built program for planner tests. Node `idx` gets id `from_u128(idx+1)`.
/// Inputs are `(required, binding)`.
#[derive(Default)]
struct Fix {
    program: ExecutionProgram,
}

impl Fix {
    fn node(
        &mut self,
        terminal: bool,
        inputs: &[(bool, ExecutionBinding)],
        outputs: u32,
    ) -> NodeIdx {
        let inputs_start = self.program.inputs.len() as u32;
        for (required, binding) in inputs {
            self.program.inputs.push(ExecutionInput {
                required: *required,
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
            terminal,
            func_id: FuncId::from_u128(idx as u128 + 1),
            inputs: Span::new(inputs_start, inputs.len() as u32),
            outputs: Span::new(outputs_start, outputs),
            ..Default::default()
        });
        idx.into()
    }
}

fn bind(idx: NodeIdx, port: usize) -> ExecutionBinding {
    ExecutionBinding::Bind(ExecutionPortAddress {
        target_idx: idx,
        port_idx: port,
    })
}

/// Plan `terminals` over `fix`. Purely structural — no cache state; the executor
/// decides cached-vs-recompute at run time.
fn plan(fix: &Fix) -> ExecutionPlan {
    let mut planner = Planner::default();
    let mut plan = ExecutionPlan::default();
    let seeds = RunSeeds {
        terminals: true,
        ..Default::default()
    };
    planner
        .plan(&fix.program, &seeds, &mut plan)
        .expect("no cycle");
    plan
}

#[test]
fn chain_orders_deps_before_consumers_and_schedules_all() {
    // A → B → C (C terminal). Every reachable node is scheduled — the planner is
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
    // A feeds both B and C (both terminal) ⇒ A's output is needed twice.
    let mut f = Fix::default();
    let a = f.node(false, &[], 1);
    f.node(true, &[(false, bind(a, 0))], 1);
    f.node(true, &[(false, bind(a, 0))], 1);

    let p = plan(&f);
    assert_eq!(p.output_usage[0], 2, "A.0 read by two consumers");
}

#[test]
fn dependency_cycle_is_rejected() {
    // A binds B, B binds A (A terminal) — the planner must error, not loop.
    let mut f = Fix::default();
    f.node(true, &[(false, bind(NodeIdx(1), 0))], 1); // A (idx 0) binds B
    f.node(false, &[(false, bind(NodeIdx(0), 0))], 1); // B (idx 1) binds A

    let mut planner = Planner::default();
    let mut plan = ExecutionPlan::default();
    let seeds = RunSeeds {
        terminals: true,
        ..Default::default()
    };
    let result = planner.plan(&f.program, &seeds, &mut plan);
    assert!(matches!(result, Err(Error::CycleDetected { .. })));
}
