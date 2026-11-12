//! [`NodeRef`]: a node with its id attached.

use std::ops::Deref;

use crate::graph::identity::NodeId;
use crate::graph::node::Node;

/// A [`Node`] with its id attached — the item [`Graph::iter`](crate::graph::Graph::iter) yields.
///
/// Iteration is the one lookup whose caller does not already hold the id, and a node stores none.
/// [`Graph::find`](crate::graph::Graph::find) and
/// [`Graph::find_mut`](crate::graph::Graph::find_mut) are *given* the id and return the node alone.
#[derive(Clone, Copy, Debug)]
pub struct NodeRef<'a> {
    pub node_id: NodeId,
    /// `pub(super)` so only the graph module's lookups mint one; readers go through [`Deref`].
    pub(super) node: &'a Node,
}

impl Deref for NodeRef<'_> {
    type Target = Node;

    fn deref(&self) -> &Self::Target {
        self.node
    }
}
