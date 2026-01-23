use crate::astro_image::AstroImage;
use crate::math;
use crate::stacking::{FrameType, SigmaClipConfig, StackingMethod};

/// Stack frames with the given method using parallel CPU processing.
pub(crate) fn stack_frames_cpu(
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

    // Process pixels in parallel
    let pixel_count = dims.pixel_count();
    let result_pixels = crate::common::parallel_map_f32(pixel_count, |pixel_idx| {
        let values: Vec<f32> = frames.iter().map(|f| f.pixels[pixel_idx]).collect();
        combine_pixels(&values, method)
    });

    AstroImage {
        metadata: first.metadata.clone(),
        pixels: result_pixels,
        dimensions: dims,
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

/// Calculate the mean of values using SIMD-accelerated sum.
fn mean(values: &[f32]) -> f32 {
    math::mean_f32(values)
}

/// Calculate the median of values.
fn median(values: &[f32]) -> f32 {
    math::median_f32(values)
}

/// Calculate sigma-clipped mean using SIMD-accelerated operations.
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
        let variance = math::sum_squared_diff(&included, avg) / included.len() as f32;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mean() {
        let values = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        assert!((mean(&values) - 3.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_mean_large() {
        // Test with enough values to exercise SIMD path (>4 elements)
        let values: Vec<f32> = (1..=100).map(|x| x as f32).collect();
        let expected = 50.5; // (1 + 100) / 2
        assert!((mean(&values) - expected).abs() < 1e-4);
    }

    #[test]
    fn test_mean_small() {
        // Test with <4 elements (scalar fallback)
        let values = vec![2.0, 4.0];
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
    fn test_sigma_clipped_mean_large() {
        // Test with enough values to exercise SIMD path
        let mut values: Vec<f32> = vec![10.0; 50];
        values.push(1000.0); // outlier
        let config = SigmaClipConfig::new(2.0, 3);
        let result = sigma_clipped_mean(&values, config);
        // Should exclude outlier and return ~10.0
        assert!(
            (result - 10.0).abs() < 1.0,
            "Expected ~10.0, got {}",
            result
        );
    }
}
