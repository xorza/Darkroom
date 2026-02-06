//! Unified stacking entry point.
//!
//! Provides `stack()` and `stack_with_progress()` functions as the main API
//! for image stacking operations.

// Allow dead_code until external callers adopt the new API

use std::path::Path;

use crate::AstroImage;

use super::cache::ImageCache;
use super::config::{CombineMethod, Normalization, Rejection, StackConfig};
use super::error::Error;
use super::progress::ProgressCallback;
use super::rejection::{
    self, AsymmetricSigmaClipConfig, GesdConfig, LinearFitClipConfig, PercentileClipConfig,
    RejectionResult, SigmaClipConfig as RejectionSigmaClipConfig, WinsorizedClipConfig,
};
use super::{CacheConfig, FrameType};
use crate::math;

/// Stack multiple images into a single result.
///
/// This is the main entry point for image stacking. It combines multiple frames
/// using the specified configuration.
///
/// # Arguments
///
/// * `paths` - Paths to input images
/// * `frame_type` - Type of frame being stacked (Light, Dark, Flat, Bias)
/// * `config` - Stacking configuration
///
/// # Returns
///
/// The stacked result as an `AstroImage`.
///
/// # Errors
///
/// Returns an error if:
/// - No paths are provided
/// - Image loading fails
/// - Image dimensions don't match
/// - Cache directory creation fails (for disk-backed storage)
///
/// # Examples
///
/// ```ignore
/// use lumos::stacking::{stack, StackConfig, FrameType};
///
/// // Default sigma-clipped mean
/// let result = stack(&paths, FrameType::Light, StackConfig::default())?;
///
/// // Median stacking
/// let result = stack(&paths, FrameType::Light, StackConfig::median())?;
///
/// // Custom configuration
/// let config = StackConfig {
///     rejection: Rejection::SigmaClipAsymmetric {
///         sigma_low: 2.0,
///         sigma_high: 3.0,
///         iterations: 5,
///     },
///     ..Default::default()
/// };
/// let result = stack(&paths, FrameType::Light, config)?;
/// ```
pub fn stack<P: AsRef<Path> + Sync>(
    paths: &[P],
    frame_type: FrameType,
    config: StackConfig,
) -> Result<AstroImage, Error> {
    stack_with_progress(paths, frame_type, config, ProgressCallback::default())
}

/// Stack multiple images with progress reporting.
///
/// Same as `stack()` but accepts a progress callback for monitoring.
///
/// # Arguments
///
/// * `paths` - Paths to input images
/// * `frame_type` - Type of frame being stacked
/// * `config` - Stacking configuration
/// * `progress` - Progress callback
///
/// # Examples
///
/// ```ignore
/// use lumos::stacking::{stack_with_progress, StackConfig, FrameType, ProgressCallback};
///
/// let progress = ProgressCallback::new(|stage, current, total| {
///     println!("{}: {}/{}", stage, current, total);
/// });
///
/// let result = stack_with_progress(&paths, FrameType::Light, StackConfig::default(), progress)?;
/// ```
pub fn stack_with_progress<P: AsRef<Path> + Sync>(
    paths: &[P],
    frame_type: FrameType,
    config: StackConfig,
    progress: ProgressCallback,
) -> Result<AstroImage, Error> {
    if paths.is_empty() {
        return Err(Error::NoPaths);
    }

    // Validate configuration
    config.validate();

    // Validate weights if provided
    if !config.weights.is_empty() && config.weights.len() != paths.len() {
        panic!(
            "Weight count ({}) must match frame count ({})",
            config.weights.len(),
            paths.len()
        );
    }

    // Log stacking parameters
    tracing::info!(
        frame_type = %frame_type,
        method = ?config.method,
        rejection = ?config.rejection,
        normalization = ?config.normalization,
        frame_count = paths.len(),
        has_weights = !config.weights.is_empty(),
        "Starting unified stack"
    );

    // Create image cache
    let cache = ImageCache::from_paths(paths, &config.cache, frame_type, progress)?;

    // Compute normalization parameters if requested
    let norm_params = match config.normalization {
        Normalization::None => None,
        Normalization::Global => Some(compute_global_norm_params(&cache)),
    };

    // Dispatch based on method, rejection, and normalization
    let result = match norm_params {
        Some(ref params) => dispatch_normalized(&cache, &config, paths.len(), params),
        None => dispatch_unnormalized(&cache, &config, paths.len()),
    };

    // Cleanup cache if not keeping
    if !config.cache.keep_cache {
        cache.cleanup();
    }

    Ok(result)
}

