//! The records a reversible removal leaves behind: everything needed to put
//! back exactly what was taken out.
//!
//! A [`DetachedNode`] carries a removed node with all the wiring that touched it, live and ghost,
//! produced by a `Graph` snapshot or detach and consumed by the matching attach, so undo restores
//! wiring rather than reconstructing it. A [`DetachedGhost`] is the same for a ghost node, which
//! holds no live wiring.
//!
//! [`DetachedNode::new`] is the one way to build one from outside, so a malformed record is
//! refused where it is made, not halfway through the graph edit.

use ::serde::{Deserialize, Serialize};

use crate::graph::error::DetachedNodeError;
use crate::graph::ghost_node::GhostNode;
use crate::graph::ghost_wiring::GhostWiring;
use crate::graph::identity::NodeId;
use crate::graph::node::Node;
use crate::graph::wiring::Wiring;

/// A node taken out of a graph, with every binding and subscription that touched it and each
/// ghost that touched it. Well formed by construction: [`new`](Self::new) checks a record from
/// outside, deserializing goes through it, and a graph builds one from its own tables.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "DetachedNodeParts")]
pub struct DetachedNode {
    pub(super) node_id: NodeId,
    pub(super) node: Node,
    pub(super) wiring: Wiring,
    pub(super) ghost_wiring: GhostWiring,
}

impl DetachedNode {
    /// A record of `parts.node` under `parts.node_id` with the wiring that touches it. Refused
    /// when the id is nil, a binding, a subscription or a ghost does not touch the node, or an
    /// input or a subscription is both live and a ghost.
    pub fn new(parts: DetachedNodeParts) -> Result<Self, DetachedNodeError> {
        let DetachedNodeParts {
            node_id,
            node,
            wiring,
            ghost_wiring,
        } = parts;
        if node_id.is_nil() {
            return Err(DetachedNodeError::NilNodeId);
        }
        if let Some((port, _)) = wiring
            .bindings()
            .find(|(port, binding)| !binding.touches(*port, node_id))
        {
            return Err(DetachedNodeError::ForeignBinding { port });
        }
        if let Some(subscription) = wiring
            .subscriptions()
            .find(|subscription| !subscription.touches(node_id))
        {
            return Err(DetachedNodeError::ForeignSubscription { subscription });
        }
        if !ghost_wiring.all_touch(node_id) {
            return Err(DetachedNodeError::ForeignGhost);
        }
        if let Some((port, _)) = wiring
            .bindings()
            .find(|(port, _)| ghost_wiring.binding(*port).is_some())
        {
            return Err(DetachedNodeError::GhostOnBoundInput { port });
        }
        if let Some(subscription) = wiring
            .subscriptions()
            .find(|subscription| ghost_wiring.subscription(*subscription).is_some())
        {
            return Err(DetachedNodeError::GhostOfSubscription { subscription });
        }
        Ok(Self {
            node_id,
            node,
            wiring,
            ghost_wiring,
        })
    }

    pub const fn node_id(&self) -> NodeId {
        self.node_id
    }

    pub const fn node(&self) -> &Node {
        &self.node
    }

    /// Each binding and subscription that touched the node.
    pub const fn wiring(&self) -> &Wiring {
        &self.wiring
    }

    /// Each ghost that touched the node.
    pub const fn ghost_wiring(&self) -> &GhostWiring {
        &self.ghost_wiring
    }
}

/// The contents of a [`DetachedNode`] before [`DetachedNode::new`] checks them — what a caller
/// assembles a record from, and what one deserializes as.
#[derive(Clone, Debug, Deserialize)]
pub struct DetachedNodeParts {
    pub node_id: NodeId,
    pub node: Node,
    /// Each binding and subscription that touches the node.
    #[serde(default)]
    pub wiring: Wiring,
    /// Each ghost that touches the node: none for a node an edit authors.
    #[serde(default)]
    pub ghost_wiring: GhostWiring,
}

impl TryFrom<DetachedNodeParts> for DetachedNode {
    type Error = DetachedNodeError;

    fn try_from(parts: DetachedNodeParts) -> Result<Self, DetachedNodeError> {
        Self::new(parts)
    }
}

/// A ghost node taken out of a graph, with each ghost that touched it: what a removal of one
/// takes out and an undo puts back. A graph builds one from its own tables.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetachedGhost {
    pub(super) node_id: NodeId,
    pub(super) ghost: GhostNode,
    pub(super) wiring: GhostWiring,
}

impl DetachedGhost {
    pub const fn node_id(&self) -> NodeId {
        self.node_id
    }

    pub const fn ghost(&self) -> &GhostNode {
        &self.ghost
    }

    pub const fn wiring(&self) -> &GhostWiring {
        &self.wiring
    }
}
