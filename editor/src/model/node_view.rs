use graph::graph::NodeId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct NodeView {
    pub id: NodeId,
    pub pos: egui::Pos2,
}

impl Default for NodeView {
    fn default() -> Self {
        let id = NodeId::unique();

        Self {
            id,
            pos: egui::Pos2::ZERO,
        }
    }
}
