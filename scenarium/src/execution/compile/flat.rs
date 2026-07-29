//! The stable-id intermediate representation passed from flattening to
//! linking.
//!
//! This module owns the contract between the two stages. Flattening appends
//! func-only nodes and id-named edges; linking consumes the value whole,
//! assigns dense indices, and produces the final program. Neither stage owns
//! the other's implementation types.

use crate::common::pool::{Pool, PoolRange};
use crate::execution::identity::{ExecutionEventPort, ExecutionNodeId, ExecutionOutputPort};
use crate::execution::source_map::{Leaf, ScopeTable};
use crate::graph::func::FuncBehavior;
use crate::graph::func::event::EventLambda;
use crate::graph::func::lambda::FuncLambda;
use crate::graph::identity::{FuncId, NodeId};
use crate::graph::node::CacheMode;
use crate::graph::node::special::SpecialNode;
use crate::{DataType, StaticValue};

/// One flatten's whole output: a flat, func-only graph in the stable-id space.
/// Composites and boundary nodes are gone, their edges short-circuited to the
/// func nodes at either end.
///
/// Everything copied out of the library travels with it — lambdas,
/// declaration flags, and declared port types — so linking needs no library.
/// Nothing here names a dense index; `nodes` remains in emit order.
#[derive(Debug)]
pub(super) struct FlatGraph {
    pub(super) nodes: Vec<FlatNode>,
    pub(super) inputs: Pool<FlatInput>,
    pub(super) outputs: Pool<FlatOutput>,
    pub(super) events: Pool<FlatEvent>,
    /// Event edges flattening resolved but cannot place: the slot to write
    /// belongs to the emitter, which is still named only by id. A data edge
    /// lives directly in the consumer's input as [`FlatBinding::Bind`].
    pub(super) subscriptions: Vec<PendingSubscription>,
    /// The instance ancestry the nodes' leaves point into.
    pub(super) scopes: ScopeTable,
    /// `(graph instance, execution node behind one interface output)`, one
    /// entry per wired exposed port per occurrence.
    ///
    /// This cannot be recovered after flattening dissolves `GraphOutput`
    /// edges. Without it, an exposed producer also read inside its instance is
    /// indistinguishable from interior plumbing.
    pub(super) exposed: Vec<(NodeId, ExecutionNodeId)>,
}

impl Default for FlatGraph {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            inputs: Pool::default(),
            outputs: Pool::default(),
            events: Pool::default(),
            subscriptions: Vec::new(),
            scopes: ScopeTable::open(),
            exposed: Vec::new(),
        }
    }
}

/// One flat node: its stable identity, authored origin, topology, and code.
///
/// The leaf rides with the node so linking's one id sort places both the
/// program and attribution column in the same dense order.
#[derive(Debug)]
pub(super) struct FlatNode {
    pub(super) id: ExecutionNodeId,
    pub(super) leaf: Leaf,
    pub(super) sink: bool,
    pub(super) disabled: bool,
    pub(super) behavior: FuncBehavior,
    pub(super) cache: CacheMode,
    pub(super) special: Option<SpecialNode>,
    pub(super) inputs: PoolRange<FlatInput>,
    pub(super) outputs: PoolRange<FlatOutput>,
    pub(super) events: PoolRange<FlatEvent>,
    pub(super) func_id: FuncId,
    pub(super) version: u32,
    pub(super) lambda: FuncLambda,
}

/// One input port with the binding flattening resolved for it.
#[derive(Debug)]
pub(super) struct FlatInput {
    pub(super) required: bool,
    pub(super) stamps_fs_path: bool,
    pub(super) binding: FlatBinding,
}

/// A binding in the stable-id space. `Bind` names the producer by id because
/// dense placement belongs to linking.
#[derive(Debug)]
pub(super) enum FlatBinding {
    None,
    Const(StaticValue),
    Bind(ExecutionOutputPort),
}

/// One output port declaration before wildcard types are linked.
#[derive(Debug)]
pub(super) enum FlatOutput {
    Fixed(DataType),
    /// Mirrors input `mirrors` on the same node. `mirrored_declared` is copied
    /// along so a constant mirror needs no library during linking.
    Wildcard {
        mirrors: u32,
        mirrored_declared: DataType,
    },
}

/// One event port before its id-named subscribers are placed.
#[derive(Debug)]
pub(super) struct FlatEvent {
    pub(super) lambda: EventLambda,
}

/// A resolved event edge by stable id.
#[derive(Debug, Clone, Copy)]
pub(super) struct PendingSubscription {
    pub(super) event: ExecutionEventPort,
    pub(super) subscriber: ExecutionNodeId,
}

#[cfg(any(test, feature = "internals"))]
pub(crate) mod internals {
    use crate::common::pool::PoolRange;
    use crate::execution::compile::flat::{FlatGraph, FlatNode};
    use crate::execution::identity::ExecutionNodeId;
    use crate::execution::source_map::Leaf;
    use crate::graph::func::FuncBehavior;
    use crate::graph::func::lambda::FuncLambda;
    use crate::graph::identity::{FuncId, NodeId};
    use crate::graph::node::CacheMode;

    /// The test-only way to build a [`FlatGraph`] outside the walk: bare nodes
    /// carrying attribution and nothing else.
    #[derive(Debug, Default)]
    pub(crate) struct FlatGraphBuilder {
        flat: FlatGraph,
    }

    impl FlatGraphBuilder {
        pub(crate) fn insert_leaf(
            &mut self,
            e_node_id: ExecutionNodeId,
            instances: impl IntoIterator<Item = NodeId>,
            node_id: NodeId,
        ) {
            let mut scope = 0;
            for instance in instances {
                scope = self.flat.scopes.push(instance, scope);
            }
            self.flat.nodes.push(FlatNode {
                id: e_node_id,
                leaf: Leaf { scope, node_id },
                sink: false,
                disabled: false,
                behavior: FuncBehavior::default(),
                cache: CacheMode::default(),
                special: None,
                inputs: PoolRange::default(),
                outputs: PoolRange::default(),
                events: PoolRange::default(),
                func_id: FuncId::nil(),
                version: 0,
                lambda: FuncLambda::default(),
            });
        }

        pub(crate) fn build(self) -> FlatGraph {
            self.flat
        }
    }
}
