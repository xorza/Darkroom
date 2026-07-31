//! What one run reports: live progress while it runs, and the completed
//! outcome it hands back.
//!
//! The two halves meet here because they are the same subject at two moments.
//! [`RunReporter`] is called on the run's own thread as each lambda starts and
//! finishes, so a host sees progress as it happens; [`ExecutionOutcome`] is
//! what the run leaves behind — one [`NodeStatus`] row per node, the
//! [`LogEntry`]s written along the way, and the [`EventTrigger`]s a successful
//! event source armed.
//!
//! [`NodeStatus`] is the row the executor emits *and* the row the host consumes —
//! [`WorkerStatus`](crate::worker::status::WorkerStatus) publishes it verbatim. A node
//! appears at most once, so nothing downstream has to reassemble it from several lists,
//! and no consumer's fold order can change what a node's result was.

use std::time::Instant;

use crate::RamUsage;
use crate::execution::error::RunError;
use crate::graph::func::event::EventLambda;
use crate::graph::identity::{EventPort, NodeId};
use crate::runtime::shared_any_state::SharedAnyState;

/// Where a log record came from and how loud it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

/// One node-attributed log record.
/// [`ContextManager::log`](crate::runtime::context::ContextManager::log) writes
/// these while a lambda runs; the outcome below carries them out, and
/// [`WorkerStatus`](crate::worker::status::WorkerStatus) republishes them.
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub node_id: NodeId,
    pub level: LogLevel,
    pub message: String,
}

/// One `(event, lambda, state)` triple a successful event-source run armed,
/// spawned by the worker as a looping task. Carried out on
/// [`ExecutionOutcome::event_triggers`], which is why it lives beside it: the
/// executor produces these without knowing the worker that consumes them.
#[derive(Debug)]
pub(crate) struct EventTrigger {
    pub(crate) event: EventPort,
    pub(crate) lambda: EventLambda,
    pub(crate) state: SharedAnyState,
}

/// What one node did this run, at the host boundary — the published projection of the
/// executor's private per-node verdict.
#[derive(Clone, Debug)]
pub enum NodeExecutionStatus {
    /// Live only: the node's lambda is running, since `at`.
    Running {
        at: Instant,
    },
    /// Served from a cache, or left resident by the pre-run cut — available, not recomputed.
    Cached,
    Executed {
        elapsed_secs: f64,
    },
    /// The run could not satisfy these input ports, by index on the node. Never empty:
    /// this status exists because at least one required port went unfed.
    MissingInputs {
        ports: Vec<usize>,
    },
    /// `elapsed_secs` is `Some` when the lambda ran and failed, timing the attempt, and
    /// `None` when the node never ran at all — an errored dependency, a func with no
    /// implementation, or a cached output that no longer loads.
    Errored {
        elapsed_secs: Option<f64>,
        error: RunError,
    },
}

/// One node's whole result for one run. `status` is `None` for a node the run had nothing
/// to say about that nonetheless holds RAM — a value resident from an earlier run, which
/// this run neither recomputed nor released.
#[derive(Clone, Debug)]
pub struct NodeStatus {
    pub node_id: NodeId,
    pub status: Option<NodeExecutionStatus>,
    pub ram: RamUsage,
}

#[derive(Debug, Default)]
pub(crate) struct ExecutionOutcome {
    pub(crate) elapsed_secs: f64,
    /// One row per node with something to report — a status, retained RAM, or both.
    /// CompiledGraph order, so a node cannot appear twice.
    pub(crate) nodes: Vec<NodeStatus>,
    /// How many nodes invoked their lambda, successes and failures alike. Counted while
    /// the rows are built rather than derived from them, since a failed node's row no
    /// longer says "executed" separately.
    pub(crate) ran_node_count: usize,
    pub(crate) triggered_events: Vec<EventPort>,
    pub(crate) event_triggers: Vec<EventTrigger>,
    pub(crate) logs: Vec<LogEntry>,
    pub(crate) cancelled: bool,
    pub(crate) cache_ram: RamUsage,
}

