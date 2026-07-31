//! [`ProgramBuilder`]: a [`CompiledGraph`] built by hand, for the tests below
//! the compiler.
//!
//! The schedule and cache tests are about topology, not authoring — they want
//! "a source, a consumer reading its port 0, a sink" and no library at all. So
//! they hand-build the artifact, and each place that did grew its own copy of
//! the same two tricks: nodes placed with `NodeId::from_u128(idx + 1)` so a
//! dense index is recoverable from an id, and a `bind(node, port)` free
//! function to spell an [`ExecutionBinding`].
//!
//! Here the id↔index pair is a value ([`Placed`]) rather than a convention two
//! `fn nx` copies have to agree on, and a binding is asked of the node it
//! points at ([`Placed::out`]).
//!
//! The compiler's own tests keep using the real [`Compiler`](crate::Compiler):
//! this is for programs a compile *wouldn't* produce — a corrupt artifact a
//! validator must reject, or a topology that would take a library to express.

use crate::execution::cache::runtime::RuntimeCache;
use crate::execution::cache::slot::OutputSnapshot;
use crate::execution::compile::compiled_graph::{
    CompiledGraph, ExecutionBinding, ExecutionEvent, ExecutionInput, ExecutionNode,
};
use crate::execution::error::Result;
use crate::execution::identity::{NodeIdx, OutputAddr};
use crate::execution::schedule::planner::Planner;
use crate::execution::schedule::{NodeState, RootFlags, RunSchedule, Scheduled};
use crate::execution::seeds::RunSeeds;
use crate::graph::func::FuncBehavior;
use crate::graph::func::event::EventLambda;
use crate::graph::func::lambda::{FuncLambda, OutputDemand};
use crate::graph::identity::{FuncId, NodeId};
use crate::graph::node::CacheMode;
use crate::{DataType, DynamicValue, async_lambda};

/// Where one node landed: the stable id a host names it by, and the dense
/// index every per-run column is keyed on.
///
/// Carrying both is what removes the `NodeId::from_u128(idx + 1)` convention —
/// a fixture asks the node rather than re-deriving one from the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Placed {
    pub(crate) node_id: NodeId,
    pub(crate) node_idx: NodeIdx,
}

impl Placed {
    /// A binding reading this node's output `port`.
    pub(crate) fn out(self, port: usize) -> ExecutionBinding {
        ExecutionBinding::Bind(OutputAddr {
            node_idx: self.node_idx,
            port_idx: port as u32,
        })
    }
}

/// Builds a [`CompiledGraph`] node by node, in the ascending id order a real
/// compile places them in.
#[derive(Debug, Default)]
pub(crate) struct ProgramBuilder {
    program: CompiledGraph,
    placed: Vec<Placed>,
}

impl ProgramBuilder {
    /// Open a node. Nothing lands until [`NodeBuilder::add`].
    pub(crate) fn node(&mut self) -> NodeBuilder<'_> {
        NodeBuilder {
            owner: self,
            e_node: ExecutionNode::default(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            events: 0,
            node_id: None,
        }
    }

    pub(crate) fn program(&self) -> &CompiledGraph {
        &self.program
    }

    /// The finished artifact, for a test that installs it rather than planning
    /// over it.
    pub(crate) fn into_program(self) -> CompiledGraph {
        self.program
    }

    pub(crate) fn program_mut(&mut self) -> &mut CompiledGraph {
        &mut self.program
    }

    /// Plan `seeds` over the program, surfacing the planner's refusal — a
    /// dependency cycle, or a seed the program does not hold.
    pub(crate) fn try_plan(&self, seeds: &RunSeeds) -> Result<RunSchedule> {
        let mut schedule = RunSchedule::default();
        Planner::default().plan(&self.program, seeds, &mut schedule)?;
        Ok(schedule)
    }

    /// [`try_plan`](Self::try_plan) for the majority of tests, where planning
    /// succeeding is a precondition rather than the subject.
    pub(crate) fn plan(&self, seeds: &RunSeeds) -> RunSchedule {
        self.try_plan(seeds).expect("the fixture program plans")
    }

    /// Plan every sink — the ordinary "produce the outputs" run.
    pub(crate) fn plan_sinks(&self) -> RunSchedule {
        self.plan(&RunSeeds::sinks())
    }

