use egui::TextureHandle;
use hashbrown::HashMap;
use imaginarium::ImageDesc;
use scenarium::execution_graph::ArgumentValues;
use scenarium::execution_stats::ExecutionStats;
use scenarium::graph::NodeId;

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

/// Per-node cache lifecycle. A node is in exactly one state — or absent.
enum CacheState {
    Pending,
    Ready(NodeCache),
}

#[derive(Default)]
pub struct ArgumentValuesCache {
    entries: HashMap<NodeId, CacheState>,
}

impl std::fmt::Debug for ArgumentValuesCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ready = self
            .entries
            .values()
            .filter(|s| matches!(s, CacheState::Ready(_)))
            .count();
        f.debug_struct("ArgumentValuesCache")
            .field("ready_count", &ready)
            .field("pending_count", &(self.entries.len() - ready))
            .finish()
    }
}

impl ArgumentValuesCache {
    pub fn get_mut(&mut self, node_id: &NodeId) -> Option<&mut NodeCache> {
        match self.entries.get_mut(node_id)? {
            CacheState::Ready(cache) => Some(cache),
            CacheState::Pending => None,
        }
    }

    pub fn insert(&mut self, node_id: NodeId, node_cache: NodeCache) {
        self.entries.insert(node_id, CacheState::Ready(node_cache));
    }

    /// Returns true if this is a new request (not already pending or ready).
    /// Call before sending a request to avoid duplicates.
    pub fn mark_pending(&mut self, node_id: NodeId) -> bool {
        use hashbrown::hash_map::Entry;
        match self.entries.entry(node_id) {
            Entry::Occupied(_) => false,
            Entry::Vacant(slot) => {
                slot.insert(CacheState::Pending);
                true
            }
        }
    }

    /// Drop any state (pending or ready) for `node_id`.
    pub fn clear_pending(&mut self, node_id: NodeId) {
        self.entries.remove(&node_id);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn invalidate_changed(&mut self, execution_stats: &ExecutionStats) {
        for executed in &execution_stats.executed_nodes {
            self.entries.remove(&executed.node_id);
        }
        for error in &execution_stats.node_errors {
            self.entries.remove(&error.node_id);
        }
        for port_address in &execution_stats.missing_inputs {
            self.entries.remove(&port_address.target_id);
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
