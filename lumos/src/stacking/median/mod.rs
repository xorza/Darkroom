//! Memory-efficient median stacking using chunked processing and memory-mapped files.
//!
//! This implementation decodes images to a binary cache format, then processes
//! the images in horizontal strips to keep memory usage bounded.

mod cpu;
mod scalar;

#[cfg(target_arch = "aarch64")]
mod neon;

#[cfg(target_arch = "x86_64")]
mod sse;

#[cfg(feature = "bench")]
pub mod bench;

use std::path::{Path, PathBuf};

use crate::astro_image::AstroImage;
use crate::stacking::FrameType;
use crate::stacking::cache::ImageCache;

/// Configuration for memory-efficient median stacking.
#[derive(Debug, Clone, PartialEq)]
pub struct MedianStackConfig {
    /// Number of rows to process at once (memory vs seeks tradeoff).
    /// Default: 128 rows.
    pub chunk_rows: usize,
    /// Directory for decoded image cache.
    pub cache_dir: PathBuf,
    /// Keep cache after stacking (useful for re-processing).
    pub keep_cache: bool,
}

impl Default for MedianStackConfig {
    fn default() -> Self {
        Self {
            chunk_rows: 128,
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
    let result = cpu::process_chunked(&cache, config.chunk_rows);

    // Cleanup
    if !config.keep_cache {
        cache.cleanup();
    }

    result
}
