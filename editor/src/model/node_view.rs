use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
pub struct NodeView {
    pub id: Uuid,
    pub name: String,
    pub pos: egui::Pos2,
    pub inputs: Vec<Input>,
    pub outputs: Vec<Output>,
    pub cache_output: bool,
    pub has_cached_output: bool,
    // node has side effects, besides calculation it's output. e.g. saving re
    pub terminal: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Connection {
    pub node_id: Uuid,
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
        let id = Uuid::new_v4();
        let name = format!("NodeView {}", id);

        Self {
            id,
            name,
            pos: egui::Pos2::ZERO,
            inputs: Vec::new(),
            outputs: Vec::new(),
            cache_output: false,
            has_cached_output: false,
            terminal: false,
        }
    }
}
