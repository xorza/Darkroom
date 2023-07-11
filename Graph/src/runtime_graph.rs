use serde::{Deserialize, Serialize};

use crate::graph::{FunctionBehavior, NodeId};

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct RuntimeOutput {
    pub binding_count: u32,
}

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct RuntimeNode {
    pub(crate) node_id: NodeId,

    pub name: String,

    pub outputs: Vec<RuntimeOutput>,
    pub is_output: bool,

    pub has_missing_inputs: bool,
    pub behavior: FunctionBehavior,
}


#[derive(Default, Serialize, Deserialize)]
pub struct RuntimeGraph {
    pub nodes: Vec<RuntimeNode>,
}


impl RuntimeNode {
    pub fn node_id(&self) -> NodeId {
        self.node_id
    }
}

impl RuntimeGraph {
    pub fn node_by_name(&self, name: &str) -> Option<&RuntimeNode> {
        self.nodes.iter().find(|&p_node| p_node.name == name)
    }

    pub fn node_by_id(&self, node_id: NodeId) -> &RuntimeNode {
        self.nodes.iter()
            .find(|&p_node| p_node.node_id == node_id)
            .unwrap()
    }
    pub fn node_by_id_mut(&mut self, node_id: NodeId) -> &mut RuntimeNode {
        self.nodes.iter_mut()
            .find(|p_node| p_node.node_id == node_id)
            .unwrap()
    }
}

