//! [`NodeKey`]: a key that names the node it belongs to.

use std::fmt::Debug;
use std::hash::Hash;

use scenarium::{EventPort, NodeId};

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

impl NodeKey for EventPort {
    fn node(self) -> NodeId {
        self.node_id
    }
}

#[cfg(test)]
mod tests {
    use scenarium::{EventPort, NodeId};

    use crate::gui::node_key::NodeKey;
    use crate::gui::port_ref::{PortKind, PortRef};

    /// A port and an emitter event belong to their node, and a subscription pin *is* its node,
    /// since a subscription is whole-node: that identity is what lets one glyph drag span all
    /// three.
    #[test]
    fn a_node_key_names_the_node_its_glyph_hangs_off() {
        let node = NodeId::unique();
        assert_eq!(
            PortRef {
                node_id: node,
                kind: PortKind::Input,
                port_idx: 3,
            }
            .node(),
            node
        );
        assert_eq!(
            EventPort {
                node_id: node,
                event_idx: 1,
            }
            .node(),
            node
        );
        assert_eq!(node.node(), node);
    }
}
