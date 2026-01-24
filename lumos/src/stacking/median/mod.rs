//! Memory-efficient median stacking using chunked processing and memory-mapped files.
//!
//! This implementation decodes images to a binary cache format, then processes
//! the images in horizontal strips to keep memory usage bounded.

mod cache;

use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::astro_image::AstroImage;
use crate::math;
use crate::stacking::FrameType;

use cache::ImageCache;

/// Configuration for memory-efficient median stacking.
#[derive(Debug, Clone)]
pub struct MedianStackConfig {
    /// Number of rows to process at once (memory vs seeks tradeoff).
    /// Default: 64 rows.
    pub chunk_rows: usize,
    /// Directory for decoded image cache.
    pub cache_dir: PathBuf,
    /// Keep cache after stacking (useful for re-processing).
    pub keep_cache: bool,
}

impl Default for MedianStackConfig {
    fn default() -> Self {
        Self {
            chunk_rows: 64,
            cache_dir: std::env::temp_dir().join("lumos_cache"),
            keep_cache: false,
        }
    }
}

/// Stack images using median with bounded memory usage.
///
/// 1. Decodes all images to binary cache (f32 format)
/// 2. Processes in horizontal strips, loading strip from all cached files
/// 3. Computes median per pixel within each strip
/// 4. Cleans up cache unless keep_cache is set
pub fn stack_median_from_paths<P: AsRef<Path>>(
    paths: &[P],
    frame_type: FrameType,
    config: &MedianStackConfig,
) -> AstroImage {
    assert!(!paths.is_empty(), "No paths provided for stacking");

    // Create cache directory
    std::fs::create_dir_all(&config.cache_dir).expect("Failed to create cache directory");

    // Decode all images to cache
    let cache = ImageCache::from_paths(paths, &config.cache_dir, frame_type);

    // Process in chunks
    let result = process_chunked(&cache, config.chunk_rows);

    // Cleanup
    if !config.keep_cache {
        cache.cleanup();
    }

    result
}

/// Process cached images in horizontal chunks.
fn process_chunked(cache: &ImageCache, chunk_rows: usize) -> AstroImage {
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

        // Load chunk from all frames
        let chunks: Vec<Vec<f32>> = (0..frame_count)
            .map(|frame_idx| cache.read_chunk(frame_idx, start_row, end_row))
            .collect();

        // Compute median for each pixel in chunk (parallel over pixels)
        let chunk_output: Vec<f32> = (0..pixels_in_chunk)
            .into_par_iter()
            .map(|pixel_idx| {
                let values: Vec<f32> = chunks.iter().map(|c| c[pixel_idx]).collect();
                math::median_f32(&values)
            })
            .collect();

        // Copy to output
        let output_start = start_row * width * channels;
        output_pixels[output_start..output_start + pixels_in_chunk].copy_from_slice(&chunk_output);
    }

    AstroImage {
        metadata: cache.metadata().clone(),
        pixels: output_pixels,
        dimensions: dims,
    }
}
