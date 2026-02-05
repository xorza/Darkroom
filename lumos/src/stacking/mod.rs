mod cache;
mod cache_config;
mod config;
mod error;
mod local_normalization;
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
    GesdConfig, LinearFitClipConfig, PercentileClipConfig, RejectionResult, SigmaClipConfig,
    WinsorizedClipConfig,
};

// Re-export normalization types for advanced users
#[allow(unused_imports)]
pub use local_normalization::{
    LocalNormalizationConfig, LocalNormalizationMap, NormalizationMethod, TileNormalizationStats,
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
    use std::path::PathBuf;

    // ========== StackConfig Tests ==========

    #[test]
    fn test_stack_config_default() {
        let config = StackConfig::default();
        assert_eq!(config.method, CombineMethod::Mean);
        assert!(matches!(config.rejection, Rejection::SigmaClip { .. }));
    }

    #[test]
    fn test_stack_config_presets() {
        let config = StackConfig::sigma_clipped(2.5);
        assert_eq!(config.method, CombineMethod::Mean);
        assert!(matches!(
            config.rejection,
            Rejection::SigmaClip { sigma, .. } if (sigma - 2.5).abs() < f32::EPSILON
        ));

        let config = StackConfig::median();
        assert_eq!(config.method, CombineMethod::Median);
        assert_eq!(config.rejection, Rejection::None);

        let config = StackConfig::mean();
        assert_eq!(config.method, CombineMethod::Mean);
        assert_eq!(config.rejection, Rejection::None);
    }

    // ========== Error Path Tests ==========

    #[test]
    fn test_stack_empty_paths_returns_no_paths_error() {
        let result = stack(
            &Vec::<PathBuf>::new(),
            FrameType::Dark,
            StackConfig::default(),
        );
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::NoPaths));
    }

    #[test]
    fn test_stack_nonexistent_file() {
        let paths = vec![PathBuf::from("/nonexistent/stack_image.fits")];
        let result = stack(&paths, FrameType::Dark, StackConfig::default());

        assert!(result.is_err());
        match result.unwrap_err() {
            Error::ImageLoad { path, .. } => {
                assert!(path.to_string_lossy().contains("nonexistent"));
            }
            e => panic!("Expected ImageLoad error, got {:?}", e),
        }
    }

    #[test]
    fn test_stack_median_nonexistent_file() {
        let paths = vec![PathBuf::from("/nonexistent/median_stack.fits")];
        let result = stack(&paths, FrameType::Flat, StackConfig::median());

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::ImageLoad { .. }));
    }

    #[test]
    fn test_stack_sigma_clipped_nonexistent_file() {
        let paths = vec![PathBuf::from("/nonexistent/sigma_stack.fits")];
        let result = stack(&paths, FrameType::Bias, StackConfig::sigma_clipped(2.5));

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::ImageLoad { .. }));
    }

    // ========== Configuration Tests ==========

    #[test]
    fn test_frame_type_display() {
        assert_eq!(FrameType::Dark.to_string(), "dark");
        assert_eq!(FrameType::Flat.to_string(), "flat");
        assert_eq!(FrameType::Bias.to_string(), "bias");
        assert_eq!(FrameType::Light.to_string(), "light");
    }

    #[test]
    fn test_rejection_sigma_clip_config() {
        let config = StackConfig {
            rejection: Rejection::SigmaClip {
                sigma: 3.0,
                iterations: 5,
            },
            ..Default::default()
        };
        config.validate();
    }

    #[test]
    #[should_panic(expected = "Sigma must be positive")]
    fn test_rejection_zero_sigma_panics() {
        let config = StackConfig {
            rejection: Rejection::SigmaClip {
                sigma: 0.0,
                iterations: 3,
            },
            ..Default::default()
        };
        config.validate();
    }

    #[test]
    #[should_panic(expected = "Sigma must be positive")]
    fn test_rejection_negative_sigma_panics() {
        let config = StackConfig {
            rejection: Rejection::SigmaClip {
                sigma: -1.0,
                iterations: 3,
            },
            ..Default::default()
        };
        config.validate();
    }

    #[test]
    #[should_panic(expected = "Iterations must be at least 1")]
    fn test_rejection_zero_iterations_panics() {
        let config = StackConfig {
            rejection: Rejection::SigmaClip {
                sigma: 2.0,
                iterations: 0,
            },
            ..Default::default()
        };
        config.validate();
    }
}