/// Compute per-frame normalization parameters for global normalization.
///
/// Uses frame 0 as the reference. For each frame/channel, computes `(gain, offset)`
/// such that `normalized = raw * gain + offset` matches the reference frame's
/// median and scale (MAD).
///
/// Formula: `gain = ref_scale / frame_scale`, `offset = ref_median - frame_median * gain`
fn compute_global_norm_params(cache: &ImageCache) -> Vec<(f32, f32)> {
    let stats = cache.compute_channel_stats();
    let channels = cache.dimensions().channels;
    let frame_count = cache.frame_count();
    let mut params = vec![(1.0f32, 0.0f32); frame_count * channels];

    for channel in 0..channels {
        let (ref_median, ref_mad) = stats[channel]; // frame 0

        for frame_idx in 0..frame_count {
            let (frame_median, frame_mad) = stats[frame_idx * channels + channel];

            let gain = if frame_mad > f32::EPSILON {
                ref_mad / frame_mad
            } else {
                1.0
            };
            let offset = ref_median - frame_median * gain;

            params[frame_idx * channels + channel] = (gain, offset);
        }
    }

    tracing::info!(
        frame_count,
        channels,
        "Computed global normalization (reference frame 0)"
    );

    params
}

/// Dispatch stacking without normalization (original path).
fn dispatch_unnormalized(
    cache: &ImageCache,
    config: &StackConfig,
    frame_count: usize,
) -> AstroImage {
    match (config.method, &config.rejection) {
        (CombineMethod::Mean, Rejection::None) => {
            cache.process_chunked(|values: &mut [f32]| math::mean_f32(values))
        }

        (CombineMethod::Median, Rejection::None) => cache.process_chunked(math::median_f32_mut),

        (CombineMethod::Median, rejection) => {
            let rejection = *rejection;
            cache.process_chunked(move |values: &mut [f32]| {
                apply_rejection(values, &rejection);
                math::median_f32_mut(values)
            })
        }

        (CombineMethod::Mean, rejection) => {
            let rejection = *rejection;
            cache.process_chunked(move |values: &mut [f32]| {
                let result = apply_rejection(values, &rejection);
                if result.remaining_count > 0 {
                    math::mean_f32(&values[..result.remaining_count])
                } else {
                    result.value
                }
            })
        }

        (CombineMethod::WeightedMean, rejection) => {
            let weights = config.normalized_weights(frame_count);
            let rejection = *rejection;
            cache.process_chunked_weighted(&weights, move |values: &mut [f32], w: &[f32]| {
                let result = apply_rejection_weighted(values, w, &rejection);
                result.value
            })
        }
    }
}

/// Dispatch stacking with per-frame normalization applied.
fn dispatch_normalized(
    cache: &ImageCache,
    config: &StackConfig,
    frame_count: usize,
    norm_params: &[(f32, f32)],
) -> AstroImage {
    match (config.method, &config.rejection) {
        (CombineMethod::Mean, Rejection::None) => cache
            .process_chunked_normalized(norm_params, |values: &mut [f32]| math::mean_f32(values)),

        (CombineMethod::Median, Rejection::None) => {
            cache.process_chunked_normalized(norm_params, math::median_f32_mut)
        }

        (CombineMethod::Median, rejection) => {
            let rejection = *rejection;
            cache.process_chunked_normalized(norm_params, move |values: &mut [f32]| {
                apply_rejection(values, &rejection);
                math::median_f32_mut(values)
            })
        }

        (CombineMethod::Mean, rejection) => {
            let rejection = *rejection;
            cache.process_chunked_normalized(norm_params, move |values: &mut [f32]| {
                let result = apply_rejection(values, &rejection);
                if result.remaining_count > 0 {
                    math::mean_f32(&values[..result.remaining_count])
                } else {
                    result.value
                }
            })
        }

        (CombineMethod::WeightedMean, rejection) => {
            let weights = config.normalized_weights(frame_count);
            let rejection = *rejection;
            cache.process_chunked_weighted_normalized(
                &weights,
                norm_params,
                move |values: &mut [f32], w: &[f32]| {
                    let result = apply_rejection_weighted(values, w, &rejection);
                    result.value
                },
            )
        }
    }
}

