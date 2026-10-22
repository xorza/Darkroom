//! [`RunReporter`]: where a run's live feedback goes, and the [`RunPhase`] it reports.

use std::fmt::Debug;
use std::time::Instant;

use crate::graph::identity::NodeId;

/// Where one node's lambda is, live: started at `at`, or finished after `elapsed_secs` with or
/// without its outputs. A cancelled lambda reports no finish — it did not complete.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RunPhase {
    Started { at: Instant },
    Succeeded { elapsed_secs: f64 },
    Failed { elapsed_secs: f64 },
}

/// Where a run's live feedback goes: node progress before and after each lambda. The run
/// loop calls this **directly**, on its own thread, in the order the events happen — so a
/// report is published as it occurs rather than when a relay next gets polled.
///
/// `Send` because the run future crosses threads; `Debug` so the structs carrying one still
/// derive it.
pub(crate) trait RunReporter: Send + Debug {
    fn progress(&mut self, node_id: NodeId, phase: RunPhase);
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::exec::report::run_reporter::{RunPhase, RunReporter};
    use crate::graph::identity::NodeId;

    /// Discards a run's live feedback, for the tests that only assert on the final outcome.
    /// Production always has a host listening, so this stays test-only.
    #[derive(Debug, Default)]
    pub(crate) struct DiscardedReports;

    impl RunReporter for DiscardedReports {
        fn progress(&mut self, _node_id: NodeId, _phase: RunPhase) {}
    }

    /// Records everything a run reports, in order, for tests that assert on live feedback.
    #[derive(Debug, Default)]
    pub(crate) struct CollectingReporter {
        pub(crate) progress: Vec<(NodeId, RunPhase)>,
    }

    impl RunReporter for CollectingReporter {
        fn progress(&mut self, node_id: NodeId, phase: RunPhase) {
            self.progress.push((node_id, phase));
        }
    }
}
