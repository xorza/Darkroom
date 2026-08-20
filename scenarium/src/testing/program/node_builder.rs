//! [`NodeBuilder`]: one node of a hand-built program, under construction.

use crate::execution::compile::compiled_graph::{
    ExecutionBinding, ExecutionEvent, ExecutionInput, ExecutionNode,
};
use crate::graph::func::FuncBehavior;
use crate::graph::func::event::EventLambda;
use crate::graph::func::lambda::FuncLambda;
use crate::graph::identity::{FuncId, NodeId};
use crate::graph::node::CacheMode;
use crate::testing::program::{Placed, ProgramBuilder};
use crate::{ConstValue, DataType, async_lambda};

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

impl<'a> NodeBuilder<'a> {
    pub(super) fn new(owner: &'a mut ProgramBuilder) -> Self {
        Self {
            owner,
            e_node: ExecutionNode::default(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            events: 0,
            node_id: None,
        }
    }

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

    /// A node a cache could serve: content-cacheable, so it earns a digest, and
    /// implemented, so the sweep does not verdict it `MissingLambda` first.
    ///
    /// The two together are what "could this be reused" takes, which is why
    /// every sweep fixture states them.
    pub(crate) fn reusable(self) -> Self {
        self.pure().stub()
    }

    /// An optional input reading `binding`.
    pub(crate) fn input(self, binding: ExecutionBinding) -> Self {
        self.push_input(ExecutionInput {
            required: false,
            stamps_fs_path: false,
            binding,
        })
    }

    /// A required input reading `binding` — [`ExecutionBinding::None`] for the
    /// unbound case the planner verdicts `MissingInputs`.
    pub(crate) fn required(self, binding: ExecutionBinding) -> Self {
        self.push_input(ExecutionInput {
            required: true,
            stamps_fs_path: false,
            binding,
        })
    }

    /// An optional input carrying a literal.
    pub(crate) fn const_input(self, value: impl Into<ConstValue>) -> Self {
        self.input(ExecutionBinding::Const(value.into()))
    }

    /// An input whose delivered value's filesystem referent folds into this
    /// node's digest — what the compiler writes for an `FsPath`-declared port,
    /// and the only thing that makes a path re-key its consumer.
    pub(crate) fn fs_path_input(self, binding: ExecutionBinding) -> Self {
        self.push_input(ExecutionInput {
            required: false,
            stamps_fs_path: true,
            binding,
        })
    }

    fn push_input(mut self, input: ExecutionInput) -> Self {
        self.inputs.push(input);
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
