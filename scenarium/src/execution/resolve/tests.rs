use super::*;
use crate::execution::plan::NodeVerdict;
use crate::execution::program::{ExecutionInput, ExecutionNode, ExecutionPortAddress, NodeIdx};
use crate::graph::NodeId;
use crate::prelude::FuncId;
use common::Span;

/// Hand-built program for cut tests: each node is `(terminal, &[producer_idx])` — an edge
/// per listed producer, on its own input port. `compute_needed` reads only the bindings and
/// the roots, so no digests/lambdas are needed. Node `idx` gets id `from_u128(idx + 1)`.
#[derive(Default)]
struct Fix {
    program: ExecutionProgram,
}

impl Fix {
    fn node(&mut self, producers: &[NodeIdx]) -> NodeIdx {
        let inputs_start = self.program.inputs.len() as u32;
        for &p in producers {
            self.program.inputs.push(ExecutionInput {
                required: false,
                binding: ExecutionBinding::Bind(ExecutionPortAddress {
                    target_idx: p,
                    port_idx: 0,
                }),
            });
        }
        let idx = self.program.e_nodes.len();
        let outputs_start = self.program.n_outputs as u32;
        self.program.n_outputs += 1;
        self.program.e_nodes.add(ExecutionNode {
            id: NodeId::from_u128(idx as u128 + 1),
            inited: true,
            func_id: FuncId::from_u128(idx as u128 + 1),
            inputs: Span::new(inputs_start, producers.len() as u32),
            outputs: Span::new(outputs_start, 1),
            ..Default::default()
        });
        idx.into()
    }
}

/// Run the backward cut over `fix` with the given roots and per-node resolution.
fn needed_of(fix: &Fix, roots: &[NodeIdx], resolved: &[Resolved]) -> Vec<bool> {
    let plan = ExecutionPlan {
        // Producer-first order for these fixtures is just index order (a producer is always
        // added before its consumer), matching the planner's post-order.
        process_order: (0..fix.program.e_nodes.len()).map(NodeIdx::from).collect(),
        verdicts: vec![NodeVerdict::Execute; fix.program.e_nodes.len()].into(),
        output_usage: vec![0; fix.program.n_outputs],
        roots: roots.to_vec(),
    };
    let resolved: NodeColumn<Resolved> = resolved.to_vec().into();
    let mut needed = NodeColumn::default();
    compute_needed(&fix.program, &plan, &resolved, &mut needed);
    (0..fix.program.e_nodes.len())
        .map(|i| needed[NodeIdx::from(i)])
        .collect()
}

#[test]
fn reuse_hit_prunes_its_whole_upstream_cone() {
    // src → mid → sink(root). sink runs, mid is a reuse hit ⇒ mid doesn't read src, so
    // both mid's producer (src) is cut while mid itself stays needed (sink reads it).
    let mut f = Fix::default();
    let src = f.node(&[]);
    let mid = f.node(&[src]);
    let sink = f.node(&[mid]);

    let needed = needed_of(
        &f,
        &[sink],
        &[Resolved::Run, Resolved::Reuse, Resolved::Run],
    );
    assert_eq!(
        needed,
        vec![false, true, true],
        "src is cut (its only consumer reused); mid+sink needed"
    );
}

#[test]
fn shared_producer_survives_when_one_consumer_runs() {
    // src feeds a reuse-hit `cached` AND a running `live`; both under the root `sink`.
    // The union must keep src alive for `live` even though `cached` won't read it.
    let mut f = Fix::default();
    let src = f.node(&[]);
    let cached = f.node(&[src]);
    let live = f.node(&[src]);
    let sink = f.node(&[cached, live]);

    let needed = needed_of(
        &f,
        &[sink],
        &[
            Resolved::Run,
            Resolved::Reuse, // cached: won't read src
            Resolved::Run,   // live: reads src
            Resolved::Run,
        ],
    );
    assert_eq!(
        needed,
        vec![true, true, true, true],
        "src survives: the running consumer `live` still reads it"
    );
}

#[test]
fn cone_reachable_only_through_a_reuse_hit_is_fully_pruned() {
    // deep → src → cached(hit) → sink(root). The whole chain above `cached` is cut.
    let mut f = Fix::default();
    let deep = f.node(&[]);
    let src = f.node(&[deep]);
    let cached = f.node(&[src]);
    let sink = f.node(&[cached]);

    let needed = needed_of(
        &f,
        &[sink],
        &[Resolved::Run, Resolved::Run, Resolved::Reuse, Resolved::Run],
    );
    assert_eq!(
        needed,
        vec![false, false, true, true],
        "both deep and src are pruned behind the reuse hit"
    );
}
