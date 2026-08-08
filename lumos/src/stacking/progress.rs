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
/// stage and a stage change is a real change of work. Which stages a run emits depends on the
/// entry point, and no run emits all of them: the raw pipeline detects stars while the decoded
/// frame is still in hand, so it folds that into `Calibrating` and never reports `Detecting`,
/// which belongs to the already-decoded path. Each stage's counter restarts at its own total.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackingStage {
    /// Decoding, calibrating and demosaicing raw lights, detecting each frame's stars as it
    /// lands. Counted in frames.
    Calibrating,
    /// Detecting stars in frames that arrived already decoded. Counted in frames.
    Detecting,
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