    /// Stand up the cache-aware sweep over a schedule the fixture describes
    /// directly, rather than one the planner produced.
    ///
    /// Sweep tests are about what liveness and reuse do to an *already*
    /// planned schedule, so they state the planner's output — roots, blocked
    /// nodes — instead of arranging a graph that would provoke it.
    pub(crate) fn sweep(&self) -> Sweep<'_> {
        Sweep {
            owner: self,
            roots: Vec::new(),
            missing: Vec::new(),
            cached: Vec::new(),
        }
    }
}

/// One node under construction. Ports are accumulated here and packed into the
/// program's columns by [`add`](Self::add), so a node's runs are contiguous the
/// way a compile leaves them.
#[derive(Debug)]
pub(crate) struct NodeBuilder<'a> {
    owner: &'a mut ProgramBuilder,
    e_node: ExecutionNode,
    inputs: Vec<ExecutionInput>,
    outputs: Vec<DataType>,
    events: usize,
    node_id: Option<NodeId>,
}

impl NodeBuilder<'_> {
    pub(crate) fn sink(mut self) -> Self {
        self.e_node.sink = true;
        self
    }

    pub(crate) fn pure(mut self) -> Self {
        self.e_node.behavior = FuncBehavior::Pure;
        self
    }

    pub(crate) fn cache(mut self, mode: CacheMode) -> Self {
        self.e_node.cache = mode;
        self
    }

    /// Override the implementation this node's slot is owned by. Placement
    /// gives each node a distinct one; a test names it only when reinstalling
    /// a node under the *same* implementation, which is what keeps its state.
    pub(crate) fn func(mut self, func_id: FuncId) -> Self {
        self.e_node.func_id = func_id;
        self
    }

    /// Override the stable id this node is placed under.
    ///
    /// Placement numbers nodes from 1 in order, which is all a fixture needs
    /// until it reinstalls a *different* set of nodes — where the point is that
    /// the surviving ids keep their slots while the index space shifts under
    /// them. Ids must still ascend, since `node_ids` is binary-searched.
    pub(crate) fn id(mut self, node_id: NodeId) -> Self {
        self.node_id = Some(node_id);
        self
    }

    pub(crate) fn lambda(mut self, lambda: FuncLambda) -> Self {
        self.e_node.lambda = lambda;
        self
    }

    /// A no-op body, for a node the sweep or the run loop must treat as
    /// runnable rather than `MissingLambda`.
    pub(crate) fn stub(self) -> Self {
        self.lambda(async_lambda!(|_| { Ok(()) }))
    }

    /// An optional input reading `binding`.
    pub(crate) fn input(mut self, binding: ExecutionBinding) -> Self {
        self.inputs.push(ExecutionInput {
            required: false,
            stamps_fs_path: false,
            binding,
        });
        self
    }

    /// A required input reading `binding` — [`ExecutionBinding::None`] for the
    /// unbound case the planner verdicts `MissingInputs`.
    pub(crate) fn required(mut self, binding: ExecutionBinding) -> Self {
        self.inputs.push(ExecutionInput {
            required: true,
            stamps_fs_path: false,
            binding,
        });
        self
    }

    /// `count` outputs, each polymorphic. [`output_types`](Self::output_types)
    /// when a test reads the declared type back.
    pub(crate) fn outputs(mut self, count: u32) -> Self {
        self.outputs.extend((0..count).map(|_| DataType::default()));
        self
    }

    pub(crate) fn output_types(mut self, types: impl IntoIterator<Item = DataType>) -> Self {
        self.outputs.extend(types);
        self
    }

    /// Place the node and hand back where it landed.
    pub(crate) fn add(self) -> Placed {
        let NodeBuilder {
            owner,
            mut e_node,
            inputs,
            outputs,
            events,
            node_id,
        } = self;
        let position = owner.placed.len();
        // The convention every hand-built fixture used, now in one place: ids
        // ascend from 1 with the placement, so `node_ids` stays sorted (which
        // `CompiledGraph::node` binary-searches) and index 0 is not the nil id.
        let node_id = node_id.unwrap_or_else(|| NodeId::from_u128(position as u128 + 1));
        if e_node.func_id.is_nil() {
            // Keyed to the *node's* id, not its position: one implementation per
            // node (the authoring default), and stable across a reinstall that
            // shifts the index space — where a func id that moved with the
            // position would read as a changed implementation and drop the
            // node's state.
            e_node.func_id = FuncId::from_u128(node_id.as_u128());
        }
        e_node.inputs = owner.program.inputs.append(inputs);
        e_node.outputs = owner.program.outputs.append(outputs);
        e_node.events = owner
            .program
            .events
            .append((0..events).map(|_| ExecutionEvent {
                subscribers: Vec::new(),
                lambda: EventLambda::default(),
            }));
        let node_idx = owner.program.push(node_id, e_node);
        let placed = Placed { node_id, node_idx };
        owner.placed.push(placed);
        placed
    }
}

