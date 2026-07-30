//! The records a reversible removal leaves behind: everything needed to put
//! back exactly what was taken out.
//!
//! A [`DetachedNode`] carries a removed node with all the wiring that touched
//! it. It is produced by a `Graph` snapshot or detach and consumed by the
//! matching attach, so undo restores wiring rather than reconstructing it —
//! which is why it owns its contents instead of borrowing.
//!
//! It also knows what a well-formed record looks like: the `assert_*` methods
//! here are the preflight a mutation runs before it commits, so a malformed
//! record fails at the call that supplied it rather than halfway through the
//! graph edit.

use ::serde::{Deserialize, Serialize};

use crate::graph::BindingEntry;
use crate::graph::Subscription;
use crate::graph::identity::NodeId;
use crate::graph::node::Node;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DetachedNode {
    pub node_id: NodeId,
    pub node: Node,
    pub bindings: Vec<BindingEntry>,
    pub subscriptions: Vec<Subscription>,
}

impl DetachedNode {
    /// Panic unless this record is well formed: a real node id, and wiring that
    /// touches it, listed in the ascending order the graph's own side tables
    /// keep. A malformed record is a caller logic error, never user input.
    pub(super) fn assert_valid(&self) {
        assert!(!self.node_id.is_nil(), "detached node id must not be nil");
        assert!(
            self.bindings
                .iter()
                .all(|entry| entry.binding.touches(entry.port, self.node_id)),
            "detached bindings must touch the detached node"
        );
        assert!(
            self.bindings
                .windows(2)
                .all(|entries| entries[0].port < entries[1].port),
            "detached bindings must have unique ordered ports"
        );
        assert!(
            self.subscriptions
                .iter()
                .all(|subscription| subscription.touches(self.node_id)),
            "detached subscriptions must touch the detached node"
        );
        assert!(
            self.subscriptions
                .windows(2)
                .all(|subscriptions| subscriptions[0] < subscriptions[1]),
            "detached subscriptions must be unique and ordered"
        );
    }
}
