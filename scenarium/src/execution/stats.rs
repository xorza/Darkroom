use crate::RamUsage;
use crate::execution::RunError;
use crate::execution::event::EventRef;
use crate::graph::{InputPort, NodeId};

#[derive(Debug, Clone)]
pub struct ExecutedNodeStats {
    pub node_id: NodeId,
    pub elapsed_secs: f64,
}

#[derive(Debug, Clone)]
pub struct NodeError {
    pub node_id: NodeId,
    pub error: RunError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub node_id: NodeId,
    pub level: LogLevel,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeRamUsage {
    pub node_id: NodeId,
    pub usage: RamUsage,
}

#[derive(Debug)]
pub struct ExecutionStats {
    pub elapsed_secs: f64,
    pub executed_nodes: Vec<ExecutedNodeStats>,
    pub missing_inputs: Vec<InputPort>,
    pub cached_nodes: Vec<NodeId>,
    pub triggered_events: Vec<EventRef>,
    pub node_errors: Vec<NodeError>,
    pub logs: Vec<LogEntry>,
    pub cancelled: bool,
    pub cache_ram: RamUsage,
    pub node_ram: Vec<NodeRamUsage>,
}
