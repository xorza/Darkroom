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

/// Where a run's live feedback goes: node progress before and after each lambda, and pinned
/// outputs the moment they are produced or reused. The run loop calls this **directly**, on
/// its own thread, in the order the events happen — so a report is published as it occurs
/// rather than when a relay next gets polled, and a pinned value is handed over instead of
/// waiting in a queue holding its RAM.
///
/// `Send` because the run future crosses threads; `Debug` so the structs carrying one still
/// derive it.
pub(crate) trait RunReporter: Send + std::fmt::Debug {
    fn progress(&mut self, progress: RunProgress);

    fn pinned(&mut self, outputs: PinnedOutputs);
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::execution::report::{PinnedOutputs, RunProgress, RunReporter};

    /// Discards a run's live feedback, for the tests that only assert on the final outcome.
    /// Production always has a host listening, so this stays test-only.
    #[derive(Debug, Default)]
    pub(crate) struct DiscardedReports;

    impl RunReporter for DiscardedReports {
        fn progress(&mut self, _progress: RunProgress) {}

        fn pinned(&mut self, _outputs: PinnedOutputs) {}
    }

    /// Records everything a run reports, in order, for tests that assert on live feedback.
    #[derive(Debug, Default)]
    pub(crate) struct CollectingReporter {
        pub(crate) progress: Vec<RunProgress>,
        pub(crate) pinned: Vec<PinnedOutputs>,
    }

    impl CollectingReporter {
        /// The first pinned push, panicking when the run delivered none.
        pub(crate) fn expect_pinned(&mut self) -> PinnedOutputs {
            assert!(
                !self.pinned.is_empty(),
                "the run must deliver a pinned output"
            );
            self.pinned.remove(0)
        }
    }

    impl RunReporter for CollectingReporter {
        fn progress(&mut self, progress: RunProgress) {
            self.progress.push(progress);
        }

        fn pinned(&mut self, outputs: PinnedOutputs) {
            self.pinned.push(outputs);
        }
    }
}
