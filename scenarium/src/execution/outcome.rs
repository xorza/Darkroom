use crate::RamUsage;
use crate::execution::RunError;
use crate::execution::identity::ExecutionEventPort;
use crate::execution::identity::{ExecutionInputPort, ExecutionNodeId};

#[derive(Debug, Clone)]
pub(crate) struct ExecutedNodeStats {
    pub e_node_id: ExecutionNodeId,
    pub elapsed_secs: f64,
}

#[derive(Debug, Clone)]
pub(crate) struct NodeError {
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
pub(crate) struct NodeRamUsage {
    pub e_node_id: ExecutionNodeId,
    pub usage: RamUsage,
}

#[derive(Debug)]
pub(crate) struct ExecutionOutcome {
    pub(crate) elapsed_secs: f64,
    pub(crate) executed_nodes: Vec<ExecutedNodeStats>,
    pub(crate) missing_inputs: Vec<ExecutionInputPort>,
    pub(crate) cached_nodes: Vec<ExecutionNodeId>,
    pub(crate) triggered_events: Vec<ExecutionEventPort>,
    pub(crate) node_errors: Vec<NodeError>,
    pub(crate) logs: Vec<LogEntry>,
    pub(crate) cancelled: bool,
    pub(crate) cache_ram: RamUsage,
    pub(crate) node_ram: Vec<NodeRamUsage>,
}
