use crate::RamUsage;
use crate::execution::RunError;
use crate::execution::event::EventRef;
use crate::execution::identity::{ExecutionInputPort, ExecutionNodeId};

#[derive(Debug, Clone)]
pub struct ExecutedNodeStats {
    pub e_node_id: ExecutionNodeId,
    pub elapsed_secs: f64,
}

#[derive(Debug, Clone)]
pub struct NodeError {
    pub e_node_id: ExecutionNodeId,
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
    pub e_node_id: ExecutionNodeId,
    pub level: LogLevel,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeRamUsage {
    pub e_node_id: ExecutionNodeId,
    pub usage: RamUsage,
}

#[derive(Debug)]
pub struct ExecutionStats {
    pub elapsed_secs: f64,
    pub executed_nodes: Vec<ExecutedNodeStats>,
    pub missing_inputs: Vec<ExecutionInputPort>,
    pub cached_nodes: Vec<ExecutionNodeId>,
    pub triggered_events: Vec<EventRef>,
    pub node_errors: Vec<NodeError>,
    pub logs: Vec<LogEntry>,
    pub cancelled: bool,
    pub cache_ram: RamUsage,
    pub node_ram: Vec<NodeRamUsage>,
}
