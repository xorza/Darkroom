//! [`NodeRef`]: a node with its id attached.

use std::ops::Deref;

use crate::graph::identity::NodeId;
use crate::graph::node::Node;

/// A [`Node`] with its id attached — the item [`Graph::iter`](crate::graph::Graph::iter) yields.
///
/// Iteration is the one lookup whose caller does not already hold the id, so it is the one that has
/// to hand one back; a node stores no id of its own, so there would otherwise be nothing to read it
/// from. [`Graph::find`](crate::graph::Graph::find) and
/// [`Graph::find_mut`](crate::graph::Graph::find_mut) are *given* the id and return the node alone.
/// That is the whole rule — not an inconsistency to reconcile.
#[derive(Clone, Copy, Debug)]
pub struct NodeRef<'a> {
    pub id: NodeId,
    /// `pub(super)` so only the graph module's lookups mint one; readers go
    /// through [`Deref`], which is the point of the type.
    pub(super) node: &'a Node,
}

impl Deref for NodeRef<'_> {
    type Target = Node;

    fn deref(&self) -> &Self::Target {
        self.node
    }
}
