use super::*;
use crate::DataType;
use crate::execution::plan::{NodeVerdict, PlannedOutputs};
use crate::execution::program::{ExecutionInput, ExecutionNode, ExecutionPortAddress};
use crate::graph::NodeId;
use crate::node::definition::FuncId;
use crate::node::lambda::OutputDemand;
use common::Span;

/// Hand-built program for cut tests: each node is `(sink, &[producer_idx])` — an edge
/// per listed producer, on its own input port. `compute_disposition` reads only the bindings and
/// the roots, so no digests/lambdas are needed. Node `idx` gets id `from_u128(idx + 1)`.
#[derive(Default)]
struct Fix {
    program: ExecutionProgram,
}

impl Fix {
    fn node(&mut self, producers: &[NodeId]) -> NodeId {
        let inputs_start = self.program.inputs.len() as u32;
        for &p in producers {
            self.program.inputs.push(ExecutionInput {
                required: false,
                stamper: None,
                binding: ExecutionBinding::Bind(ExecutionPortAddress {
                    target: p,
                    port_idx: 0,
                }),
            });
        }
        let idx = self.program.e_nodes.len();
        let outputs_start = self.program.output_types.len() as u32;
        self.program.output_types.push(DataType::Any);
        let node_id = NodeId::from_u128(idx as u128 + 1);
        self.program.node_order.push(node_id);
        self.program.e_nodes.insert(
            node_id,
            ExecutionNode {
                id: node_id,
                func_id: FuncId::from_u128(idx as u128 + 1),
                inputs: Span::new(inputs_start, producers.len() as u32),
                outputs: Span::new(outputs_start, 1),
                ..Default::default()
            },
        );
        node_id
    }
}

/// Run the backward cut over `fix` with the given roots and per-node resolution,
/// returning each node's merged [`Disposition`].
fn dispositions_of(fix: &Fix, roots: &[NodeId], reused: &[bool]) -> Vec<Disposition> {
    assert_eq!(reused.len(), fix.program.e_nodes.len());
    let verdicts = fix
        .program
        .node_ids()
        .map(|node_id| (node_id, NodeVerdict::Execute))
        .collect();
    let plan = ExecutionPlan {
        // Producer-first order for these fixtures is just index order (a producer is always
        // added before its consumer), matching the planner's post-order.
        process_order: fix.program.node_ids().collect(),
        verdicts,
        outputs: PlannedOutputs {
            demand: vec![OutputDemand::Skip; fix.program.n_outputs()].into(),
            readers: vec![0; fix.program.n_outputs()].into(),
        },
        roots: roots.to_vec(),
        pinned: Vec::new(),
    };
    let reused: NodeSet = fix
        .program
        .node_ids()
        .zip(reused.iter().copied())
        .filter_map(|(node_id, reused)| reused.then_some(node_id))
        .collect();
    let mut disposition = NodeMap::default();
    compute_disposition(&fix.program, &plan, &reused, &mut disposition);
    fix.program
        .node_ids()
        .map(|node_id| disposition[&node_id])
        .collect()
}

#[test]
fn reuse_hit_prunes_its_whole_upstream_cone() {
    // src → mid → sink(root). sink runs, mid is a reuse hit ⇒ mid doesn't read src, so
    // mid's producer (src) is cut while mid itself stays on the frontier (sink reads it).
    let mut f = Fix::default();
    let src = f.node(&[]);
    let mid = f.node(&[src]);
    let sink = f.node(&[mid]);

    let dispositions = dispositions_of(&f, &[sink], &[false, true, false]);
    assert_eq!(
        dispositions,
        vec![Disposition::Cut, Disposition::Reuse, Disposition::Run],
        "src is cut (its only consumer reused); mid serves its cache; sink runs"
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

    let dispositions = dispositions_of(
        &f,
        &[sink],
        &[
            false, true,  // cached: won't read src
            false, // live: reads src
            false,
        ],
    );
    assert_eq!(
        dispositions,
        vec![
            Disposition::Run,
            Disposition::Reuse,
            Disposition::Run,
            Disposition::Run,
        ],
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

    let dispositions = dispositions_of(&f, &[sink], &[false, false, true, false]);
    assert_eq!(
        dispositions,
        vec![
            Disposition::Cut,
            Disposition::Cut,
            Disposition::Reuse,
            Disposition::Run,
        ],
        "both deep and src are pruned behind the reuse hit"
    );
}
