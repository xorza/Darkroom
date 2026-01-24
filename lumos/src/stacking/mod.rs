mod cpu;
use strum_macros::Display;

use crate::AstroImage;
use crate::astro_image::ImageDimensions;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StackingMethod {
    /// Average all pixel values. Fast but sensitive to outliers.
    Mean,
    /// Take the median pixel value. Best for outlier rejection.
    #[default]
    Median,
    /// Average after excluding pixels beyond N sigma from the mean.
    /// The f32 parameter specifies the sigma threshold (typically 2.0-3.0).
    SigmaClippedMean(SigmaClipConfig),
}

impl std::fmt::Display for StackingMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StackingMethod::Mean => write!(f, "mean"),
            StackingMethod::Median => write!(f, "median"),
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
    dimensions: Option<ImageDimensions>,
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
            dimensions: None,
            frame_type,
            method,
        }
    }
}

/// Stack frames with the given method (uses parallel CPU implementation).
pub fn stack_frames(
    frames: &[AstroImage],
    method: StackingMethod,
    frame_type: FrameType,
) -> AstroImage {
    cpu::stack_frames_cpu(frames, method, frame_type)
}

#[cfg(test)]
mod tests {
    use crate::testing::load_calibration_images;

    use super::*;

    #[test]
    fn test_stacking_method_default() {
        assert_eq!(StackingMethod::default(), StackingMethod::Median);
    }

    #[test]
    #[cfg_attr(not(feature = "slow-tests"), ignore)]
    fn test_stack_flats_from_env() {
        let Some(flats) = load_calibration_images("Flats") else {
            eprintln!("LUMOS_CALIBRATION_DIR not set or Flats dir missing, skipping test");
            return;
        };

        if flats.is_empty() {
            eprintln!("No flat files found in Flats directory, skipping test");
            return;
        }

        println!("Stacking {} flats with median method...", flats.len());
        let master_flat = stack_frames(&flats, StackingMethod::Median, FrameType::Flat);

        println!(
            "Master flat: {}x{}x{}",
            master_flat.dimensions.width,
            master_flat.dimensions.height,
            master_flat.dimensions.channels
        );

        assert_eq!(master_flat.dimensions, flats[0].dimensions);
        assert!(!master_flat.pixels.is_empty());

        let img: imaginarium::Image = master_flat.into();
        img.save_file(common::test_utils::test_output_path("master_flat.tiff"))
            .unwrap();
    }
}
