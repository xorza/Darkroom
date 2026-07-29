use super::*;
use crate::execution::cache::runtime::RuntimeCache;
use crate::execution::cache::slot::{OutputSnapshot, ValueState};
use crate::execution::identity::ExecutionNodeId;
use crate::execution::program::index::{NodeIdx, OutputAddr};
use crate::execution::program::{ExecutionBinding, ExecutionInput, ExecutionNode, ExecutionOutput};
use crate::node::definition::{FuncBehavior, FuncId};
use crate::node::lambda::FuncLambda;
use crate::{DynamicValue, StaticValue, async_lambda};

#[derive(Debug)]
struct CachedNode {
    e_node_id: ExecutionNodeId,
    values: Vec<DynamicValue>,
}

#[derive(Default)]
struct Fix {
    program: Program,
    order: Vec<ExecutionNodeId>,
}

impl Fix {
    fn node(&mut self, inputs: &[(bool, ExecutionBinding)], outputs: u32) -> ExecutionNodeId {
        let inputs = self
            .program
            .inputs
            .append(inputs.iter().map(|(required, binding)| ExecutionInput {
                required: *required,
                stamps_fs_path: false,
                binding: binding.clone(),
            }));
        let outputs = self
            .program
            .outputs
            .append((0..outputs).map(|_| ExecutionOutput::default()));
        let idx = self.program.e_nodes.len();
        let e_node_id = ExecutionNodeId::from_u128(idx as u128 + 1);
        self.order.push(e_node_id);
        self.program.push(
            e_node_id,
            ExecutionNode {
                behavior: FuncBehavior::Pure,
                func_id: FuncId::from_u128(idx as u128 + 1),
                inputs,
                outputs,
                lambda: async_lambda!(|_| { Ok(()) }),
                ..Default::default()
            },
        );
        e_node_id
    }

    /// The schedule as it arrives at the sweep — what the planner would have left
    /// behind — swept to the one resolved run the executor reads.
    async fn resolve(
        &self,
        roots: &[ExecutionNodeId],
        seeded: &[ExecutionNodeId],
        missing: &[ExecutionNodeId],
        cached: Vec<CachedNode>,
    ) -> RunSchedule {
        let mut schedule = RunSchedule::default();
        schedule.reset_for_program(&self.program);
        schedule
            .process_order
            .extend(self.order.iter().map(|id| nx(*id)));
        // `Cut` is the planner's positive verdict: everything runnable but nothing claimed.
        schedule
            .states
            .reset(self.program.e_nodes.len(), NodeState::Cut);
        for e_node_id in missing {
            schedule.states[nx(*e_node_id)] = NodeState::MissingInputs;
        }
        for root in roots {
            schedule.roots.insert(nx(*root));
        }
        for seed in seeded {
            schedule.seeded.insert(nx(*seed));
        }
        let mut cache = RuntimeCache::default();
        cache.reconcile_fresh(&self.program);
        cache.stamp_digests(&self.program, schedule.executing());
        for cached in cached {
            let digest = cache[nx(cached.e_node_id)].current_digest.unwrap();
            cache[nx(cached.e_node_id)].value = ValueState::Resident {
                snapshot: OutputSnapshot::new(cached.values),
                produced_under: Some(digest),
            };
        }
        schedule.resolve(&self.program, &mut cache).await;
        schedule
    }
}

/// The fixture's id ↔ index invariant: ids are assigned `from_u128(idx + 1)`
/// in push order, so a node's dense index is recoverable from its id.
fn nx(e_node_id: ExecutionNodeId) -> NodeIdx {
    NodeIdx(e_node_id.as_uuid().as_u128() as u32 - 1)
}

fn bind(e_node_id: ExecutionNodeId, port_idx: usize) -> ExecutionBinding {
    ExecutionBinding::Bind(OutputAddr {
        node_idx: nx(e_node_id),
        port_idx: port_idx as u32,
    })
}

fn value(value: i64) -> DynamicValue {
    DynamicValue::Static(StaticValue::Int(value))
}

#[tokio::test]
async fn reuse_hit_prunes_its_whole_upstream_cone() {
    let mut fix = Fix::default();
    let source = fix.node(&[], 1);
    let cached = fix.node(&[(false, bind(source, 0))], 1);
    let sink = fix.node(&[(false, bind(cached, 0))], 0);

    let run = fix
        .resolve(
            &[sink],
            &[],
            &[],
            vec![CachedNode {
                e_node_id: cached,
                values: vec![value(1)],
            }],
        )
        .await;

    assert_eq!(run.states[nx(source)], NodeState::Cut);
    assert_eq!(run.states[nx(cached)], NodeState::Reuse);
    assert_eq!(run.states[nx(sink)], NodeState::Run);
    assert_eq!(
        run.outputs.readers.slice(fix.program.by_id(source).outputs),
        &[0]
    );
}

