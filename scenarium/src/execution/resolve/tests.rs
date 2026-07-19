use super::*;
use crate::DataType;
use crate::execution::cache::{OutputSnapshot, RuntimeCache, ValueState};
use crate::execution::plan::NodeVerdict;
use crate::execution::program::{
    ExecutionBinding, ExecutionInput, ExecutionNode, ExecutionPortAddress,
};
use crate::graph::NodeId;
use crate::node::definition::{FuncBehavior, FuncId};
use crate::{DynamicValue, StaticValue};
use common::Span;

#[derive(Debug)]
struct CachedNode {
    node_id: NodeId,
    values: Vec<DynamicValue>,
}

#[derive(Default)]
struct Fix {
    program: ExecutionProgram,
    order: Vec<NodeId>,
}

impl Fix {
    fn node(&mut self, inputs: &[(bool, ExecutionBinding)], outputs: u32) -> NodeId {
        let inputs_start = self.program.inputs.len() as u32;
        for (required, binding) in inputs {
            self.program.inputs.push(ExecutionInput {
                required: *required,
                stamper: None,
                binding: binding.clone(),
            });
        }
        let outputs_start = self.program.output_types.len() as u32;
        self.program
            .output_types
            .resize(outputs_start as usize + outputs as usize, DataType::Any);
        self.program
            .output_pinned
            .resize(outputs_start as usize + outputs as usize, false);
        let idx = self.program.e_nodes.len();
        let node_id = NodeId::from_u128(idx as u128 + 1);
        self.order.push(node_id);
        self.program.e_nodes.insert(
            node_id,
            ExecutionNode {
                behavior: FuncBehavior::Pure,
                func_id: FuncId::from_u128(idx as u128 + 1),
                inputs: Span::new(inputs_start, inputs.len() as u32),
                outputs: Span::new(outputs_start, outputs),
                ..Default::default()
            },
        );
        node_id
    }

    fn resolve(
        &self,
        roots: &[NodeId],
        pinned: &[NodeId],
        missing: &[NodeId],
        cached: Vec<CachedNode>,
    ) -> ResolvedRun {
        let mut verdicts: NodeMap<NodeVerdict> = self
            .program
            .e_nodes
            .keys()
            .copied()
            .map(|node_id| (node_id, NodeVerdict::Execute))
            .collect();
        for node_id in missing {
            *verdicts.get_mut(node_id).unwrap() = NodeVerdict::MissingInputs;
        }
        let plan = ExecutionPlan {
            process_order: self.order.clone(),
            verdicts,
            roots: roots.iter().copied().collect(),
            pinned: pinned.iter().copied().collect(),
        };
        let mut cache = RuntimeCache::default();
        cache.reconcile(&self.program);
        let resource_stamps = RunResourceStamps::default();
        stamp_digests(&self.program, &mut cache, &resource_stamps, &plan);
        for cached in cached {
            let digest = cache.slots[&cached.node_id].current_digest.unwrap();
            cache.slots.get_mut(&cached.node_id).unwrap().value = ValueState::Resident {
                snapshot: OutputSnapshot::new(cached.values),
                produced_under: Some(digest),
            };
        }
        let mut resolver = Resolver::default();
        resolver.resolve(&self.program, &plan, &mut cache, &resource_stamps);
        resolver.run
    }
}

fn bind(node_id: NodeId, port_idx: usize) -> ExecutionBinding {
    ExecutionBinding::Bind(ExecutionPortAddress {
        target: node_id,
        port_idx,
    })
}

fn value(value: i64) -> DynamicValue {
    DynamicValue::Static(StaticValue::Int(value))
}

#[test]
fn reuse_hit_prunes_its_whole_upstream_cone() {
    let mut fix = Fix::default();
    let source = fix.node(&[], 1);
    let cached = fix.node(&[(false, bind(source, 0))], 1);
    let sink = fix.node(&[(false, bind(cached, 0))], 0);

    let run = fix.resolve(
        &[sink],
        &[],
        &[],
        vec![CachedNode {
            node_id: cached,
            values: vec![value(1)],
        }],
    );

    assert_eq!(run.disposition[&source], Disposition::Cut);
    assert_eq!(run.disposition[&cached], Disposition::Reuse);
    assert_eq!(run.disposition[&sink], Disposition::Run);
    assert_eq!(
        run.outputs
            .readers
            .slice(fix.program.e_nodes[&source].outputs),
        &[0]
    );
}

