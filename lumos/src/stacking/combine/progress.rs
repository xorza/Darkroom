//! Progress reporting for stacking operations.

use common::SharedFn;

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

/// Callback type for progress reporting.
pub type ProgressCallback = SharedFn<dyn Fn(StackingProgress) + Send + Sync>;

/// Report progress using the callback if set.
pub fn report_progress(
    callback: &ProgressCallback,
    current: usize,
    total: usize,
    stage: StackingStage,
) {
    if let Some(f) = callback.as_ref() {
        f(StackingProgress {
            current,
            total,
            stage,
        });
    }
}
