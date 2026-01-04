use common::key_index_vec::KeyIndexKey;
use graph::graph::NodeId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct ViewNode {
    pub id: NodeId,
    pub pos: egui::Pos2,
}

impl Default for ViewNode {
    fn default() -> Self {
        let id = NodeId::unique();

        Self {
            id,
            pos: egui::Pos2::ZERO,
        }
    }
}
impl KeyIndexKey<NodeId> for ViewNode {
    fn key(&self) -> &NodeId {
        &self.id
    }
}
