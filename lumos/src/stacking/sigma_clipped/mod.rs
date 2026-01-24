//! Sigma-clipped mean stacking with automatic memory management.
//!
//! Automatically chooses between in-memory and disk-based processing
//! based on available system memory.

#[cfg(feature = "bench")]
pub mod bench;

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::astro_image::AstroImage;
use crate::math;
use crate::stacking::cache::ImageCache;
use crate::stacking::error::Error;
use crate::stacking::{CacheConfig, FrameType, SigmaClipConfig};

/// Configuration for sigma-clipped mean stacking.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SigmaClippedConfig {
    /// Sigma clipping parameters.
    pub clip: SigmaClipConfig,
    /// Cache configuration.
    pub cache: CacheConfig,
}

impl SigmaClippedConfig {
    /// Create a new config with the given sigma threshold.
    pub fn with_sigma(sigma: f32) -> Self {
        Self {
            clip: SigmaClipConfig::new(sigma, 3),
            ..Default::default()
        }
    }
}

/// Statistics for sigma clipping operations.
#[derive(Debug, Default)]
struct ClipStats {
    /// Total number of input values processed.
    total_values: AtomicU64,
    /// Total number of values clipped (removed as outliers).
    clipped_values: AtomicU64,
    /// Number of pixels where clipping occurred.
    pixels_with_clipping: AtomicU64,
    /// Number of pixels where excessive clipping occurred (>50% of values).
    pixels_excessive_clipping: AtomicU64,
}

impl ClipStats {
    fn record(&self, original_len: usize, final_len: usize) {
        let clipped = original_len - final_len;
        self.total_values
            .fetch_add(original_len as u64, Ordering::Relaxed);
        self.clipped_values
            .fetch_add(clipped as u64, Ordering::Relaxed);
        if clipped > 0 {
            self.pixels_with_clipping.fetch_add(1, Ordering::Relaxed);
        }
        if clipped > original_len / 2 {
            self.pixels_excessive_clipping
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    fn log_summary(&self, frame_count: usize) {
        let total = self.total_values.load(Ordering::Relaxed);
        let clipped = self.clipped_values.load(Ordering::Relaxed);
        let pixels_clipped = self.pixels_with_clipping.load(Ordering::Relaxed);
        let excessive = self.pixels_excessive_clipping.load(Ordering::Relaxed);

        if total == 0 {
            return;
        }

        let pixel_count = total / frame_count as u64;
        let clip_percent = 100.0 * clipped as f64 / total as f64;
        let pixels_clipped_percent = 100.0 * pixels_clipped as f64 / pixel_count as f64;
        let excessive_percent = 100.0 * excessive as f64 / pixel_count as f64;

        tracing::info!(
            "Sigma clipping stats: {:.2}% of values clipped ({} of {})",
            clip_percent,
            clipped,
            total
        );
        tracing::info!(
            "  Pixels with any clipping: {:.2}% ({} of {})",
            pixels_clipped_percent,
            pixels_clipped,
            pixel_count
        );

        if excessive > 0 {
            tracing::warn!(
                "  Pixels with excessive clipping (>50%): {:.2}% ({} pixels) - consider adjusting sigma threshold",
                excessive_percent,
                excessive
            );
        }

        // Warn if overall clipping seems too aggressive or too lenient
        if clip_percent > 20.0 {
            tracing::warn!(
                "High clipping rate ({:.1}%) - sigma threshold may be too aggressive",
                clip_percent
            );
        } else if clip_percent < 0.1 && frame_count > 10 {
            tracing::debug!(
                "Very low clipping rate ({:.2}%) - data may be very clean or threshold too lenient",
                clip_percent
            );
        }
    }
}

/// Stack images using sigma-clipped mean.
///
/// Automatically chooses storage mode based on available memory:
/// - If all images fit in 75% of available RAM, processes entirely in memory
/// - Otherwise, uses disk cache with memory-mapped access
///
/// # Errors
///
/// Returns an error if:
/// - No paths are provided
/// - Image loading fails
/// - Image dimensions don't match
/// - Cache directory creation fails
/// - Cache file I/O fails
pub fn stack_sigma_clipped_from_paths<P: AsRef<Path> + Sync>(
    paths: &[P],
    frame_type: FrameType,
    config: &SigmaClippedConfig,
) -> Result<AstroImage, Error> {
    let cache = ImageCache::from_paths(paths, &config.cache, frame_type)?;
    let clip = config.clip;
    let stats = ClipStats::default();

    let result = cache.process_chunked(|values: &mut [f32]| {
        let original_len = values.len();
        let (result, final_len) = sigma_clipped_mean_with_stats(values, &clip);
        stats.record(original_len, final_len);
        result
    });

    stats.log_summary(paths.len());

    if !config.cache.keep_cache {
        cache.cleanup();
    }

    Ok(result)
}

/// Calculate sigma-clipped mean in-place, returning both the result and final value count.
///
/// Algorithm:
/// 1. Use median as center (robust to outliers)
/// 2. Compute std dev from median
/// 3. Clip values beyond sigma threshold from median
/// 4. Return mean of remaining values (statistically efficient)
///
/// Note: This function modifies the input slice. The caller provides a mutable
/// buffer that can be reused across calls (one per thread in parallel processing).
///
/// Returns: (mean, final_count) where final_count is the number of values after clipping.
fn sigma_clipped_mean_with_stats(values: &mut [f32], config: &SigmaClipConfig) -> (f32, usize) {
    debug_assert!(!values.is_empty());

    if values.len() <= 2 {
        return (math::mean_f32(values), values.len());
    }

    let mut len = values.len();

    for _ in 0..config.max_iterations {
        if len <= 2 {
            break;
        }

        let active = &mut values[..len];

        // Use median as center - robust to outliers (in-place)
        let center = math::median_f32_mut(active);
        let variance = math::sum_squared_diff(active, center) / len as f32;
        let std_dev = variance.sqrt();

        if std_dev < f32::EPSILON {
            break;
        }

        let threshold = config.sigma * std_dev;

        // Partition: move kept values to front, track new length
        let mut write_idx = 0;
        for read_idx in 0..len {
            if (values[read_idx] - center).abs() <= threshold {
                values[write_idx] = values[read_idx];
                write_idx += 1;
            }
        }

        // Stop if no values were clipped
        if write_idx == len {
            break;
        }
        len = write_idx;
    }

    // Return mean of remaining values (lower noise than median for Gaussian data)
    (math::mean_f32(&values[..len]), len)
}

/// Calculate sigma-clipped mean in-place (for benchmarks).
#[cfg(feature = "bench")]
pub fn sigma_clipped_mean(values: &mut [f32], config: &SigmaClipConfig) -> f32 {
    sigma_clipped_mean_with_stats(values, config).0
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// Calculate sigma-clipped mean in-place (test helper).
    fn sigma_clipped_mean(values: &mut [f32], config: &SigmaClipConfig) -> f32 {
        sigma_clipped_mean_with_stats(values, config).0
    }

    // ========== Error Path Tests ==========

    #[test]
    fn test_empty_paths_returns_no_paths_error() {
        let paths: Vec<PathBuf> = vec![];
        let config = SigmaClippedConfig::default();
        let result = stack_sigma_clipped_from_paths(&paths, FrameType::Light, &config);

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::NoPaths));
    }