/// Apply rejection algorithm to values.
fn apply_rejection(values: &mut [f32], rejection: &Rejection) -> RejectionResult {
    match rejection {
        Rejection::None => RejectionResult {
            value: math::mean_f32(values),
            remaining_count: values.len(),
        },

        Rejection::SigmaClip { sigma, iterations } => {
            let config = RejectionSigmaClipConfig::new(*sigma, *iterations);
            rejection::sigma_clipped_mean(values, &config)
        }

        Rejection::SigmaClipAsymmetric {
            sigma_low,
            sigma_high,
            iterations,
        } => {
            let config = AsymmetricSigmaClipConfig::new(*sigma_low, *sigma_high, *iterations);
            rejection::sigma_clipped_mean_asymmetric(values, &config)
        }

        Rejection::Winsorized { sigma, iterations } => {
            let config = WinsorizedClipConfig::new(*sigma, *iterations);
            rejection::winsorized_sigma_clipped_mean(values, &config)
        }

        Rejection::LinearFit {
            sigma_low,
            sigma_high,
            iterations,
        } => {
            let config = LinearFitClipConfig::new(*sigma_low, *sigma_high, *iterations);
            rejection::linear_fit_clipped_mean(values, &config)
        }

        Rejection::Percentile { low, high } => {
            let config = PercentileClipConfig::new(*low, *high);
            rejection::percentile_clipped_mean(values, &config)
        }

        Rejection::Gesd {
            alpha,
            max_outliers,
        } => {
            let config = GesdConfig::new(*alpha, *max_outliers);
            rejection::gesd_mean(values, &config)
        }
    }
}

