use egui::TextureHandle;
use graph::execution_graph::ArgumentValues;
use graph::execution_stats::ExecutionStats;
use graph::graph::NodeId;
use hashbrown::HashMap;

#[derive(Default)]
pub struct ArgumentValuesCache {
    pub values: HashMap<NodeId, ArgumentValues>,
    pub preview_textures: HashMap<NodeId, HashMap<usize, TextureHandle>>,
}

impl std::fmt::Debug for ArgumentValuesCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArgumentValuesCache")
            .field("values_count", &self.values.len())
            .field("preview_textures_count", &self.preview_textures.len())
            .finish()
    }
}

impl ArgumentValuesCache {
    pub fn get(&self, node_id: &NodeId) -> Option<&ArgumentValues> {
        self.values.get(node_id)
    }

    pub fn insert(&mut self, node_id: NodeId, values: ArgumentValues) {
        self.values.insert(node_id, values);
    }

    pub fn get_textures(&mut self, node_id: &NodeId) -> &mut HashMap<usize, TextureHandle> {
        self.preview_textures.entry(*node_id).or_default()
    }

    pub fn clear(&mut self) {
        self.values.clear();
        self.preview_textures.clear();
    }

    pub fn invalidate_changed(&mut self, execution_stats: &ExecutionStats) {
        // Remove cached values for executed nodes (their values may have changed)
        for executed in &execution_stats.executed_nodes {
            self.values.remove(&executed.node_id);
            self.preview_textures.remove(&executed.node_id);
        }

        // Remove cached values for nodes with missing inputs
        for port_address in &execution_stats.missing_inputs {
            self.values.remove(&port_address.target_id);
            self.preview_textures.remove(&port_address.target_id);
        }
    }
}
