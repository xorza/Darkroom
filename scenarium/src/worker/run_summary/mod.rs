//! What a completed run leaves behind, as the worker publishes it.

use std::sync::Arc;

use crate::RamUsage;
use crate::execution::report::{ExecutionOutcome, LogEntry, NodeStatus};
use crate::worker::activity::WorkerActivity;

/// One completed run: the worker's activity once it ended, the run's verdicts, one row per node
/// with something to report, the logs its lambdas wrote, and what the cache holds afterwards.
#[derive(Clone, Default, Debug)]
pub struct RunSummary {
    pub activity: WorkerActivity,
    /// How many nodes invoked their lambda, successes and failures alike.
    pub executed_node_count: usize,
    pub cancelled: bool,
    /// One row per node, in compiled order, so a node appears at most once.
    pub nodes: Vec<NodeStatus>,
    pub logs: Vec<LogEntry>,
    pub cache_ram: RamUsage,
}

/// Publishes each completed run into one retained allocation. A host that dropped the last
/// summary before the next run ends — the usual case — gets the same buffers back, so a run
/// under the event loop allocates nothing to report itself.
#[derive(Default, Debug)]
pub(crate) struct RunSummaryPublisher {
    summary: Arc<RunSummary>,
}

impl RunSummaryPublisher {
    /// Move `outcome`'s rows and logs into the summary, leaving the outcome's buffers empty for
    /// the next run.
    pub(crate) fn publish(
        &mut self,
        activity: WorkerActivity,
        outcome: &mut ExecutionOutcome,
    ) -> Arc<RunSummary> {
        // A summary still queued at the host cannot be written into, and `Arc::make_mut` would
        // deep-clone the vectors cleared below, so publish into a fresh one instead.
        if Arc::get_mut(&mut self.summary).is_none() {
            self.summary = Arc::default();
        }
        let summary = Arc::get_mut(&mut self.summary)
            .expect("the summary allocation is uniquely held after the swap above");
        summary.activity = activity;
        summary.executed_node_count = outcome.ran_node_count;
        summary.cancelled = outcome.cancelled;
        summary.nodes.clear();
        summary.nodes.append(&mut outcome.nodes);
        summary.logs.clear();
        summary.logs.append(&mut outcome.logs);
        summary.cache_ram = outcome.cache_ram;
        Arc::clone(&self.summary)
    }
}

#[cfg(test)]
mod tests;
