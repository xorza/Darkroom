use graph::execution_graph::ArgumentValues;
use graph::execution_stats::ExecutionStats;
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

    pub fn invalidate_changed(&mut self, execution_stats: &ExecutionStats) {
        // Remove cached values for executed nodes (their values may have changed)
        for executed in &execution_stats.executed_nodes {
            self.values.remove(&executed.node_id);
        }

        // Remove cached values for nodes with missing inputs
        for port_address in &execution_stats.missing_inputs {
            self.values.remove(&port_address.target_id);
        }
    }
}
