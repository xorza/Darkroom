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
mod tests;
