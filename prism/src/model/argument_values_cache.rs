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

/// Cache mutations the worker layer signals to the renderer.
/// `Session` queues these in `drain_inbound` / `refresh_graph`; the
/// renderer drains them at frame start. Keeps the cache UI-owned
/// without making `Session` a transport for non-UI state.
pub enum CacheEvent {
    /// Drop entries whose stats moved (executed, errored, missing inputs).
    InvalidateNodes(Vec<NodeId>),
    /// Worker returned values for a previously-pending request.
    Insert(NodeId, NodeCache),
    /// Worker reported "no values available" for a pending request.
    ClearPending(NodeId),
    /// Graph was replaced or refreshed — drop everything.
    Clear,
}

impl std::fmt::Debug for CacheEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidateNodes(ids) => {
                f.debug_tuple("InvalidateNodes").field(&ids.len()).finish()
            }
            Self::Insert(id, _) => f.debug_tuple("Insert").field(id).finish(),
            Self::ClearPending(id) => f.debug_tuple("ClearPending").field(id).finish(),
            Self::Clear => f.debug_tuple("Clear").finish(),
        }
    }
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

    pub fn apply(&mut self, event: CacheEvent) {
        match event {
            CacheEvent::InvalidateNodes(ids) => {
                for id in ids {
                    self.entries.remove(&id);
                }
            }
            CacheEvent::Insert(id, cache) => self.insert(id, cache),
            CacheEvent::ClearPending(id) => self.clear_pending(id),
            CacheEvent::Clear => self.clear(),
        }
    }
}

/// Collect every node id whose state moved in this execution — used to
/// build [`CacheEvent::InvalidateNodes`] without dragging
/// `ExecutionStats` through the event queue.
pub fn invalidated_nodes(execution_stats: &ExecutionStats) -> Vec<NodeId> {
    let mut ids = Vec::with_capacity(
        execution_stats.executed_nodes.len()
            + execution_stats.node_errors.len()
            + execution_stats.missing_inputs.len(),
    );
    ids.extend(execution_stats.executed_nodes.iter().map(|n| n.node_id));
    ids.extend(execution_stats.node_errors.iter().map(|n| n.node_id));
    ids.extend(execution_stats.missing_inputs.iter().map(|p| p.target_id));
    ids
}

impl From<ArgumentValues> for NodeCache {
    fn from(values: ArgumentValues) -> Self {
        NodeCache {
            arg_values: values,
            ..Default::default()
        }
    }
}
