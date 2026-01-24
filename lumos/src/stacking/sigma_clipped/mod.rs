//! Memory-efficient sigma-clipped mean stacking using chunked processing and memory-mapped files.
//!
//! This implementation decodes images to a binary cache format, then processes
//! the images in horizontal strips to keep memory usage bounded.

use std::path::{Path, PathBuf};

use rayon::prelude::*;

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
    /// Default: 128 rows.
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
///
/// 1. Decodes all images to binary cache (f32 format)
/// 2. Processes in horizontal strips, loading strip from all cached files
/// 3. Computes sigma-clipped mean per pixel within each strip
/// 4. Cleans up cache unless keep_cache is set
pub fn stack_sigma_clipped_from_paths<P: AsRef<Path>>(
    paths: &[P],
    frame_type: FrameType,
    config: &SigmaClippedConfig,
) -> AstroImage {
    assert!(!paths.is_empty(), "No paths provided for stacking");

    // Create cache directory
    std::fs::create_dir_all(&config.cache_dir).expect("Failed to create cache directory");

    // Decode all images to cache
    let cache = ImageCache::from_paths(paths, &config.cache_dir, frame_type);

    // Process in chunks
    let result = process_chunked(&cache, config.chunk_rows, &config.clip);

    // Cleanup
    if !config.keep_cache {
        cache.cleanup();
    }

    result
}

/// Process cached images in horizontal chunks.
fn process_chunked(cache: &ImageCache, chunk_rows: usize, clip: &SigmaClipConfig) -> AstroImage {
    let dims = cache.dimensions();
    let frame_count = cache.frame_count();
    let width = dims.width;
    let height = dims.height;
    let channels = dims.channels;

    // Output buffer
    let mut output_pixels = vec![0.0f32; dims.pixel_count()];

    // Process by row chunks
    let num_chunks = height.div_ceil(chunk_rows);

    for chunk_idx in 0..num_chunks {
        let start_row = chunk_idx * chunk_rows;
        let end_row = (start_row + chunk_rows).min(height);
        let rows_in_chunk = end_row - start_row;
        let pixels_in_chunk = rows_in_chunk * width * channels;

        // Get slices from all frames (zero-copy from mmap)
        let chunks: Vec<&[f32]> = (0..frame_count)
            .map(|frame_idx| cache.read_chunk(frame_idx, start_row, end_row))
            .collect();

        // Compute sigma-clipped mean for each pixel in chunk (parallel over rows)
        // Each thread reuses its own buffers to avoid per-pixel allocation
        let output_slice = &mut output_pixels[start_row * width * channels..][..pixels_in_chunk];

        output_slice
            .par_chunks_mut(width * channels)
            .enumerate()
            .for_each(|(row_in_chunk, row_output)| {
                // One buffer per thread, reused for all pixels in row
                let mut values = vec![0.0f32; frame_count];
                let row_offset = row_in_chunk * width * channels;

                for (pixel_in_row, out) in row_output.iter_mut().enumerate() {
                    let pixel_idx = row_offset + pixel_in_row;
                    // Gather values from all frames
                    for (frame_idx, chunk) in chunks.iter().enumerate() {
                        values[frame_idx] = chunk[pixel_idx];
                    }
                    *out = sigma_clipped_mean(&values, clip);
                }
            });
    }

    AstroImage {
        metadata: cache.metadata().clone(),
        pixels: output_pixels,
        dimensions: dims,
    }
}

/// Calculate sigma-clipped mean using median for robust center estimation.
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

        // Use median as center - robust to outliers
        let center = math::median_f32(&included);
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
