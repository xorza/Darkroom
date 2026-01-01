use graph::{graph::NodeId, prelude::NodeBehavior};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
pub struct NodeView {
    pub id: NodeId,
    pub name: String,
    pub pos: egui::Pos2,
    pub inputs: Vec<Input>,
    pub outputs: Vec<Output>,
    pub behavior: NodeBehavior,
    pub terminal: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Connection {
    pub node_id: NodeId,
    pub output_index: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Input {
    pub name: String,
    pub connection: Option<Connection>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Output {
    pub name: String,
}

impl Default for NodeView {
    fn default() -> Self {
        let id = NodeId::unique();
        let name = format!("NodeView {}", id);

        Self {
            id,
            name,
            pos: egui::Pos2::ZERO,
            inputs: Vec::new(),
            outputs: Vec::new(),
            behavior: NodeBehavior::AsFunction,
            terminal: false,
        }
    }
}
