//! [`NodeKey`]: a key that names the node it belongs to.

use std::fmt::Debug;
use std::hash::Hash;

use scenarium::NodeId;

/// A key that names the node it hangs off — how a cache keyed by ports,
/// events or pins evicts a deleted node's entries, and how a gesture keyed by
/// one notices its node disappearing. A node id is its own node.
pub(crate) trait NodeKey: Copy + Eq + Hash + Debug {
    fn node(self) -> NodeId;
}

impl NodeKey for NodeId {
    fn node(self) -> NodeId {
        self
    }
}
