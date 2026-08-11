//! Progress reporting for stacking operations.

use std::fmt;
use std::sync::Arc;

/// Progress information for stacking operations.
#[derive(Debug, Clone)]
pub struct StackingProgress {
    /// Current step (0-based).
    pub current: usize,
    /// Total number of steps.
    pub total: usize,
    /// Description of current operation.
    pub stage: StackingStage,
}

/// The pass a [`StackingProgress`] report belongs to.
///
/// One variant per pass that walks a countable set, so `current`/`total` mean one thing within a
/// stage and a stage change is a real change of work. Each stage's counter restarts at its own
/// total, and a run emits only the stages its route uses — but which stages those are follows from
/// the work asked for, not from which function was called: both front ends report `Preparing` and
/// `Registering`, a statistical combine reports `Loading` and `Combining` where a drizzle reports
/// `Drizzling`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackingStage {
    /// Turning inputs into frames with detected stars: decoding, calibrating and demosaicing where
    /// the input is raw, and detecting each frame's stars either way. Counted in frames.
    ///
    /// One stage rather than one per activity because it is one pass: the raw path detects while
    /// the decoded frame is still in hand, so a frame is reported once whichever route it took.
    Preparing,
    /// Registering each frame against the reference and warping it into place. Counted in
    /// frames, and the reference itself is not among them.
    Registering,
    /// Reading frames into the combine's cache, spilling them to disk on the streaming tier.
    /// Counted in frames.
    Loading,
    /// Walking the output in row chunks and reducing the frames into it. Counted in
    /// chunk-channel pairs, not frames.
    Combining,
    /// Accumulating frames into the drizzle grid. Counted in frames.
    Drizzling,
}

type ProgressFn = dyn Fn(StackingProgress) + Send + Sync;

/// Optional shared callback for progress reporting.
#[derive(Clone, Default)]
pub struct ProgressCallback {
    callback: Option<Arc<ProgressFn>>,
}

impl ProgressCallback {
    pub fn new(callback: impl Fn(StackingProgress) + Send + Sync + 'static) -> Self {
        Self {
            callback: Some(Arc::new(callback)),
        }
    }

    /// Report one step of `stage`. A default callback reports nowhere.
    pub(crate) fn report(&self, current: usize, total: usize, stage: StackingStage) {
        if let Some(callback) = &self.callback {
            callback(StackingProgress {
                current,
                total,
                stage,
            });
        }
    }
}

impl fmt::Debug for ProgressCallback {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("ProgressCallback")
            .field(&self.callback.as_ref().map(|_| "<set>"))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use crate::stacking::progress::{ProgressCallback, StackingProgress, StackingStage};

    #[test]
    fn callback_reports_exact_progress_and_default_is_silent() {
        ProgressCallback::default().report(1, 2, StackingStage::Loading);

        let reports = Arc::new(Mutex::new(Vec::new()));
        let callback = ProgressCallback::new({
            let reports = Arc::clone(&reports);
            move |progress| reports.lock().unwrap().push(progress)
        });
        callback.report(3, 5, StackingStage::Combining);

        let reports = reports.lock().unwrap();
        let [
            StackingProgress {
                current,
                total,
                stage,
            },
        ] = reports.as_slice()
        else {
            panic!("expected one progress report");
        };
        assert_eq!((*current, *total, *stage), (3, 5, StackingStage::Combining));
    }
}
