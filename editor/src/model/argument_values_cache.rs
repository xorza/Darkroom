use egui::TextureHandle;
use graph::execution_graph::ArgumentValues;
use graph::execution_stats::ExecutionStats;
use graph::graph::NodeId;
use hashbrown::HashMap;
use imaginarium::ImageDesc;

#[derive(Clone)]
pub struct CachedTexture {
    pub desc: ImageDesc,
    pub handle: TextureHandle,
}

#[derive(Default)]
pub struct NodeCache {
    pub arg_values: ArgumentValues,
    pub input_previews: Vec<Option<CachedTexture>>,
    pub output_previews: Vec<Option<CachedTexture>>,
}

#[derive(Default)]
pub struct ArgumentValuesCache {
    pub values: HashMap<NodeId, NodeCache>,
}

impl std::fmt::Debug for ArgumentValuesCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArgumentValuesCache")
            .field("values_count", &self.values.len())
            .finish()
    }
}

impl ArgumentValuesCache {
    pub fn get_mut(&mut self, node_id: &NodeId) -> Option<&mut NodeCache> {
        self.values.get_mut(node_id)
    }

    pub fn insert(&mut self, node_id: NodeId, node_cache: NodeCache) {
        self.values.insert(node_id, node_cache);
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

impl From<ArgumentValues> for NodeCache {
    fn from(values: ArgumentValues) -> Self {
        NodeCache {
            arg_values: values,
            ..Default::default()
        }
    }
}
