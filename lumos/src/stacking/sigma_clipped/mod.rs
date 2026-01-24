//! Memory-efficient sigma-clipped mean stacking using chunked processing and memory-mapped files.

#[cfg(feature = "bench")]
pub mod bench;

use std::path::{Path, PathBuf};

use crate::astro_image::AstroImage;
use crate::math;
use crate::stacking::cache::ImageCache;
use crate::stacking::{FrameType, SigmaClipConfig};

/// Configuration for memory-efficient sigma-clipped mean stacking.
#[derive(Debug, Clone, PartialEq)]
pub struct SigmaClippedConfig {
    /// Sigma clipping parameters.
    pub clip: SigmaClipConfig,
    /// Number of rows to process at once (memory vs seeks tradeoff).
    pub chunk_rows: usize,
    /// Directory for decoded image cache.
    pub cache_dir: PathBuf,
    /// Keep cache after stacking (useful for re-processing).
    pub keep_cache: bool,
}

impl Default for SigmaClippedConfig {
    fn default() -> Self {
        Self {
            clip: SigmaClipConfig::default(),
            chunk_rows: 128,
            cache_dir: std::env::temp_dir().join("lumos_cache"),
            keep_cache: false,
        }
    }
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

/// Stack images using sigma-clipped mean with bounded memory usage.
pub fn stack_sigma_clipped_from_paths<P: AsRef<Path>>(
    paths: &[P],
    frame_type: FrameType,
    config: &SigmaClippedConfig,
) -> AstroImage {
    assert!(!paths.is_empty(), "No paths provided for stacking");

    std::fs::create_dir_all(&config.cache_dir).expect("Failed to create cache directory");

    let cache = ImageCache::from_paths(paths, &config.cache_dir, frame_type);
    let clip = config.clip;
    let result = cache.process_chunked(config.chunk_rows, |values: &mut [f32]| {
        sigma_clipped_mean(values, &clip)
    });

    if !config.keep_cache {
        cache.cleanup();
    }

    result
}

/// Calculate sigma-clipped mean.
///
/// Algorithm:
/// 1. Use median as center (robust to outliers)
/// 2. Compute std dev from median
/// 3. Clip values beyond sigma threshold from median
/// 4. Return mean of remaining values (statistically efficient)
fn sigma_clipped_mean(values: &[f32], config: &SigmaClipConfig) -> f32 {
    debug_assert!(!values.is_empty());

    if values.len() <= 2 {
        return math::mean_f32(values);
    }

    let mut included: Vec<f32> = values.to_vec();

    for _ in 0..config.max_iterations {
        if included.len() <= 2 {
            break;
        }

        // Use median as center - robust to outliers (in-place)
        let center = math::median_f32_mut(&mut included);
        let variance = math::sum_squared_diff(&included, center) / included.len() as f32;
        let std_dev = variance.sqrt();

        if std_dev < f32::EPSILON {
            break;
        }

        let threshold = config.sigma * std_dev;
        let prev_len = included.len();

        included.retain(|&v| (v - center).abs() <= threshold);

        // Stop if no values were clipped
        if included.len() == prev_len {
            break;
        }
    }

    // Return mean of remaining values (lower noise than median for Gaussian data)
    math::mean_f32(&included)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sigma_clipped_mean_removes_outlier() {
        let values = vec![1.0, 2.0, 2.0, 2.0, 2.0, 100.0];
        let config = SigmaClipConfig::new(2.0, 3);
        let result = sigma_clipped_mean(&values, &config);
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
        let result = sigma_clipped_mean(&values, &config);
        assert!(
            (result - 10.0).abs() < 1.0,
            "Expected ~10.0, got {}",
            result
        );
    }
}
