mod cpu;
mod mean;
mod median;

use strum_macros::Display;

pub use median::MedianStackConfig;

#[cfg(feature = "bench")]
pub mod bench {
    pub use super::median::bench as median;
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
    Median(MedianStackConfig),
    /// Average after excluding pixels beyond N sigma from the mean.
    /// The f32 parameter specifies the sigma threshold (typically 2.0-3.0).
    SigmaClippedMean(SigmaClipConfig),
}

impl Default for StackingMethod {
    fn default() -> Self {
        Self::Median(MedianStackConfig::default())
    }
}

impl std::fmt::Display for StackingMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StackingMethod::Mean => write!(f, "mean"),
            StackingMethod::Median(_) => write!(f, "median"),
            StackingMethod::SigmaClippedMean(config) => {
                write!(f, "sigma{:.1}", config.sigma)
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

use std::path::PathBuf;

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
    /// # Panics
    /// Panics if no paths were provided or if frames have mismatched dimensions.
    pub fn process(&self) -> AstroImage {
        assert!(!self.paths.is_empty(), "No paths provided for stacking");

        match &self.method {
            StackingMethod::Mean => mean::stack_mean_from_paths(&self.paths, self.frame_type),
            StackingMethod::Median(config) => {
                median::stack_median_from_paths(&self.paths, self.frame_type, config)
            }
            StackingMethod::SigmaClippedMean(_) => {
                // Sigma-clipped needs all values for iterative clipping
                let frames: Vec<AstroImage> = self
                    .paths
                    .iter()
                    .map(|p| AstroImage::from_file(p).expect("Failed to load image"))
                    .collect();
                cpu::stack_frames_cpu(&frames, &self.method, self.frame_type)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::calibration_image_paths;

    use super::*;

    #[test]
    fn test_stacking_method_default() {
        assert_eq!(
            StackingMethod::default(),
            StackingMethod::Median(MedianStackConfig::default())
        );
    }

    #[test]
    #[cfg_attr(not(feature = "slow-tests"), ignore)]
    fn test_stack_flats_median_from_env() {
        let Some(paths) = calibration_image_paths("Flats") else {
            eprintln!("LUMOS_CALIBRATION_DIR not set or Flats dir missing, skipping test");
            return;
        };

        if paths.is_empty() {
            eprintln!("No flat files found in Flats directory, skipping test");
            return;
        }

        println!("Stacking {} flats with median method...", paths.len());
        let stack = ImageStack::new(FrameType::Flat, StackingMethod::default(), paths.clone());
        let master_flat = stack.process();

        // Load first frame to verify dimensions match
        let first = AstroImage::from_file(&paths[0]).unwrap();
        println!(
            "Master flat: {}x{}x{}",
            master_flat.dimensions.width,
            master_flat.dimensions.height,
            master_flat.dimensions.channels
        );

        assert_eq!(master_flat.dimensions, first.dimensions);
        assert!(!master_flat.pixels.is_empty());

        let img: imaginarium::Image = master_flat.into();
        img.save_file(common::test_utils::test_output_path("master_flat.tiff"))
            .unwrap();
    }

    #[test]
    #[cfg_attr(not(feature = "slow-tests"), ignore)]
    fn test_stack_darks_mean_from_env() {
        let Some(paths) = calibration_image_paths("Darks") else {
            eprintln!("LUMOS_CALIBRATION_DIR not set or Darks dir missing, skipping test");
            return;
        };

        if paths.is_empty() {
            eprintln!("No dark files found in Darks directory, skipping test");
            return;
        }

        println!("Stacking {} darks with mean method...", paths.len());
        let stack = ImageStack::new(FrameType::Dark, StackingMethod::Mean, paths.clone());
        let master_dark = stack.process();

        // Load first frame to verify dimensions match
        let first = AstroImage::from_file(&paths[0]).unwrap();
        println!(
            "Master dark: {}x{}x{}",
            master_dark.dimensions.width,
            master_dark.dimensions.height,
            master_dark.dimensions.channels
        );

        assert_eq!(master_dark.dimensions, first.dimensions);
        assert!(!master_dark.pixels.is_empty());

        let img: imaginarium::Image = master_dark.into();
        img.save_file(common::test_utils::test_output_path(
            "master_dark_mean.tiff",
        ))
        .unwrap();
    }

    #[test]
    #[cfg_attr(not(feature = "slow-tests"), ignore)]
    fn test_stack_darks_median_from_env() {
        let Some(paths) = calibration_image_paths("Darks") else {
            eprintln!("LUMOS_CALIBRATION_DIR not set or Darks dir missing, skipping test");
            return;
        };

        if paths.is_empty() {
            eprintln!("No dark files found in Darks directory, skipping test");
            return;
        }

        println!("Stacking {} darks with median method...", paths.len());
        let stack = ImageStack::new(FrameType::Dark, StackingMethod::default(), paths.clone());
        let master_dark = stack.process();

        // Load first frame to verify dimensions match
        let first = AstroImage::from_file(&paths[0]).unwrap();
        println!(
            "Master dark: {}x{}x{}",
            master_dark.dimensions.width,
            master_dark.dimensions.height,
            master_dark.dimensions.channels
        );

        assert_eq!(master_dark.dimensions, first.dimensions);
        assert!(!master_dark.pixels.is_empty());

        let img: imaginarium::Image = master_dark.into();
        img.save_file(common::test_utils::test_output_path(
            "master_dark_median.tiff",
        ))
        .unwrap();
    }
}