    #[test]
    fn test_nonexistent_file_returns_image_load_error() {
        let paths = vec![PathBuf::from("/nonexistent/sigma_image.fits")];
        let config = SigmaClippedConfig::default();
        let result = stack_sigma_clipped_from_paths(&paths, FrameType::Light, &config);

        assert!(result.is_err());
        match result.unwrap_err() {
            Error::ImageLoad { path, .. } => {
                assert!(path.to_string_lossy().contains("nonexistent"));
            }
            e => panic!("Expected ImageLoad error, got {:?}", e),
        }
    }

    // ========== Algorithm Tests ==========

    #[test]
    fn test_sigma_clipped_mean_removes_outlier() {
        let mut values = vec![1.0, 2.0, 2.0, 2.0, 2.0, 100.0];
        let config = SigmaClipConfig::new(2.0, 3);
        let result = sigma_clipped_mean(&mut values, &config);
        assert!(
            result < 10.0,
            "Expected outlier to be clipped, got {}",
            result
        );
    }

    #[test]
    fn test_sigma_clipped_mean_large() {
        let mut values: Vec<f32> = vec![10.0; 50];
        values.push(1000.0);
        let config = SigmaClipConfig::new(2.0, 3);
        let result = sigma_clipped_mean(&mut values, &config);
        assert!(
            (result - 10.0).abs() < 1.0,
            "Expected ~10.0, got {}",
            result
        );
    }

    #[test]
    fn test_sigma_clipped_mean_single_value() {
        let mut values = vec![5.0];
        let config = SigmaClipConfig::new(2.0, 3);
        let result = sigma_clipped_mean(&mut values, &config);
        assert!((result - 5.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_sigma_clipped_mean_two_values() {
        let mut values = vec![3.0, 7.0];
        let config = SigmaClipConfig::new(2.0, 3);
        let result = sigma_clipped_mean(&mut values, &config);
        // With only 2 values, should return mean
        assert!((result - 5.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_sigma_clipped_mean_identical_values() {
        let mut values = vec![5.0; 10];
        let config = SigmaClipConfig::new(2.0, 3);
        let result = sigma_clipped_mean(&mut values, &config);
        // All identical, std_dev = 0, should return mean
        assert!((result - 5.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_sigma_clipped_mean_with_stats_tracks_clipping() {
        let mut values = vec![1.0, 2.0, 2.0, 2.0, 2.0, 100.0];
        let config = SigmaClipConfig::new(2.0, 3);
        let (result, final_len) = sigma_clipped_mean_with_stats(&mut values, &config);

        // Should have clipped the outlier
        assert!(final_len < 6);
        assert!(result < 10.0);
    }

    #[test]
    fn test_sigma_clipped_mean_no_clipping_needed() {
        let mut values = vec![1.0, 1.1, 1.2, 1.0, 0.9];
        let config = SigmaClipConfig::new(3.0, 3); // High sigma, shouldn't clip
        let (_, final_len) = sigma_clipped_mean_with_stats(&mut values, &config);

        // No values should be clipped
        assert_eq!(final_len, 5);
    }

    #[test]
    fn test_sigma_clipped_config_with_sigma() {
        let config = SigmaClippedConfig::with_sigma(1.5);
        assert!((config.clip.sigma - 1.5).abs() < f32::EPSILON);
        assert_eq!(config.clip.max_iterations, 3);
    }
}
