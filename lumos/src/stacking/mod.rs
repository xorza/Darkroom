mod cache;
mod cache_config;
mod error;
mod mean;
mod median;
mod sigma_clipped;

use std::path::PathBuf;

use strum_macros::Display;

pub use cache_config::CacheConfig;
pub use error::Error;
pub use median::MedianConfig;
pub use sigma_clipped::SigmaClippedConfig;

#[cfg(feature = "bench")]
pub mod bench {
    pub use super::mean::bench as mean;
    pub use super::median::bench as median;
    pub use super::sigma_clipped::bench as sigma_clipped;
}

use crate::AstroImage;

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

/// Method used for combining multiple frames during stacking.
#[derive(Debug, Clone, PartialEq)]
pub enum StackingMethod {
    /// Average all pixel values. Fast but sensitive to outliers.
    Mean,
    /// Take the median pixel value. Best for outlier rejection.
    /// Uses memory-mapped chunked processing for efficiency.
    Median(MedianConfig),
    /// Average after excluding pixels beyond N sigma from the mean.
    /// Uses memory-mapped chunked processing for efficiency.
    SigmaClippedMean(SigmaClippedConfig),
}

impl Default for StackingMethod {
    fn default() -> Self {
        Self::Median(MedianConfig::default())
    }
}

impl std::fmt::Display for StackingMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StackingMethod::Mean => write!(f, "mean"),
            StackingMethod::Median(_) => write!(f, "median"),
            StackingMethod::SigmaClippedMean(config) => {
                write!(f, "sigma{:.1}", config.clip.sigma)
            }
        }
    }
}

/// Configuration for sigma-clipped mean stacking.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SigmaClipConfig {
    /// Number of standard deviations for clipping threshold.
    pub sigma: f32,
    /// Maximum number of iterations for iterative clipping.
    pub max_iterations: u32,
}

impl Eq for SigmaClipConfig {}

impl Default for SigmaClipConfig {
    fn default() -> Self {
        Self {
            sigma: 2.5,
            max_iterations: 3,
        }
    }
}

impl SigmaClipConfig {
    pub fn new(sigma: f32, max_iterations: u32) -> Self {
        assert!(sigma > 0.0, "Sigma must be positive");
        assert!(max_iterations > 0, "Max iterations must be at least 1");
        Self {
            sigma,
            max_iterations,
        }
    }
}

/// Accumulator for incrementally stacking frames using running mean.
///
/// More memory efficient than storing all frames - only keeps running sum and count.
/// Only supports mean stacking (not median or sigma-clipped).
#[derive(Debug)]
pub struct ImageStack {
    paths: Vec<PathBuf>,
    frame_type: FrameType,
    method: StackingMethod,
}

impl ImageStack {
    /// Create a new stack for the given frame type with paths to load.
    pub fn new<I, P>(frame_type: FrameType, method: StackingMethod, paths: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        Self {
            paths: paths.into_iter().map(|p| p.into()).collect(),
            frame_type,
            method,
        }
    }

