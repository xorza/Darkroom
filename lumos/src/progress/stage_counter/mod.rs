//! [`StageCounter`]: one stage's completed units, counted by parallel workers.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::progress::progress_callback::ProgressCallback;
use crate::progress::stacking_progress::StackingStage;

/// A stage's completed-unit count that parallel workers share.
///
/// The callback sees `1, 2, …, total` in order and one at a time, however the workers finish, and
/// is never called under a lock: a callback that runs rayon work of its own, whose worker may steal
/// a unit that completes on the same thread, cannot deadlock against itself. Whichever worker finds
/// no report under way reports every count not yet reported; one that finds a report under way
/// leaves its count to it, and the reporter looks again after it stands down.
#[derive(Debug)]
pub(crate) struct StageCounter<'a> {
    progress: &'a ProgressCallback,
    stage: StackingStage,
    total: usize,
    done: AtomicUsize,
    /// The last count the callback was handed; only the reporter touches it.
    reported: AtomicUsize,
    reporting: AtomicBool,
}

impl<'a> StageCounter<'a> {
    pub(crate) const fn new(
        progress: &'a ProgressCallback,
        stage: StackingStage,
        total: usize,
    ) -> Self {
        Self {
            progress,
            stage,
            total,
            done: AtomicUsize::new(0),
            reported: AtomicUsize::new(0),
            reporting: AtomicBool::new(false),
        }
    }

    /// Count one more unit done and report it; the count after this one.
    pub(crate) fn complete_one(&self) -> usize {
        // Sequentially consistent throughout: a worker's increment, then its failed claim, must be
        // seen by the load the reporter makes after it stands down.
        let done = self.done.fetch_add(1, Ordering::SeqCst) + 1;
        debug_assert!(
            done <= self.total,
            "{:?} counted past its total",
            self.stage
        );
        while !self.reporting.swap(true, Ordering::SeqCst) {
            let mut reported = self.reported.load(Ordering::Relaxed);
            let target = self.done.load(Ordering::SeqCst);
            while reported < target {
                reported += 1;
                self.progress.report(reported, self.total, self.stage);
            }
            self.reported.store(reported, Ordering::Relaxed);
            self.reporting.store(false, Ordering::SeqCst);
            if self.done.load(Ordering::SeqCst) == reported {
                break;
            }
        }
        done
    }
}

#[cfg(test)]
mod tests;
