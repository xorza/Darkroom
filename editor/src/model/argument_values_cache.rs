use graph::execution_graph::ArgumentValues;
use graph::graph::NodeId;
use hashbrown::HashMap;

#[derive(Debug, Default)]
pub struct ArgumentValuesCache {
    pub values: HashMap<NodeId, ArgumentValues>,
}

impl ArgumentValuesCache {
    pub fn get(&self, node_id: &NodeId) -> Option<&ArgumentValues> {
        self.values.get(node_id)
    }

    pub fn insert(&mut self, node_id: NodeId, values: ArgumentValues) {
        self.values.insert(node_id, values);
    }

    pub fn clear(&mut self) {
        self.values.clear();
    }
}
