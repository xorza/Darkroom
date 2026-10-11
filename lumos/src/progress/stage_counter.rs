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
mod tests {
    use std::sync::{Arc, Mutex, mpsc};
    use std::thread;
    use std::time::Duration;

    use rayon::prelude::*;

    use crate::progress::progress_callback::ProgressCallback;
    use crate::progress::stacking_progress::StackingStage;
    use crate::progress::stage_counter::StageCounter;

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

    /// A callback that runs rayon work of its own still sees every count in order: a worker waiting
    /// for the parts of its loop that others took steals units of the stage meanwhile, which
    /// complete on the same thread and must not wait for the report they are inside. A watchdog
    /// fails the test where a deadlock would hang it.
    #[test]
    fn a_callback_that_uses_rayon_completes() {
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let reports = Arc::new(Mutex::new(Vec::new()));
            let callback = ProgressCallback::new({
                let reports = Arc::clone(&reports);
                move |progress| {
                    (0..8).into_par_iter().for_each(|_| {
                        thread::sleep(Duration::from_micros(20));
                    });
                    reports.lock().unwrap().push(progress.current);
                }
            });
            let counter = StageCounter::new(&callback, StackingStage::Loading, 256);
            (0..256).into_par_iter().for_each(|_| {
                counter.complete_one();
            });
            sender.send(reports.lock().unwrap().clone()).unwrap();
        });
        let reports = receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("the stage deadlocked in its progress callback");
        assert_eq!(reports, (1..=256).collect::<Vec<_>>());
    }
}
