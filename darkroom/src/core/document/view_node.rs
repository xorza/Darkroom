use common::KeyIndexKey;
use glam::Vec2;
use scenarium::graph::{Node, NodeId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ViewNode {
    pub id: NodeId,
    pub pos: Vec2,
}

impl Eq for ViewNode {}

impl From<&Node> for ViewNode {
    fn from(node: &Node) -> Self {
        Self {
            id: node.id,
            pos: Vec2::ZERO,
        }
    }
}

impl KeyIndexKey<NodeId> for ViewNode {
    fn key(&self) -> &NodeId {
        &self.id
    }
}
