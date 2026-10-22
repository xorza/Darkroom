//! [`NodeBuilder`]: one node of a hand-built program, under construction.

use crate::containers::col::Span;
use crate::data::const_value::ConstValue;
use crate::data::data_type::DataType;
use crate::exec::compile::compiled_graph::{ExecBinding, ExecEvent, ExecInput, ExecNode};
use crate::graph::func::FuncBehavior;
use crate::graph::func::func_output::FuncOutput;
use crate::graph::func::lambda::FuncLambda;
use crate::graph::func::signature::FuncSignature;
use crate::graph::identity::{FuncId, FuncRef, NodeId};
use crate::graph::node::CacheMode;
use crate::internals;
use crate::internals::program::{Placed, ProgramBuilder};

/// One node under construction. Ports are accumulated here and packed into the
/// program's columns by [`add`](Self::add), so a node's runs are contiguous the
/// way a compile leaves them.
#[derive(Debug)]
pub(crate) struct NodeBuilder<'a> {
    owner: &'a mut ProgramBuilder,
    e_node: ExecNode,
    inputs: Vec<ExecInput>,
    outputs: Vec<FuncOutput>,
    events: Vec<ExecEvent>,
    func_id: Option<FuncId>,
    node_id: Option<NodeId>,
}

impl<'a> NodeBuilder<'a> {
    pub(super) fn new(owner: &'a mut ProgramBuilder) -> Self {
        Self {
            owner,
            e_node: ExecNode::bare(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            events: Vec::new(),
            func_id: None,
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
        self.func_id = Some(func_id);
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

    /// An optional input reading `binding`.
    pub(crate) fn input(self, binding: ExecBinding) -> Self {
        self.push_input(ExecInput {
            required: false,
            stamps_fs_path: false,
            binding,
        })
    }

    /// A required input reading `binding` — [`ExecBinding::None`] for the
    /// unbound case the planner verdicts `MissingInputs`.
    pub(crate) fn required(self, binding: ExecBinding) -> Self {
        self.push_input(ExecInput {
            required: true,
            stamps_fs_path: false,
            binding,
        })
    }

    /// An optional input carrying a literal.
    pub(crate) fn const_input(self, value: impl Into<ConstValue>) -> Self {
        self.input(ExecBinding::Const(value.into()))
    }

    /// An input whose delivered value's filesystem referent folds into this
    /// node's digest — what the compiler writes for an `FsPath`-declared port,
    /// and the only thing that makes a path re-key its consumer.
    pub(crate) fn fs_path_input(self, binding: ExecBinding) -> Self {
        self.push_input(ExecInput {
            required: false,
            stamps_fs_path: true,
            binding,
        })
    }

    fn push_input(mut self, input: ExecInput) -> Self {
        self.inputs.push(input);
        self
    }

    /// `count` outputs, each `Int`.
    pub(crate) fn outputs(self, count: u32) -> Self {
        self.output_types((0..count).map(|_| DataType::Int))
    }

    /// One output per type, declared in the node's signature as a func's are,
    /// which is where an output type reaches the digest.
    pub(crate) fn output_types(mut self, types: impl IntoIterator<Item = DataType>) -> Self {
        for data_type in types {
            let name = format!("out{}", self.outputs.len());
            self.outputs.push(FuncOutput::new(name, data_type));
        }
        self
    }

    /// Declare one event whose subscribers are `subscribers`, in order.
    pub(crate) fn event(mut self, subscribers: impl IntoIterator<Item = Placed>) -> Self {
        let subscribers = self
            .owner
            .program
            .subscribers
            .append(subscribers.into_iter().map(|node| node.node_idx));
        self.events.push(ExecEvent {
            subscribers,
            lambda: internals::stub_event(),
        });
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
            func_id,
            node_id,
        } = self;
        let pos = owner.placed.len();
        // The convention every hand-built fixture used, now in one place: ids
        // ascend from 1 with the placement, so `node_ids` stays sorted (which
        // `CompiledGraph::node` binary-searches) and index 0 is not the nil id.
        let node_id = node_id.unwrap_or_else(|| NodeId::from_u128(pos as u128 + 1));
        // Keyed by default to the *node's* id, not its position: one
        // implementation per node (the authoring default), and stable across a
        // reinstall that shifts the index space — where a func id that moved
        // with the position would read as a changed implementation and drop the
        // node's state.
        let func_id = func_id.unwrap_or_else(|| FuncId::from_u128(node_id.as_u128()));
        e_node.func_ref = FuncRef::new(func_id, FuncSignature::of(&[], &outputs, &[]));
        e_node.inputs = owner.program.inputs.append(inputs);
        let output_count = u32::try_from(outputs.len()).expect("a fixture's outputs fit in u32");
        e_node.outputs = Span::new(owner.program.output_count, output_count);
        owner.program.output_count += output_count;
        e_node.events = owner.program.events.append(events);
        let node_idx = owner.program.push(node_id, e_node);
        let placed = Placed { node_id, node_idx };
        owner.placed.push(placed);
        placed
    }
}