#[tokio::test]
async fn exact_demand_accepts_narrow_producer_cache_and_ignores_reused_reader() {
    let mut fix = Fix::default();
    let source = fix.node(&[], 2);
    let cached = fix.node(&[(false, bind(source, 1))], 1);
    let live = fix.node(&[(false, bind(source, 0))], 1);
    let sink = fix.node(&[(false, bind(cached, 0)), (false, bind(live, 0))], 0);

    let run = fix
        .resolve(
            &[sink],
            &[],
            &[],
            vec![
                CachedNode {
                    e_node_id: source,
                    values: vec![value(7), DynamicValue::Unbound],
                },
                CachedNode {
                    e_node_id: cached,
                    values: vec![value(8)],
                },
            ],
        )
        .await;

    assert_eq!(run.states[nx(source)], NodeState::Reuse);
    assert_eq!(run.states[nx(cached)], NodeState::Reuse);
    assert_eq!(run.states[nx(live)], NodeState::Run);
    assert_eq!(run.states[nx(sink)], NodeState::Run);
    assert_eq!(
        run.outputs.demand.slice(fix.program.by_id(source).outputs),
        &[OutputDemand::Produce, OutputDemand::Skip]
    );
    assert_eq!(
        run.outputs.readers.slice(fix.program.by_id(source).outputs),
        &[1, 0]
    );
}

#[tokio::test]
async fn missing_input_stops_liveness_before_its_producer() {
    let mut fix = Fix::default();
    let source = fix.node(&[], 1);
    let blocked = fix.node(
        &[(false, bind(source, 0)), (true, ExecutionBinding::None)],
        0,
    );

    let run = fix.resolve(&[blocked], &[], &[blocked], Vec::new()).await;

    assert_eq!(run.states[nx(source)], NodeState::Cut);
    assert_eq!(
        run.states[nx(blocked)],
        NodeState::MissingInputs,
        "a blocked root keeps the planner's verdict — the sweep refines only \
         runnable nodes, so the reason it did not run survives to the outcome"
    );
    assert_eq!(
        run.outputs.demand.slice(fix.program.by_id(source).outputs),
        &[OutputDemand::Skip]
    );
    assert_eq!(
        run.outputs.readers.slice(fix.program.by_id(source).outputs),
        &[0]
    );
}

#[tokio::test]
async fn missing_lambda_stops_liveness_before_its_producer() {
    let mut fix = Fix::default();
    let source = fix.node(&[], 1);
    let missing = fix.node(&[(false, bind(source, 0))], 1);
    fix.program.by_id_mut(missing).lambda = FuncLambda::None;
    let sink = fix.node(&[(false, bind(missing, 0))], 0);

    let run = fix
        .resolve(
            &[sink],
            &[],
            &[],
            vec![CachedNode {
                e_node_id: missing,
                values: vec![value(9)],
            }],
        )
        .await;

    assert_eq!(run.states[nx(source)], NodeState::Cut);
    assert_eq!(
        run.states[nx(missing)],
        NodeState::MissingLambda,
        "a matching cache cannot hide a reached missing implementation"
    );
    assert_eq!(run.states[nx(sink)], NodeState::Run);
    assert_eq!(
        run.outputs.demand.slice(fix.program.by_id(source).outputs),
        &[OutputDemand::Skip]
    );
    assert_eq!(
        run.outputs.readers.slice(fix.program.by_id(source).outputs),
        &[0]
    );
    assert_eq!(
        run.outputs
            .readers
            .slice(fix.program.by_id(missing).outputs),
        &[1],
        "the downstream skip still owns one read to retire"
    );
}

/// A node seed demands every output it has, without any consumer reading them —
/// the "run to this node" semantic, distinct from demand arriving through a
/// binding.
#[tokio::test]
async fn a_node_seed_demands_every_output_without_readers() {
    let mut fix = Fix::default();
    let unseeded = fix.node(&[], 2);
    let seeded = fix.node(&[], 2);

    let run = fix
        .resolve(&[unseeded, seeded], &[seeded], &[], Vec::new())
        .await;

    assert_eq!(
        run.outputs
            .demand
            .slice(fix.program.by_id(unseeded).outputs),
        &[OutputDemand::Skip, OutputDemand::Skip],
        "a root nobody reads and nobody seeded produces nothing"
    );
    assert_eq!(
        run.outputs.demand.slice(fix.program.by_id(seeded).outputs),
        &[OutputDemand::Produce, OutputDemand::Produce]
    );
    assert!(run.outputs.readers.iter().all(|readers| *readers == 0));
}

#[tokio::test]
async fn cone_reachable_only_through_a_reuse_hit_is_fully_pruned() {
    let mut fix = Fix::default();
    let deep = fix.node(&[], 1);
    let source = fix.node(&[(false, bind(deep, 0))], 1);
    let cached = fix.node(&[(false, bind(source, 0))], 1);
    let sink = fix.node(&[(false, bind(cached, 0))], 0);

    let run = fix
        .resolve(
            &[sink],
            &[],
            &[],
            vec![CachedNode {
                e_node_id: cached,
                values: vec![value(1)],
            }],
        )
        .await;

    assert_eq!(run.states[nx(deep)], NodeState::Cut);
    assert_eq!(run.states[nx(source)], NodeState::Cut);
    assert_eq!(run.states[nx(cached)], NodeState::Reuse);
    assert_eq!(run.states[nx(sink)], NodeState::Run);
}
