pub(crate) mod cache;
mod cache_config;
pub(crate) mod config;
pub(crate) mod error;
pub mod progress;
pub mod rejection;
pub(crate) mod stack;

use strum_macros::Display;

// ========== Public API ==========
pub use config::{CombineMethod, Normalization, StackConfig, Weighting};
pub use rejection::Rejection;
pub use stack::{stack, stack_with_progress};

pub use cache_config::CacheConfig;
pub use error::Error;
pub use progress::{ProgressCallback, StackingProgress, StackingStage};

// Re-export rejection types for advanced users
pub use rejection::{
    GesdConfig, LinearFitClipConfig, PercentileClipConfig, SigmaClipConfig, WinsorizedClipConfig,
};

/// Type of calibration frame being stacked.
///
/// Used for logging and error messages. Does not affect stacking algorithm
/// behavior — stacking method, rejection, and normalization are controlled
/// by [`StackConfig`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Display)]
#[strum(serialize_all = "lowercase")]
pub enum FrameType {
    /// Dark frames - thermal noise calibration
    Dark,
    /// Flat frames - vignetting and dust correction
    Flat,
    /// Bias frames - readout noise calibration
    Bias,
    /// Light frames - actual image data
    Light,
}

#[cfg(test)]
mod bench;
#[cfg(test)]
mod tests;
