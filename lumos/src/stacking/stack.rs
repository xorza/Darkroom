//! Unified stacking entry point.
//!
//! Provides `stack()` and `stack_with_progress()` functions as the main API
//! for image stacking operations.

// Allow dead_code until external callers adopt the new API

use std::path::Path;

use crate::AstroImage;

use super::cache::ImageCache;
use super::config::{CombineMethod, Rejection, StackConfig};
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

    // Dispatch based on method and rejection
    let result = match (config.method, &config.rejection) {
        // Mean without rejection - simple average
        (CombineMethod::Mean, Rejection::None) => {
            cache.process_chunked(|values: &mut [f32]| math::mean_f32(values))
        }

        // Median - uses its own implicit rejection
        (CombineMethod::Median, Rejection::None) => cache.process_chunked(math::median_f32_mut),

        // Median with explicit rejection - apply rejection first
        (CombineMethod::Median, rejection) => {
            let rejection = *rejection;
            cache.process_chunked(move |values: &mut [f32]| {
                apply_rejection(values, &rejection);
                math::median_f32_mut(values)
            })
        }

        // Mean with rejection
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

        // Weighted mean
        (CombineMethod::WeightedMean, rejection) => {
            let weights = config.normalized_weights(paths.len());
            let rejection = *rejection;
            cache.process_chunked_weighted(&weights, move |values: &mut [f32], w: &[f32]| {
                let result = apply_rejection_weighted(values, w, &rejection);
                result.value
            })
        }
    };

    // Cleanup cache if not keeping
    if !config.cache.keep_cache {
        cache.cleanup();
    }

    Ok(result)
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
    use std::path::PathBuf;

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
}
