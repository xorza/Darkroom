//! [`StageCounter`]: one stage's completed units, counted by parallel workers.

use parking_lot::Mutex;

use crate::stacking::progress::{ProgressCallback, StackingStage};

/// A stage's completed-unit count that parallel workers share.
///
/// The count and its report move together under one lock, so the callback sees `1, 2, …, total`
/// in order and one at a time, however the workers finish. A slow callback therefore holds up the
/// workers that report through it; it runs once per unit, not per sample.
#[derive(Debug)]
pub(crate) struct StageCounter<'a> {
    progress: &'a ProgressCallback,
    stage: StackingStage,
    total: usize,
    done: Mutex<usize>,
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
            done: Mutex::new(0),
        }
    }

    /// Count one more unit done and report it; the count after this one.
    pub(crate) fn complete_one(&self) -> usize {
        let mut done = self.done.lock();
        *done += 1;
        debug_assert!(
            *done <= self.total,
            "{:?} counted past its total",
            self.stage
        );
        self.progress.report(*done, self.total, self.stage);
        *done
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use rayon::prelude::*;

    use crate::stacking::progress::stage_counter::StageCounter;
    use crate::stacking::progress::{ProgressCallback, StackingStage};

    /// Workers finishing in any order still hand the callback `1..=total`, in order.
    #[test]
    fn parallel_completions_report_in_order() {
        let reports = Arc::new(Mutex::new(Vec::new()));
        let callback = ProgressCallback::new({
            let reports = Arc::clone(&reports);
            move |progress| {
                reports
                    .lock()
                    .unwrap()
                    .push((progress.current, progress.total));
            }
        });
        let counter = StageCounter::new(&callback, StackingStage::Loading, 64);
        (0..64).into_par_iter().for_each(|_| {
            counter.complete_one();
        });
        let expected: Vec<_> = (1..=64).map(|current| (current, 64)).collect();
        assert_eq!(*reports.lock().unwrap(), expected);
    }
}