/// The cache-aware sweep over a hand-stated schedule: which nodes the planner
/// would have rooted, which it would have blocked, and what the cache already
/// holds.
#[derive(Debug)]
pub(crate) struct Sweep<'a> {
    owner: &'a ProgramBuilder,
    roots: Vec<(Placed, RootFlags)>,
    missing: Vec<Placed>,
    cached: Vec<(Placed, Vec<DynamicValue>)>,
}

impl<'a> Sweep<'a> {
    /// A plain walk root — a sink, or an event subscriber.
    pub(crate) fn root(mut self, node: Placed) -> Self {
        self.roots.push((node, RootFlags::PLAIN));
        self
    }

    /// A node-seeded root: every output demanded, `disabled` overridden.
    pub(crate) fn seeded(mut self, node: Placed) -> Self {
        self.roots.push((node, RootFlags::SEEDED));
        self
    }

    /// A node the planner blocked for want of a required input.
    pub(crate) fn missing(mut self, node: Placed) -> Self {
        self.missing.push(node);
        self
    }

    /// Prime the cache: `values` resident under the digest the sweep is about
    /// to stamp for `node`, so it reads as a hit.
    pub(crate) fn cached(
        mut self,
        node: Placed,
        values: impl IntoIterator<Item = DynamicValue>,
    ) -> Self {
        self.cached.push((node, values.into_iter().collect()));
        self
    }

    /// Run the sweep, handing back the schedule it refined and the cache it
    /// stamped.
    pub(crate) async fn run(self) -> Swept<'a> {
        let Sweep {
            owner,
            roots,
            missing,
            cached,
        } = self;
        let program = &owner.program;

        let mut schedule = RunSchedule::default();
        schedule.reset_for_program(program);
        schedule
            .process_order
            .extend(owner.placed.iter().map(|placed| placed.node_idx));
        // `Cut` is the planner's positive verdict — runnable, nothing claimed
        // it yet — so that is what a swept-from schedule starts at.
        schedule.states.reset(program.e_nodes.len(), NodeState::Cut);
        for node in missing {
            schedule.states[node.node_idx] = NodeState::MissingInputs;
        }
        for (node, flags) in roots {
            schedule.add_root(node.node_idx, flags);
        }

        let mut cache = RuntimeCache::default();
        cache.install_for_test(program);
        cache.stamp_digests(program, schedule.executing());
        for (node, values) in cached {
            let digest = cache[node.node_idx]
                .current_digest
                .expect("a cached fixture node is reproducible, so it has a digest");
            cache[node.node_idx].load_output(OutputSnapshot::new(values), Some(digest));
        }

        Scheduled::assume(program, &mut schedule)
            .resolve(&mut cache)
            .await;
        Swept { program, schedule }
    }
}

/// What a [`Sweep`] left behind, over the program it swept — so an assertion
/// names a node's port run without the caller re-deriving it.
#[derive(Debug)]
pub(crate) struct Swept<'a> {
    program: &'a CompiledGraph,
    pub(crate) schedule: RunSchedule,
}

impl Swept<'_> {
    pub(crate) fn state(&self, node: Placed) -> NodeState {
        self.schedule.states[node.node_idx]
    }

    pub(crate) fn demand(&self, node: Placed) -> &[OutputDemand] {
        &self.schedule.outputs.demand[self.program[node.node_idx].outputs]
    }

    pub(crate) fn readers(&self, node: Placed) -> &[u32] {
        &self.schedule.outputs.readers[self.program[node.node_idx].outputs]
    }
}