/// Apply rejection algorithm with weights.
fn apply_rejection_weighted(
    values: &mut [f32],
    weights: &[f32],
    rejection: &Rejection,
) -> RejectionResult {
    match rejection {
        Rejection::None => {
            let value = weighted_mean(values, weights);
            RejectionResult {
                value,
                remaining_count: values.len(),
            }
        }

        Rejection::SigmaClip { sigma, iterations } => {
            let config = RejectionSigmaClipConfig::new(*sigma, *iterations);
            let result = rejection::sigma_clipped_mean(values, &config);
            if result.remaining_count > 0 {
                let value = weighted_mean(
                    &values[..result.remaining_count],
                    &weights[..result.remaining_count],
                );
                RejectionResult { value, ..result }
            } else {
                result
            }
        }

        Rejection::SigmaClipAsymmetric {
            sigma_low,
            sigma_high,
            iterations,
        } => {
            let config = AsymmetricSigmaClipConfig::new(*sigma_low, *sigma_high, *iterations);
            let result = rejection::sigma_clipped_mean_asymmetric(values, &config);
            if result.remaining_count > 0 {
                let value = weighted_mean(
                    &values[..result.remaining_count],
                    &weights[..result.remaining_count],
                );
                RejectionResult { value, ..result }
            } else {
                result
            }
        }

        Rejection::Winsorized { sigma, iterations } => {
            // Winsorized replaces outliers with boundary values (all values kept).
            // Apply winsorization, then compute weighted mean of adjusted values.
            let config = WinsorizedClipConfig::new(*sigma, *iterations);
            let winsorized = rejection::winsorize(values, &config);
            let value = weighted_mean(&winsorized, weights);
            RejectionResult {
                value,
                remaining_count: values.len(),
            }
        }

        Rejection::LinearFit {
            sigma_low,
            sigma_high,
            iterations,
        } => {
            let config = LinearFitClipConfig::new(*sigma_low, *sigma_high, *iterations);
            let result = rejection::linear_fit_clipped_mean(values, &config);
            if result.remaining_count > 0 {
                let value = weighted_mean(
                    &values[..result.remaining_count],
                    &weights[..result.remaining_count],
                );
                RejectionResult { value, ..result }
            } else {
                result
            }
        }

        Rejection::Percentile { low, high } => {
            // Sort values and weights together so they stay aligned
            let mut pairs: Vec<(f32, f32)> = values
                .iter()
                .zip(weights.iter())
                .map(|(&v, &w)| (v, w))
                .collect();
            pairs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

            let n = pairs.len();
            let low_count = ((*low / 100.0) * n as f32).floor() as usize;
            let high_count = ((*high / 100.0) * n as f32).floor() as usize;
            let start = low_count;
            let end = n.saturating_sub(high_count);
            let (start, end) = if start >= end {
                let mid = n / 2;
                (mid, mid + 1)
            } else {
                (start, end)
            };

            let remaining = &pairs[start..end];
            let value = weighted_mean_pairs(remaining);
            RejectionResult {
                value,
                remaining_count: remaining.len(),
            }
        }

        Rejection::Gesd {
            alpha,
            max_outliers,
        } => {
            let config = GesdConfig::new(*alpha, *max_outliers);
            let result = rejection::gesd_mean(values, &config);
            if result.remaining_count > 0 {
                let value = weighted_mean(
                    &values[..result.remaining_count],
                    &weights[..result.remaining_count],
                );
                RejectionResult { value, ..result }
            } else {
                result
            }
        }
    }
}

/// Compute weighted mean from (value, weight) pairs.
fn weighted_mean_pairs(pairs: &[(f32, f32)]) -> f32 {
    if pairs.is_empty() {
        return 0.0;
    }
    let mut sum = 0.0f32;
    let mut weight_sum = 0.0f32;
    for &(v, w) in pairs {
        sum += v * w;
        weight_sum += w;
    }
    if weight_sum > f32::EPSILON {
        sum / weight_sum
    } else {
        pairs.iter().map(|(v, _)| v).sum::<f32>() / pairs.len() as f32
    }
}

