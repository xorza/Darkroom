use serde::{Deserialize, Serialize};

use common::id_type;

use crate::data::DataType;
use crate::graph::{Graph, NodeId};

id_type!(SubGraphId);

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct SubInputNodeConnection {
    pub subnode_id: NodeId,
    pub subnode_input_index: u32,
}

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct SubInput {
    pub name: String,
    pub data_type: DataType,
    pub is_required: bool,
    pub connections: Vec<SubInputNodeConnection>,
}

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct SubOutput {
    pub name: String,
    pub data_type: DataType,
    pub subnode_id: NodeId,
    pub subnode_output_index: u32,
}

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct SubGraph {
    self_id: SubGraphId,

    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<SubInput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<SubOutput>,
}


impl SubGraph {
    pub fn new() -> SubGraph {
        SubGraph {
            self_id: SubGraphId::unique(),

            name: "".to_string(),
            inputs: vec![],
            outputs: vec![],
        }
    }

    pub fn id(&self) -> SubGraphId {
        self.self_id
    }
}
impl Graph {
    pub fn add_subgraph(&mut self, subgraph: &SubGraph) {
        match self
            .subgraphs_mut()
            .iter()
            .position(|sg| sg.id() == subgraph.id()) {
            Some(index) => self.subgraphs_mut()[index] = subgraph.clone(),
            None => self.subgraphs_mut().push(subgraph.clone()),
        }
    }
    pub fn remove_subgraph_by_id(&mut self, id: SubGraphId) {
        assert!(!id.is_nil());

        self.subgraphs_mut()
            .retain(|subgraph| subgraph.id() != id);

        self.nodes()
            .iter()
            .filter(|node| node.subgraph_id == Some(id))
            .map(|node| node.id())
            .collect::<Vec<NodeId>>()
            .iter()
            .cloned()
            .for_each(|node_id| {
                self.remove_node_by_id(node_id);
            });
    }

    pub fn subgraph_by_id_mut(&mut self, id: SubGraphId) -> Option<&mut SubGraph> {
        assert!(!id.is_nil());
        self.subgraphs_mut()
            .iter_mut()
            .find(|subgraph| subgraph.id() == id)
    }
    pub fn subgraph_by_id(&self, id: SubGraphId) -> Option<&SubGraph> {
        assert!(!id.is_nil());
        self.subgraphs()
            .iter()
            .find(|subgraph| subgraph.id() == id)
    }
}