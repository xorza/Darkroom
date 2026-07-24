use crate::RamUsage;
use crate::execution::error::RunError;
use crate::execution::event::EventTrigger;
use crate::execution::identity::ExecutionEventPort;
use crate::execution::identity::{ExecutionInputPort, ExecutionNodeId};

#[derive(Debug, Clone)]
pub(crate) struct ExecutedNodeOutcome {
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

#[derive(Debug, Default)]
pub(crate) struct ExecutionOutcome {
    pub(crate) elapsed_secs: f64,
    pub(crate) executed_nodes: Vec<ExecutedNodeOutcome>,
    pub(crate) missing_inputs: Vec<ExecutionInputPort>,
    pub(crate) cached_nodes: Vec<ExecutionNodeId>,
    pub(crate) triggered_events: Vec<ExecutionEventPort>,
    pub(crate) event_triggers: Vec<EventTrigger>,
    pub(crate) node_errors: Vec<NodeError>,
    pub(crate) logs: Vec<LogEntry>,
    pub(crate) cancelled: bool,
    pub(crate) cache_ram: RamUsage,
    pub(crate) node_ram: Vec<NodeRamUsage>,
}

impl ExecutionOutcome {
    pub(crate) fn clear(&mut self) {
        self.elapsed_secs = 0.0;
        self.executed_nodes.clear();
        self.missing_inputs.clear();
        self.cached_nodes.clear();
        self.triggered_events.clear();
        self.event_triggers.clear();
        self.node_errors.clear();
        self.logs.clear();
        self.cancelled = false;
        self.cache_ram = RamUsage::default();
        self.node_ram.clear();
    }
}
