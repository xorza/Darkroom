//! What one run reports: live progress while it runs, and the completed
//! outcome it hands back.
//!
//! The two halves meet here because they are the same subject at two moments.
//! [`RunReporter`](run_reporter::RunReporter) is called on the run's own thread as each lambda
//! starts and finishes, so a host sees progress as it happens;
//! [`ExecOutcome`](exec_outcome::ExecOutcome) is what the run leaves behind — one
//! [`NodeStatus`](node_status::NodeStatus) row per node, the
//! [`LogEntry`](crate::runtime::log_entry::LogEntry)s written along the way, and the
//! [`EventTrigger`](event_trigger::EventTrigger)s a successful event source armed.
//!
//! [`NodeStatus`](node_status::NodeStatus) is the row the executor emits *and* the row the host
//! consumes — [`RunSummary`](crate::worker::run_summary::RunSummary) publishes it verbatim. A node
//! appears at most once, so nothing downstream has to reassemble it from several lists, and no
//! consumer's fold order can change what a node's result was.

pub(crate) mod event_trigger;
pub(crate) mod exec_outcome;
pub(crate) mod node_status;
pub(crate) mod run_reporter;