#[test]
fn exact_demand_accepts_narrow_producer_cache_and_ignores_reused_reader() {
    let mut fix = Fix::default();
    let source = fix.node(&[], 2);
    let cached = fix.node(&[(false, bind(source, 1))], 1);
    let live = fix.node(&[(false, bind(source, 0))], 1);
    let sink = fix.node(&[(false, bind(cached, 0)), (false, bind(live, 0))], 0);

    let run = fix.resolve(
        &[sink],
        &[],
        &[],
        vec![
            CachedNode {
                node_id: source,
                values: vec![value(7), DynamicValue::Unbound],
            },
            CachedNode {
                node_id: cached,
                values: vec![value(8)],
            },
        ],
    );

    assert_eq!(run.disposition[&source], Disposition::Reuse);
    assert_eq!(run.disposition[&cached], Disposition::Reuse);
    assert_eq!(run.disposition[&live], Disposition::Run);
    assert_eq!(run.disposition[&sink], Disposition::Run);
    assert_eq!(
        run.outputs
            .demand
            .slice(fix.program.e_nodes[&source].outputs),
        &[OutputDemand::Produce, OutputDemand::Skip]
    );
    assert_eq!(
        run.outputs
            .readers
            .slice(fix.program.e_nodes[&source].outputs),
        &[1, 0]
    );
}

#[test]
fn missing_input_stops_liveness_before_its_producer() {
    let mut fix = Fix::default();
    let source = fix.node(&[], 1);
    let blocked = fix.node(
        &[(false, bind(source, 0)), (true, ExecutionBinding::None)],
        0,
    );

    let run = fix.resolve(&[blocked], &[], &[blocked], Vec::new());

    assert_eq!(run.disposition[&source], Disposition::Cut);
    assert_eq!(run.disposition[&blocked], Disposition::Cut);
    assert_eq!(
        run.outputs
            .demand
            .slice(fix.program.e_nodes[&source].outputs),
        &[OutputDemand::Skip]
    );
    assert_eq!(
        run.outputs
            .readers
            .slice(fix.program.e_nodes[&source].outputs),
        &[0]
    );
}

#[test]
fn graph_and_node_pins_seed_demand_without_readers() {
    let mut fix = Fix::default();
    let graph_pinned = fix.node(&[], 2);
    let node_pinned = fix.node(&[], 2);
    let output_idx = fix.program.output_idx(graph_pinned, 1);
    fix.program.output_pinned[output_idx.idx()] = true;

    let run = fix.resolve(
        &[graph_pinned, node_pinned],
        &[node_pinned],
        &[],
        Vec::new(),
    );

    assert_eq!(
        run.outputs
            .demand
            .slice(fix.program.e_nodes[&graph_pinned].outputs),
        &[OutputDemand::Skip, OutputDemand::Produce]
    );
    assert_eq!(
        run.outputs
            .demand
            .slice(fix.program.e_nodes[&node_pinned].outputs),
        &[OutputDemand::Produce, OutputDemand::Produce]
    );
    assert!(
        run.outputs
            .readers
            .values
            .iter()
            .all(|readers| *readers == 0)
    );
}

#[test]
fn cone_reachable_only_through_a_reuse_hit_is_fully_pruned() {
    let mut fix = Fix::default();
    let deep = fix.node(&[], 1);
    let source = fix.node(&[(false, bind(deep, 0))], 1);
    let cached = fix.node(&[(false, bind(source, 0))], 1);
    let sink = fix.node(&[(false, bind(cached, 0))], 0);

    let run = fix.resolve(
        &[sink],
        &[],
        &[],
        vec![CachedNode {
            node_id: cached,
            values: vec![value(1)],
        }],
    );

    assert_eq!(run.disposition[&deep], Disposition::Cut);
    assert_eq!(run.disposition[&source], Disposition::Cut);
    assert_eq!(run.disposition[&cached], Disposition::Reuse);
    assert_eq!(run.disposition[&sink], Disposition::Run);
}
