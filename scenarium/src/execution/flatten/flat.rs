//! The stable-id intermediate representation passed from flattening to
//! linking.
//!
//! This module owns the contract between the two stages. Flattening appends
//! func-only nodes and id-named edges; linking consumes the value whole,
//! assigns dense indices, and produces the final program.
//!
//! What has a stage-local type here is what the walk can settle but only in the
//! stable-id space — which is one thing: an input's binding, which names a
//! producer linking has not placed yet.
//!
//! Everything else is either already the program's own type or not here at all.
//! A node is an [`ExecutionNode`], because a walk settles every field of one.
//! An output port is its resolved [`DataType`], because the walk resolves the
//! graph's whole output-type table anyway to gate its bindings. Event ports are
//! a reserved run, because a lambda is the library's to state and linking holds
//! the library. An event's subscribers are a side list, because the slot to
//! write belongs to the emitter rather than to the node the walk is standing
//! on.

use crate::common::column::{Column, Span};
use crate::execution::compiled::ExecutionNode;
use crate::execution::identity::{
    EventIdx, ExecutionEventPort, ExecutionNodeId, ExecutionOutputPort, InputIdx, OutputIdx,
};
use crate::{DataType, StaticValue};

/// One flatten's whole output: a flat, func-only graph in the stable-id
/// space.
///
/// The nodes are already [`ExecutionNode`]s: a walk can settle every field of
/// one, so there is no separate node type here to restate them and no field for
/// linking to translate. What the walk *cannot* settle is where a node lands —
/// that is the id sort's — so the two columns below stay in emit order and
/// name no `NodeIdx`, exactly like the input pool names no `OutputAddr`.
///
/// Event ports are *reserved* rather than emitted: a lambda is the library's to
/// state and linking holds the library, so carrying a copy through here would
/// buy a translation and nothing else. The walk settles only what it alone
/// knows — how many event ports each node owns, which is what fixes the run.
#[derive(Debug, Default)]
pub(crate) struct FlatGraph {
    /// The nodes, in emit order. Parallel to [`e_node_ids`](Self::e_node_ids) —
    /// the same split the finished program keeps, since one id sort places both
    /// at once.
    pub(crate) e_nodes: Vec<ExecutionNode>,
    pub(crate) e_node_ids: Vec<ExecutionNodeId>,
    /// The one port pool the walk fills: an input's binding names a producer by
    /// id, which is the one thing about a port the library cannot state.
    pub(crate) inputs: Column<InputIdx, FlatInput>,
    /// Each output port's effective type, wildcards already followed. The walk
    /// resolves the graph's whole table to gate its bindings, so answering here
    /// costs a lookup and saves linking a second walk of the same chains over
    /// its own index space.
    pub(crate) outputs: Column<OutputIdx, DataType>,
    /// Event ports reserved so far, so each node's run is fixed before its
    /// lambdas exist — see [`reserve_events`](Self::reserve_events).
    pub(crate) events: u32,
    /// Event edges flattening resolved but cannot place: the slot to write
    /// belongs to the emitter, which is still named only by id. A data edge
    /// lives directly in the consumer's input as [`FlatBinding::Bind`].
    pub(crate) subscriptions: Vec<PendingSubscription>,
}

impl FlatGraph {
    /// Empty every buffer, keeping capacity, so one `FlatGraph` on the
    /// [`Compiler`](crate::execution::compile::Compiler) serves every compile
    /// instead of one. Leaves the value indistinguishable from
    /// [`Default`] — a flatten that starts here cannot observe the last one.
    pub(crate) fn clear(&mut self) {
        self.e_nodes.clear();
        self.e_node_ids.clear();
        self.inputs.clear();
        self.outputs.clear();
        self.events = 0;
        self.subscriptions.clear();
    }

    /// Append one emitted node across both columns at once — the only way they
    /// are written, so they cannot fall out of step.
    ///
    /// Id uniqueness is enforced where linking places these, so this just
    /// appends.
    pub(crate) fn push_node(&mut self, id: ExecutionNodeId, e_node: ExecutionNode) {
        self.e_nodes.push(e_node);
        self.e_node_ids.push(id);
    }

    /// Claim one node's event ports.
    ///
    /// The run is all the walk can say about them: which lambda sits in each is
    /// the library's answer, and linking holds the library. Reserving keeps the
    /// runs packed in emit order all the same, so a placed node owns the run its
    /// flat self claimed.
    pub(crate) fn reserve_events(&mut self, len: usize) -> Span<EventIdx> {
        let start = self.events;
        self.events = start
            .checked_add(u32::try_from(len).expect("a node's port count must fit in u32"))
            .expect("a program's port count must fit in u32");
        Span::new(start, len as u32)
    }
}

/// One input port with the binding flattening resolved for it.
#[derive(Debug)]
pub(crate) struct FlatInput {
    pub(crate) required: bool,
    pub(crate) stamps_fs_path: bool,
    pub(crate) binding: FlatBinding,
}

/// A binding in the stable-id space. `Bind` names the producer by id because
/// dense placement belongs to linking.
#[derive(Debug)]
pub(crate) enum FlatBinding {
    None,
    Const(StaticValue),
    Bind(ExecutionOutputPort),
}

/// A resolved event edge by stable id.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PendingSubscription {
    pub(crate) event: ExecutionEventPort,
    pub(crate) subscriber: ExecutionNodeId,
}

#[cfg(any(test, feature = "internals"))]
pub(crate) mod internals {
    use crate::execution::compiled::ExecutionNode;
    use crate::execution::flatten::flat::FlatGraph;
    use crate::execution::identity::ExecutionNodeId;

    /// The test-only way to build a [`FlatGraph`] outside the walk: bare nodes
    /// and nothing else.
    #[derive(Debug, Default)]
    pub(crate) struct FlatGraphBuilder {
        flat: FlatGraph,
    }

    impl FlatGraphBuilder {
        /// A default node is exactly the bare one a fixture wants: no ports, no
        /// lambda, a nil func id.
        pub(crate) fn insert_node(&mut self, e_node_id: ExecutionNodeId) {
            self.flat.push_node(e_node_id, ExecutionNode::default());
        }

        pub(crate) fn build(self) -> FlatGraph {
            self.flat
        }
    }
}