    /// Process all frames and return the stacked result.
    ///
    /// Uses memory-efficient streaming for mean and median stacking.
    /// For sigma-clipped, loads all frames into memory.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - No paths are provided
    /// - Image loading fails
    /// - Image dimensions don't match
    /// - Cache directory creation fails (for disk-backed storage)
    /// - Cache file I/O fails (for disk-backed storage)
    pub fn process(&self) -> Result<AstroImage, Error> {
        if self.paths.is_empty() {
            return Err(Error::NoPaths);
        }

        match &self.method {
            StackingMethod::Mean => mean::stack_mean_from_paths(&self.paths, self.frame_type),
            StackingMethod::Median(config) => {
                median::stack_median_from_paths(&self.paths, self.frame_type, config)
            }
            StackingMethod::SigmaClippedMean(config) => {
                sigma_clipped::stack_sigma_clipped_from_paths(&self.paths, self.frame_type, config)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::{calibration_image_paths, init_tracing};

    use super::*;

    // ========== Error Path Tests ==========

    #[test]
    fn test_image_stack_empty_paths_returns_no_paths_error() {
        let stack = ImageStack::new(FrameType::Dark, StackingMethod::Mean, Vec::<PathBuf>::new());
        let result = stack.process();

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::NoPaths));
    }

    #[test]
    fn test_image_stack_mean_nonexistent_file() {
        let paths = vec![PathBuf::from("/nonexistent/stack_image.fits")];
        let stack = ImageStack::new(FrameType::Dark, StackingMethod::Mean, paths);
        let result = stack.process();

        assert!(result.is_err());
        match result.unwrap_err() {
            Error::ImageLoad { path, .. } => {
                assert!(path.to_string_lossy().contains("nonexistent"));
            }
            e => panic!("Expected ImageLoad error, got {:?}", e),
        }
    }

    #[test]
    fn test_image_stack_median_nonexistent_file() {
        let paths = vec![PathBuf::from("/nonexistent/median_stack.fits")];
        let stack = ImageStack::new(FrameType::Flat, StackingMethod::default(), paths);
        let result = stack.process();

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::ImageLoad { .. }));
    }

    #[test]
    fn test_image_stack_sigma_clipped_nonexistent_file() {
        let paths = vec![PathBuf::from("/nonexistent/sigma_stack.fits")];
        let config = SigmaClippedConfig::default();
        let stack = ImageStack::new(
            FrameType::Bias,
            StackingMethod::SigmaClippedMean(config),
            paths,
        );
        let result = stack.process();

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::ImageLoad { .. }));
    }

    // ========== Configuration Tests ==========

    #[test]
    fn test_stacking_method_default() {
        assert_eq!(
            StackingMethod::default(),
            StackingMethod::Median(MedianConfig::default())
        );
    }

    #[test]
    fn test_stacking_method_display() {
        assert_eq!(StackingMethod::Mean.to_string(), "mean");
        assert_eq!(
            StackingMethod::Median(MedianConfig::default()).to_string(),
            "median"
        );

        let sigma_config = SigmaClippedConfig {
            clip: SigmaClipConfig::new(2.5, 3),
            ..Default::default()
        };
        assert_eq!(
            StackingMethod::SigmaClippedMean(sigma_config).to_string(),
            "sigma2.5"
        );
    }

    #[test]
    fn test_sigma_clip_config_new() {
        let config = SigmaClipConfig::new(3.0, 5);
        assert!((config.sigma - 3.0).abs() < f32::EPSILON);
        assert_eq!(config.max_iterations, 5);
    }

    #[test]
    #[should_panic(expected = "Sigma must be positive")]
    fn test_sigma_clip_config_zero_sigma_panics() {
        SigmaClipConfig::new(0.0, 3);
    }

    #[test]
    #[should_panic(expected = "Sigma must be positive")]
    fn test_sigma_clip_config_negative_sigma_panics() {
        SigmaClipConfig::new(-1.0, 3);
    }

    #[test]
    #[should_panic(expected = "Max iterations must be at least 1")]
    fn test_sigma_clip_config_zero_iterations_panics() {
        SigmaClipConfig::new(2.0, 0);
    }

    #[test]
    fn test_frame_type_display() {
        assert_eq!(FrameType::Dark.to_string(), "dark");
        assert_eq!(FrameType::Flat.to_string(), "flat");
        assert_eq!(FrameType::Bias.to_string(), "bias");
        assert_eq!(FrameType::Light.to_string(), "light");
    }

    #[test]
    fn test_image_stack_new() {
        let paths = vec![PathBuf::from("/a"), PathBuf::from("/b")];
        let stack = ImageStack::new(FrameType::Light, StackingMethod::Mean, paths);
        // Verify it was created (no panic)
        assert_eq!(format!("{:?}", stack.frame_type), "Light");
    }

    /// Helper to test stacking with a given frame type, method, and output filename.
    fn test_stack_from_env(
        subdir: &str,
        frame_type: FrameType,
        method: StackingMethod,
        output_name: &str,
    ) {
        init_tracing();

        let Some(paths) = calibration_image_paths(subdir) else {
            eprintln!(
                "LUMOS_CALIBRATION_DIR not set or {} dir missing, skipping test",
                subdir
            );
            return;
        };

        if paths.is_empty() {
            eprintln!("No files found in {} directory, skipping test", subdir);
            return;
        }

        println!(
            "Stacking {} {}s with {:?} method...",
            paths.len(),
            subdir.to_lowercase(),
            method
        );
        let stack = ImageStack::new(frame_type, method, paths.clone());
        let master = stack.process().unwrap();

        let first = AstroImage::from_file(&paths[0]).unwrap();
        println!(
            "Master {}: {}x{}x{}",
            subdir.to_lowercase(),
            master.dimensions.width,
            master.dimensions.height,
            master.dimensions.channels
        );

        assert_eq!(master.dimensions, first.dimensions);
        assert!(!master.pixels.is_empty());

        let img: imaginarium::Image = master.into();
        img.save_file(common::test_utils::test_output_path(output_name))
            .unwrap();
    }

    #[test]
    #[cfg_attr(not(feature = "slow-tests"), ignore)]
    fn test_stack_darks_mean_from_env() {
        test_stack_from_env(
            "Darks",
            FrameType::Dark,
            StackingMethod::Mean,
            "master_dark_mean.tiff",
        );
    }

    #[test]
    #[cfg_attr(not(feature = "slow-tests"), ignore)]
    fn test_stack_darks_median_from_env() {
        test_stack_from_env(
            "Darks",
            FrameType::Dark,
            StackingMethod::default(),
            "master_dark_median.tiff",
        );
    }

    #[test]
    #[cfg_attr(not(feature = "slow-tests"), ignore)]
    fn test_stack_darks_sigma_clipped_from_env() {
        test_stack_from_env(
            "Darks",
            FrameType::Dark,
            StackingMethod::SigmaClippedMean(SigmaClippedConfig::default()),
            "master_dark_sigma_clipped.tiff",
        );
    }
}
