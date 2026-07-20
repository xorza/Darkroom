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

/// Stage of stacking operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackingStage {
    /// Loading images into memory or writing to disk cache.
    Loading,
    /// Processing chunks during stacking.
    Processing,
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
}

impl fmt::Debug for ProgressCallback {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("ProgressCallback")
            .field(&self.callback.as_ref().map(|_| "<set>"))
            .finish()
    }
}

pub(crate) fn report_progress(
    callback: &ProgressCallback,
    current: usize,
    total: usize,
    stage: StackingStage,
) {
    if let Some(callback) = &callback.callback {
        callback(StackingProgress {
            current,
            total,
            stage,
        });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use crate::stacking::progress::{
        ProgressCallback, StackingProgress, StackingStage, report_progress,
    };

    #[test]
    fn callback_reports_exact_progress_and_default_is_silent() {
        report_progress(&ProgressCallback::default(), 1, 2, StackingStage::Loading);

        let reports = Arc::new(Mutex::new(Vec::new()));
        let callback = ProgressCallback::new({
            let reports = Arc::clone(&reports);
            move |progress| reports.lock().unwrap().push(progress)
        });
        report_progress(&callback, 3, 5, StackingStage::Processing);

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
        assert_eq!(
            (*current, *total, *stage),
            (3, 5, StackingStage::Processing)
        );
    }
}
