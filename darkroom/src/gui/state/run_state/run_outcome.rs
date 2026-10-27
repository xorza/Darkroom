//! [`RunOutcome`]: what the last completed run came to, as the status bar reports it.

use scenarium::{NodeExecStatus, NodeId, RunError, RunSummary};

/// The tally of one completed run's rows, and the node a reader should look at first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RunOutcome {
    /// Nodes whose lambda ran and returned.
    pub(crate) ran_count: usize,
    /// Nodes served from a cache.
    pub(crate) cached_count: usize,
    /// Nodes that failed on their own: the lambda failed, the node has no implementation, or
    /// its cached output no longer loads. Not the nodes that failure stopped.
    pub(crate) failed_count: usize,
    /// Nodes that never ran because a node before them failed.
    pub(crate) skipped_count: usize,
    /// Nodes the run could not feed an input.
    pub(crate) missing_count: usize,
    /// The first node that failed in the run's order, else the first one missing an input: the
    /// cause a reader starts from.
    pub(crate) cause: Option<NodeId>,
    pub(crate) cancelled: bool,
}

impl RunOutcome {
    /// The outcome of `summary`, whose rows are in the compiled order.
    pub(crate) fn of(summary: &RunSummary) -> Self {
        let mut outcome = Self {
            cancelled: summary.cancelled,
            ..Self::default()
        };
        let mut first_missing = None;
        for row in &summary.nodes {
            match &row.status {
                None => {}
                Some(NodeExecStatus::Executed { .. }) => outcome.ran_count += 1,
                Some(NodeExecStatus::Cached) => outcome.cached_count += 1,
                Some(NodeExecStatus::MissingInputs { .. }) => {
                    outcome.missing_count += 1;
                    first_missing.get_or_insert(row.node_id);
                }
                Some(NodeExecStatus::Errored {
                    error: RunError::SkippedUpstream,
                }) => outcome.skipped_count += 1,
                Some(NodeExecStatus::Errored { .. }) => {
                    outcome.failed_count += 1;
                    outcome.cause.get_or_insert(row.node_id);
                }
            }
        }
        outcome.cause = outcome.cause.or(first_missing);
        outcome
    }
}