impl ExecutionOutcome {
    pub(crate) fn clear(&mut self) {
        self.elapsed_secs = 0.0;
        self.nodes.clear();
        self.ran_node_count = 0;
        self.triggered_events.clear();
        self.event_triggers.clear();
        self.logs.clear();
        self.cancelled = false;
        self.cache_ram = RamUsage::default();
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum RunPhase {
    Started { at: Instant },
    Finished { elapsed_secs: f64 },
}

#[derive(Debug, Clone)]
pub(crate) struct RunProgress {
    pub(crate) node_id: NodeId,
    pub(crate) phase: RunPhase,
}

/// Where a run's live feedback goes: node progress before and after each lambda. The run
/// loop calls this **directly**, on its own thread, in the order the events happen — so a
/// report is published as it occurs rather than when a relay next gets polled.
///
/// `Send` because the run future crosses threads; `Debug` so the structs carrying one still
/// derive it.
pub(crate) trait RunReporter: Send + std::fmt::Debug {
    fn progress(&mut self, progress: RunProgress);
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::RamUsage;
    use crate::execution::error::RunError;
    use crate::execution::report::{ExecutionOutcome, NodeExecutionStatus};
    use crate::graph::identity::NodeId;

    impl ExecutionOutcome {
        /// This node's row, or `None` when the run reported nothing for it.
        pub(crate) fn status(&self, node_id: NodeId) -> Option<&NodeExecutionStatus> {
            self.nodes
                .iter()
                .find(|node| node.node_id == node_id)?
                .status
                .as_ref()
        }

        /// Whether the node invoked its lambda and succeeded.
        pub(crate) fn ran(&self, node_id: NodeId) -> bool {
            matches!(
                self.status(node_id),
                Some(NodeExecutionStatus::Executed { .. })
            )
        }

        /// Every node that invoked its lambda and succeeded.
        pub(crate) fn ran_nodes(&self) -> impl Iterator<Item = NodeId> + '_ {
            self.nodes
                .iter()
                .filter(|node| matches!(node.status, Some(NodeExecutionStatus::Executed { .. })))
                .map(|node| node.node_id)
        }

        /// Whether the node was served from a cache instead of recomputing.
        pub(crate) fn cached(&self, node_id: NodeId) -> bool {
            matches!(self.status(node_id), Some(NodeExecutionStatus::Cached))
        }

        /// This node's failure, or `None` when it did not fail.
        pub(crate) fn error(&self, node_id: NodeId) -> Option<&RunError> {
            match self.status(node_id)? {
                NodeExecutionStatus::Errored { error, .. } => Some(error),
                _ => None,
            }
        }

        /// Every node the run reported a failure for.
        pub(crate) fn errored_nodes(&self) -> impl Iterator<Item = NodeId> + '_ {
            self.nodes
                .iter()
                .filter(|node| matches!(node.status, Some(NodeExecutionStatus::Errored { .. })))
                .map(|node| node.node_id)
        }

        /// Every node the run reported a missing input for.
        pub(crate) fn missing_input_nodes(&self) -> impl Iterator<Item = NodeId> + '_ {
            self.nodes
                .iter()
                .filter(|node| {
                    matches!(node.status, Some(NodeExecutionStatus::MissingInputs { .. }))
                })
                .map(|node| node.node_id)
        }

        /// RAM this node's resident output holds after the run.
        pub(crate) fn node_ram(&self, node_id: NodeId) -> RamUsage {
            self.nodes
                .iter()
                .find(|node| node.node_id == node_id)
                .map(|node| node.ram)
                .unwrap_or_default()
        }
    }

    use crate::execution::report::{RunProgress, RunReporter};

    /// Discards a run's live feedback, for the tests that only assert on the final outcome.
    /// Production always has a host listening, so this stays test-only.
    #[derive(Debug, Default)]
    pub(crate) struct DiscardedReports;

    impl RunReporter for DiscardedReports {
        fn progress(&mut self, _progress: RunProgress) {}
    }

    /// Records everything a run reports, in order, for tests that assert on live feedback.
    #[derive(Debug, Default)]
    pub(crate) struct CollectingReporter {
        pub(crate) progress: Vec<RunProgress>,
    }

    impl RunReporter for CollectingReporter {
        fn progress(&mut self, progress: RunProgress) {
            self.progress.push(progress);
        }
    }
}
