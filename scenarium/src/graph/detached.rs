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

use crate::graph::binding_entry::BindingEntry;
use crate::graph::error::DetachedNodeError;
use crate::graph::ghost_node::GhostNode;
use crate::graph::ghost_wiring::GhostWiring;
use crate::graph::identity::NodeId;
use crate::graph::node::Node;
use crate::graph::subscription::Subscription;

/// A node taken out of a graph, with every binding and subscription that
/// touched it, in the order the graph keeps them, and each ghost that touched it. Well formed by
/// construction: [`new`](Self::new) checks and orders a record from outside,
/// deserializing goes through it, and a graph builds one from its own tables.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "DetachedNodeParts")]
pub struct DetachedNode {
    pub(super) node_id: NodeId,
    pub(super) node: Node,
    pub(super) bindings: Vec<BindingEntry>,
    pub(super) subscriptions: Vec<Subscription>,
    pub(super) ghost_wiring: GhostWiring,
}

impl DetachedNode {
    /// A record of `parts.node` under `parts.node_id` with the wiring that
    /// touches it, put in the graph's order. Refused when the id is nil, a
    /// binding, a subscription or a ghost does not touch the node, one appears twice, or an
    /// input or a subscription is both live and a ghost.
    pub fn new(parts: DetachedNodeParts) -> Result<Self, DetachedNodeError> {
        let DetachedNodeParts {
            node_id,
            node,
            mut bindings,
            mut subscriptions,
            ghost_wiring,
        } = parts;
        if node_id.is_nil() {
            return Err(DetachedNodeError::NilNodeId);
        }
        if let Some(entry) = bindings
            .iter()
            .find(|entry| !entry.binding.touches(entry.port, node_id))
        {
            return Err(DetachedNodeError::ForeignBinding { port: entry.port });
        }
        if let Some(subscription) = subscriptions
            .iter()
            .find(|subscription| !subscription.touches(node_id))
        {
            return Err(DetachedNodeError::ForeignSubscription {
                subscription: *subscription,
            });
        }
        bindings.sort_unstable_by_key(|entry| entry.port);
        if let Some(pair) = bindings
            .windows(2)
            .find(|pair| pair[0].port == pair[1].port)
        {
            return Err(DetachedNodeError::DuplicateBinding { port: pair[0].port });
        }
        subscriptions.sort_unstable();
        if let Some(pair) = subscriptions.windows(2).find(|pair| pair[0] == pair[1]) {
            return Err(DetachedNodeError::DuplicateSubscription {
                subscription: pair[0],
            });
        }
        if !ghost_wiring.all_touch(node_id) {
            return Err(DetachedNodeError::ForeignGhost);
        }
        if let Some(entry) = bindings
            .iter()
            .find(|entry| ghost_wiring.binding(entry.port).is_some())
        {
            return Err(DetachedNodeError::GhostOnBoundInput { port: entry.port });
        }
        if let Some(&subscription) = subscriptions
            .iter()
            .find(|subscription| ghost_wiring.subscription(**subscription).is_some())
        {
            return Err(DetachedNodeError::GhostOfSubscription { subscription });
        }
        Ok(Self {
            node_id,
            node,
            bindings,
            subscriptions,
            ghost_wiring,
        })
    }

    pub const fn node_id(&self) -> NodeId {
        self.node_id
    }

    pub const fn node(&self) -> &Node {
        &self.node
    }

    pub fn bindings(&self) -> &[BindingEntry] {
        &self.bindings
    }

    pub fn subscriptions(&self) -> &[Subscription] {
        &self.subscriptions
    }

    /// Each ghost that touched the node.
    pub const fn ghost_wiring(&self) -> &GhostWiring {
        &self.ghost_wiring
    }
}

/// The contents of a [`DetachedNode`] before [`DetachedNode::new`] checks and
/// orders them — what a caller assembles a record from, and what one
/// deserializes as.
#[derive(Clone, Debug, Deserialize)]
pub struct DetachedNodeParts {
    pub node_id: NodeId,
    pub node: Node,
    pub bindings: Vec<BindingEntry>,
    pub subscriptions: Vec<Subscription>,
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
