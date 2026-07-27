use std::time::Instant;

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
