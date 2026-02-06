mod cache;
mod cache_config;
mod config;
mod error;
pub mod progress;
pub mod rejection;
mod stack;

use strum_macros::Display;

// ========== Public API ==========
pub use config::{CombineMethod, Normalization, Rejection, StackConfig};
pub use stack::{stack, stack_with_progress};

pub use cache_config::CacheConfig;
pub use error::Error;
pub use progress::{ProgressCallback, StackingProgress, StackingStage};

// Re-export rejection types for advanced users
pub use rejection::{
    AsymmetricSigmaClipConfig, GesdConfig, LinearFitClipConfig, PercentileClipConfig,
    RejectionResult, SigmaClipConfig, WinsorizedClipConfig,
};

/// Type of calibration frame being stacked.
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
mod tests {
    use super::*;

    #[test]
    fn test_frame_type_display() {
        assert_eq!(FrameType::Dark.to_string(), "dark");
        assert_eq!(FrameType::Flat.to_string(), "flat");
        assert_eq!(FrameType::Bias.to_string(), "bias");
        assert_eq!(FrameType::Light.to_string(), "light");
    }
}
