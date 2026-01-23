use strum_macros::Display;

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

/// Combine pixel values using the specified method.
fn combine_pixels(values: &[f32], method: StackingMethod) -> f32 {
    match method {
        StackingMethod::Mean => mean(values),
        StackingMethod::Median => median(values),
        StackingMethod::SigmaClippedMean(config) => sigma_clipped_mean(values, config),
    }
}

/// Calculate the mean of values.
fn mean(values: &[f32]) -> f32 {
    debug_assert!(!values.is_empty());
    values.iter().sum::<f32>() / values.len() as f32
}

/// Calculate the median of values.
fn median(values: &[f32]) -> f32 {
    debug_assert!(!values.is_empty());

    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let len = sorted.len();
    if len.is_multiple_of(2) {
        (sorted[len / 2 - 1] + sorted[len / 2]) / 2.0
    } else {
        sorted[len / 2]
    }
}

/// Calculate sigma-clipped mean.
fn sigma_clipped_mean(values: &[f32], config: SigmaClipConfig) -> f32 {
    debug_assert!(!values.is_empty());

    if values.len() <= 2 {
        return mean(values);
    }

    let mut included: Vec<f32> = values.to_vec();

    for _ in 0..config.max_iterations {
        if included.len() <= 2 {
            break;
        }

        let avg = mean(&included);
        let variance =
            included.iter().map(|v| (v - avg).powi(2)).sum::<f32>() / included.len() as f32;
        let std_dev = variance.sqrt();

        if std_dev < f32::EPSILON {
            break;
        }

        let threshold = config.sigma * std_dev;
        let prev_len = included.len();

        included.retain(|&v| (v - avg).abs() <= threshold);

        // Stop if no values were clipped
        if included.len() == prev_len {
            break;
        }
    }

    mean(&included)
}

/// Stack frames with the given method, verifying dimensions match.
pub fn stack_frames(
    frames: &[AstroImage],
    method: StackingMethod,
    frame_type: FrameType,
) -> AstroImage {
    assert!(
        !frames.is_empty(),
        "Must provide at least one {} frame",
        frame_type
    );

    let first = &frames[0];
    let dims = first.dimensions;
    let pixel_count = first.pixel_count();

    // Verify all images have same dimensions
    for (i, frame) in frames.iter().enumerate().skip(1) {
        assert!(
            frame.dimensions == dims,
            "{} frame {} has different dimensions: {:?} vs {:?}",
            frame_type,
            i,
            frame.dimensions,
            dims
        );
    }

    let mut result_pixels = vec![0.0f32; pixel_count];

    // Collect values for each pixel position and combine
    let mut values = Vec::with_capacity(frames.len());

    for (pixel_idx, result) in result_pixels.iter_mut().enumerate() {
        values.clear();
        for frame in frames {
            values.push(frame.pixels[pixel_idx]);
        }

        *result = combine_pixels(&values, method);
    }

    AstroImage {
        metadata: first.metadata.clone(),
        pixels: result_pixels,
        dimensions: dims,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mean() {
        let values = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        assert!((mean(&values) - 3.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_median_odd() {
        let values = vec![1.0, 3.0, 2.0, 5.0, 4.0];
        assert!((median(&values) - 3.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_median_even() {
        let values = vec![1.0, 2.0, 3.0, 4.0];
        assert!((median(&values) - 2.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_sigma_clipped_mean_removes_outlier() {
        let values = vec![1.0, 2.0, 2.0, 2.0, 2.0, 100.0]; // 100 is outlier
        let config = SigmaClipConfig::new(2.0, 3);
        let result = sigma_clipped_mean(&values, config);
        // Should exclude 100 and average the rest
        assert!(
            result < 10.0,
            "Expected outlier to be clipped, got {}",
            result
        );
    }

    #[test]
    fn test_stacking_method_default() {
        assert_eq!(StackingMethod::default(), StackingMethod::Median);
    }

    #[test]
    #[cfg_attr(not(feature = "slow-tests"), ignore)]
    fn test_stack_flats_from_env() {
        use crate::test_utils::load_calibration_images;

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
