use crate::execution::Error;
use crate::graph::{InputPort, NodeId};
use crate::worker::EventRef;

#[derive(Debug, Clone)]
pub struct ExecutedNodeStats {
    pub node_id: NodeId,
    pub elapsed_secs: f64,
}

#[derive(Debug, Clone)]
pub struct NodeError {
    pub node_id: NodeId,
    pub error: Error,
}

#[derive(Debug)]
pub struct ExecutionStats {
    pub elapsed_secs: f64,

    pub executed_nodes: Vec<ExecutedNodeStats>,
    pub missing_inputs: Vec<InputPort>,
    pub cached_nodes: Vec<NodeId>,
    pub triggered_events: Vec<EventRef>,
    pub node_errors: Vec<NodeError>,
}
