use std::time::Instant;

use crate::DynamicValue;
use crate::execution::identity::NodeAddress;
use crate::graph::NodeId;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RunPhase {
    Started { at: Instant },
    Finished { elapsed_secs: f64 },
}

#[derive(Debug, Clone)]
pub struct RunProgress {
    pub node_id: NodeId,
    pub phase: RunPhase,
}

#[derive(Debug, Clone)]
pub struct PinnedOutput {
    pub port_idx: usize,
    pub value: DynamicValue,
}

#[derive(Debug, Clone)]
pub struct PinnedOutputs {
    pub node: NodeAddress,
    pub values: Vec<PinnedOutput>,
}

#[derive(Debug, Clone)]
pub enum RunEvent {
    Progress(RunProgress),
    PinnedOutputs(PinnedOutputs),
}
