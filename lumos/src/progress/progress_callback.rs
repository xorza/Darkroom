//! [`ProgressCallback`]: the caller's optional progress sink.

use std::fmt;
use std::sync::Arc;

use crate::progress::stacking_progress::{StackingProgress, StackingStage};

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

    use crate::progress::progress_callback::ProgressCallback;
    use crate::progress::stacking_progress::{StackingProgress, StackingStage};

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
