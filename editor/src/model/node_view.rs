use common::key_index_vec::KeyIndexKey;
use egui::Pos2;
use graph::graph::NodeId;
use serde::{Deserialize, Serialize};

use crate::common::UiEquals;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewNode {
    pub id: NodeId,
    pub pos: Pos2,
}

impl Default for ViewNode {
    fn default() -> Self {
        let id = NodeId::unique();

        Self {
            id,
            pos: Pos2::ZERO,
        }
    }
}
impl KeyIndexKey<NodeId> for ViewNode {
    fn key(&self) -> &NodeId {
        &self.id
    }
}

impl PartialEq for ViewNode {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.pos.ui_equals(&other.pos)
    }
}

impl Eq for ViewNode {}