/// Compute weighted mean.
fn weighted_mean(values: &[f32], weights: &[f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }

    let mut sum = 0.0f32;
    let mut weight_sum = 0.0f32;

    for (&v, &w) in values.iter().zip(weights.iter()) {
        sum += v * w;
        weight_sum += w;
    }

    if weight_sum > f32::EPSILON {
        sum / weight_sum
    } else {
        values.iter().sum::<f32>() / values.len() as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::astro_image::{AstroImage, ImageDimensions};
    use std::path::PathBuf;

    /// Helper: create an in-memory ImageCache from a list of AstroImages.
    fn cache_from_images(images: Vec<AstroImage>) -> super::super::cache::ImageCache {
        super::super::cache::make_test_cache(images)
    }

    #[test]
    fn test_stack_empty_paths() {
        let paths: Vec<PathBuf> = vec![];
        let result = stack(&paths, FrameType::Light, StackConfig::default());
        assert!(matches!(result.unwrap_err(), Error::NoPaths));
    }

    #[test]
    fn test_stack_nonexistent_file() {
        let paths = vec![PathBuf::from("/nonexistent/image.fits")];
        let result = stack(&paths, FrameType::Light, StackConfig::default());
        assert!(matches!(result.unwrap_err(), Error::ImageLoad { .. }));
    }

    #[test]
    #[should_panic(expected = "Weight count")]
    fn test_stack_weight_mismatch() {
        let paths = vec![
            PathBuf::from("/a.fits"),
            PathBuf::from("/b.fits"),
            PathBuf::from("/c.fits"),
        ];
        let config = StackConfig::weighted(vec![1.0, 2.0]); // Wrong count
        let _ = stack(&paths, FrameType::Light, config);
    }

    #[test]
    fn test_apply_rejection_none() {
        let mut values = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let result = apply_rejection(&mut values, &Rejection::None);
        assert!((result.value - 3.0).abs() < f32::EPSILON);
        assert_eq!(result.remaining_count, 5);
    }

    #[test]
    fn test_apply_rejection_sigma_clip() {
        let mut values = vec![1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 100.0];
        let result = apply_rejection(
            &mut values,
            &Rejection::SigmaClip {
                sigma: 2.0,
                iterations: 3,
            },
        );
        assert!(
            result.value < 10.0,
            "Outlier should be clipped, got {}",
            result.value
        );
        assert!(result.remaining_count < 8);
    }

    #[test]
    fn test_weighted_mean() {
        let values = vec![1.0, 10.0];
        let weights = vec![0.9, 0.1];
        let result = weighted_mean(&values, &weights);
        assert!((result - 1.9).abs() < f32::EPSILON);
    }

    #[test]
    fn test_weighted_percentile_uses_weights() {
        // Values: sorted would be [1, 2, 3, 4, 5, 6, 7, 8, 9, 10]
        // 20% clip from each end -> keep [3, 4, 5, 6, 7, 8]
        // Weights heavily favor value 8 (weight 10.0) over others (weight 1.0)
        let mut values = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
        let weights = vec![1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 10.0, 1.0, 1.0];

        let result = apply_rejection_weighted(
            &mut values,
            &weights,
            &Rejection::Percentile {
                low: 20.0,
                high: 20.0,
            },
        );

        // Unweighted mean of [3,4,5,6,7,8] = 5.5
        // Weighted mean should be pulled toward 8 (weight 10.0)
        assert_eq!(result.remaining_count, 6);
        assert!(
            result.value > 5.5 + 0.5,
            "Weighted percentile should be pulled toward heavily weighted value 8, got {}",
            result.value
        );
    }

    #[test]
    fn test_weighted_winsorized_uses_weights() {
        // Values with an outlier; weights heavily favor value 1.0
        let mut values = vec![1.0, 2.0, 2.0, 2.0, 2.0, 100.0];
        let weights = vec![10.0, 1.0, 1.0, 1.0, 1.0, 1.0];

        let result = apply_rejection_weighted(
            &mut values,
            &weights,
            &Rejection::Winsorized {
                sigma: 2.0,
                iterations: 3,
            },
        );

        // All values retained (winsorized, not removed)
        assert_eq!(result.remaining_count, 6);

        // Unweighted winsorized mean would be close to ~2.0 (outlier clamped)
        // Weighted should be pulled toward 1.0 due to weight 10.0
        let mut values_unwt = vec![1.0, 2.0, 2.0, 2.0, 2.0, 100.0];
        let uniform_weights = vec![1.0; 6];
        let unweighted_result = apply_rejection_weighted(
            &mut values_unwt,
            &uniform_weights,
            &Rejection::Winsorized {
                sigma: 2.0,
                iterations: 3,
            },
        );

        assert!(
            result.value < unweighted_result.value,
            "Weighted winsorized (heavy on 1.0) should be less than uniform: {} vs {}",
            result.value,
            unweighted_result.value,
        );
    }

    #[test]
    fn test_weighted_asymmetric_sigma_clip() {
        // Verify the asymmetric dispatch works in the weighted path
        // Use data with spread (non-zero MAD) plus a clear outlier
        let mut values = vec![1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 100.0];
        let weights = vec![10.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0];

        let result = apply_rejection_weighted(
            &mut values,
            &weights,
            &Rejection::SigmaClipAsymmetric {
                sigma_low: 4.0,
                sigma_high: 2.0,
                iterations: 3,
            },
        );

        // High outlier (100.0) should be rejected
        assert!(result.remaining_count < 8);
        // Weighted mean should be pulled toward 1.0
        assert!(
            result.value < 2.5,
            "Should be pulled toward 1.0, got {}",
            result.value
        );
    }

    // ========== Global Normalization Tests ==========

    #[test]
    fn test_compute_global_norm_params_identity_for_identical_frames() {
        // All frames have same pixel values -> normalization should be identity
        let dims = ImageDimensions::new(4, 4, 1);
        let images = vec![
            AstroImage::from_pixels(dims, vec![5.0; 16]),
            AstroImage::from_pixels(dims, vec![5.0; 16]),
            AstroImage::from_pixels(dims, vec![5.0; 16]),
        ];
        let cache = cache_from_images(images);
        let params = compute_global_norm_params(&cache);

        // 3 frames * 1 channel = 3 entries
        assert_eq!(params.len(), 3);
        for (gain, offset) in &params {
            assert!(
                (*gain - 1.0).abs() < 1e-5,
                "Gain should be ~1.0, got {}",
                gain
            );
            assert!(offset.abs() < 1e-5, "Offset should be ~0.0, got {}", offset);
        }
    }

    #[test]
    fn test_compute_global_norm_params_offset_correction() {
        // Frame 0: values around 100, Frame 1: values around 200 (same scale, different median)
        let dims = ImageDimensions::new(4, 4, 1);
        let frame0: Vec<f32> = (0..16).map(|i| 100.0 + i as f32).collect();
        let frame1: Vec<f32> = (0..16).map(|i| 200.0 + i as f32).collect();

        let images = vec![
            AstroImage::from_pixels(dims, frame0),
            AstroImage::from_pixels(dims, frame1),
        ];
        let cache = cache_from_images(images);
        let params = compute_global_norm_params(&cache);

        // Frame 0 is reference -> (gain=1, offset=0)
        let (g0, o0) = params[0];
        assert!((g0 - 1.0).abs() < 1e-5);
        assert!(o0.abs() < 1e-5);

        // Frame 1 should have gain ~1.0 (same scale), offset ~-100.0
        let (g1, o1) = params[1];
        assert!((g1 - 1.0).abs() < 0.1, "Gain should be ~1.0, got {}", g1);
        assert!(
            (o1 - (-100.0)).abs() < 1.0,
            "Offset should be ~-100.0, got {}",
            o1
        );
    }

    #[test]
    fn test_compute_global_norm_params_scale_correction() {
        // Frame 0: values in [90..110], Frame 1: values in [80..120] (wider spread)
        let dims = ImageDimensions::new(10, 10, 1);
        let frame0: Vec<f32> = (0..100).map(|i| 90.0 + (i as f32) * 20.0 / 99.0).collect();
        let frame1: Vec<f32> = (0..100).map(|i| 80.0 + (i as f32) * 40.0 / 99.0).collect();

        let images = vec![
            AstroImage::from_pixels(dims, frame0),
            AstroImage::from_pixels(dims, frame1),
        ];
        let cache = cache_from_images(images);
        let params = compute_global_norm_params(&cache);

        // Frame 1 has ~2x the spread of frame 0, so gain should be ~0.5
        let (g1, _o1) = params[1];
        assert!(
            (g1 - 0.5).abs() < 0.15,
            "Gain should be ~0.5 (narrowing wider frame), got {}",
            g1
        );
    }

    #[test]
    fn test_normalized_mean_stacking_corrects_offset() {
        // Two frames with different brightness but same structure.
        // Without normalization, mean would blend the two levels.
        // With normalization, both should be brought to the reference level.
        let dims = ImageDimensions::new(4, 4, 1);
        let base_value = 100.0;
        let offset = 50.0;
        let frame0: Vec<f32> = vec![base_value; 16];
        let frame1: Vec<f32> = vec![base_value + offset; 16];

        let images_norm = vec![
            AstroImage::from_pixels(dims, frame0.clone()),
            AstroImage::from_pixels(dims, frame1.clone()),
        ];
        let cache = cache_from_images(images_norm);
        let norm_params = compute_global_norm_params(&cache);

        // With normalization: all pixels should be close to base_value
        let result = cache
            .process_chunked_normalized(&norm_params, |values: &mut [f32]| math::mean_f32(values));
        for &pixel in result.channel(0) {
            assert!(
                (pixel - base_value).abs() < 1.0,
                "Normalized mean should be ~{}, got {}",
                base_value,
                pixel
            );
        }

        // Without normalization: mean would be base_value + offset/2 = 125
        let images_raw = vec![
            AstroImage::from_pixels(dims, frame0),
            AstroImage::from_pixels(dims, frame1),
        ];
        let cache_raw = cache_from_images(images_raw);
        let result_raw = cache_raw.process_chunked(|values: &mut [f32]| math::mean_f32(values));
        for &pixel in result_raw.channel(0) {
            assert!(
                (pixel - (base_value + offset / 2.0)).abs() < 1.0,
                "Unnormalized mean should be ~{}, got {}",
                base_value + offset / 2.0,
                pixel
            );
        }
    }

    #[test]
    fn test_normalized_stacking_rgb() {
        // RGB frames where each channel has a different offset in frame 1
        let dims = ImageDimensions::new(4, 4, 3);
        // Frame 0: R=100, G=200, B=300 for all pixels
        let pixels0: Vec<f32> = (0..16).flat_map(|_| vec![100.0, 200.0, 300.0]).collect();
        // Frame 1: R=120, G=180, B=350 for all pixels
        let pixels1: Vec<f32> = (0..16).flat_map(|_| vec![120.0, 180.0, 350.0]).collect();

        let images = vec![
            AstroImage::from_pixels(dims, pixels0),
            AstroImage::from_pixels(dims, pixels1),
        ];
        let cache = cache_from_images(images);
        let norm_params = compute_global_norm_params(&cache);

        let result = cache
            .process_chunked_normalized(&norm_params, |values: &mut [f32]| math::mean_f32(values));

        // After normalization to frame 0's reference, each channel should be
        // close to the reference frame's values
        for &pixel in result.channel(0) {
            assert!(
                (pixel - 100.0).abs() < 2.0,
                "R channel should be ~100, got {}",
                pixel
            );
        }
        for &pixel in result.channel(1) {
            assert!(
                (pixel - 200.0).abs() < 2.0,
                "G channel should be ~200, got {}",
                pixel
            );
        }
        for &pixel in result.channel(2) {
            assert!(
                (pixel - 300.0).abs() < 2.0,
                "B channel should be ~300, got {}",
                pixel
            );
        }
    }

    #[test]
    fn test_dispatch_normalized_vs_unnormalized() {
        // Verify dispatch_normalized actually applies normalization
        let dims = ImageDimensions::new(4, 4, 1);
        let frame0: Vec<f32> = vec![100.0; 16];
        let frame1: Vec<f32> = vec![200.0; 16];

        let images = vec![
            AstroImage::from_pixels(dims, frame0.clone()),
            AstroImage::from_pixels(dims, frame1.clone()),
        ];
        let cache = cache_from_images(images);
        let norm_params = compute_global_norm_params(&cache);

        let config = StackConfig {
            normalization: Normalization::Global,
            rejection: Rejection::None,
            ..Default::default()
        };

        let result_norm = dispatch_normalized(&cache, &config, 2, &norm_params);
        let result_unnorm = dispatch_unnormalized(&cache, &config, 2);

        // Normalized: should be ~100 (reference frame level)
        // Unnormalized: should be ~150 (average of 100 and 200)
        let norm_pixel = result_norm.channel(0)[0];
        let unnorm_pixel = result_unnorm.channel(0)[0];
        assert!(
            (norm_pixel - 100.0).abs() < 1.0,
            "Normalized should be ~100, got {}",
            norm_pixel
        );
        assert!(
            (unnorm_pixel - 150.0).abs() < 1.0,
            "Unnormalized should be ~150, got {}",
            unnorm_pixel
        );
    }
}
