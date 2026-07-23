use std::time::Instant;

use crate::DynamicValue;
use crate::execution::identity::ExecutionNodeId;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum RunPhase {
    Started { at: Instant },
    Finished { elapsed_secs: f64 },
}

#[derive(Debug, Clone)]
pub(crate) struct RunProgress {
    pub(crate) e_node_id: ExecutionNodeId,
    pub(crate) phase: RunPhase,
}

#[derive(Debug, Clone)]
pub struct PinnedOutput {
    pub port_idx: usize,
    pub value: DynamicValue,
}

#[derive(Debug, Clone)]
pub struct PinnedOutputs {
    pub e_node_id: ExecutionNodeId,
    pub values: Vec<PinnedOutput>,
}

#[derive(Debug, Clone)]
pub(crate) enum RunEvent {
    Progress(RunProgress),
    PinnedOutputs(PinnedOutputs),
}
