//! [`ItemId`]: what the canvas selects, a node body or a wire tie.

use scenarium::NodeId;
use serde::{Deserialize, Serialize};

use crate::core::document::tie::TieId;

/// One item of the canvas, a node (a graph node or a stub) or a tie. The selection holds these,
/// so click, band select, Delete and the group drag treat a tie as they treat a node.
/// `darkroom/AGENTS.md`, "Wire ties", "A tie is an item", holds the decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub(crate) enum ItemId {
    Node(NodeId),
    Tie(TieId),
}

impl ItemId {
    /// The node this item is, or `None` for a tie.
    pub(crate) const fn node(self) -> Option<NodeId> {
        match self {
            Self::Node(node_id) => Some(node_id),
            Self::Tie(_) => None,
        }
    }
}

impl From<NodeId> for ItemId {
    fn from(node_id: NodeId) -> Self {
        Self::Node(node_id)
    }
}

impl From<TieId> for ItemId {
    fn from(tie: TieId) -> Self {
        Self::Tie(tie)
    }
}
