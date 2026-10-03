//! The records a reversible removal leaves behind: everything needed to put
//! back exactly what was taken out.
//!
//! A [`DetachedNode`] carries a removed node with all the wiring that touched
//! it. It is produced by a `Graph` snapshot or detach and consumed by the
//! matching attach, so undo restores wiring rather than reconstructing it —
//! which is why it owns its contents instead of borrowing.
//!
//! It also owns what a well-formed record is: [`DetachedNode::new`] is the
//! one way to build one from outside, so a malformed record is refused where
//! it is made, not halfway through the graph edit.

use ::serde::{Deserialize, Serialize};

use crate::graph::BindingEntry;
use crate::graph::Subscription;
use crate::graph::error::DetachedNodeError;
use crate::graph::identity::NodeId;
use crate::graph::node::Node;

/// A node taken out of a graph, with every binding and subscription that
/// touched it, in the order the graph keeps them. Well formed by
/// construction: [`new`](Self::new) checks and orders a record from outside,
/// deserializing goes through it, and a graph builds one from its own tables.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "DetachedNodeFields")]
pub struct DetachedNode {
    pub(super) node_id: NodeId,
    pub(super) node: Node,
    pub(super) bindings: Vec<BindingEntry>,
    pub(super) subscriptions: Vec<Subscription>,
}

impl DetachedNode {
    /// A record of `node` under `node_id` with the wiring that touches it,
    /// put in the graph's order. Refused when the id is nil, or a binding or
    /// subscription does not touch the node or appears twice.
    pub fn new(
        node_id: NodeId,
        node: Node,
        mut bindings: Vec<BindingEntry>,
        mut subscriptions: Vec<Subscription>,
    ) -> Result<Self, DetachedNodeError> {
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
        Ok(Self {
            node_id,
            node,
            bindings,
            subscriptions,
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
}

/// A [`DetachedNode`] as serialized, before [`DetachedNode::new`] accepts it.
#[derive(Debug, Deserialize)]
struct DetachedNodeFields {
    node_id: NodeId,
    node: Node,
    bindings: Vec<BindingEntry>,
    subscriptions: Vec<Subscription>,
}

impl TryFrom<DetachedNodeFields> for DetachedNode {
    type Error = DetachedNodeError;

    fn try_from(fields: DetachedNodeFields) -> Result<Self, DetachedNodeError> {
        Self::new(
            fields.node_id,
            fields.node,
            fields.bindings,
            fields.subscriptions,
        )
    }
}
