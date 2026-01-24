//! Memory-efficient sigma-clipped mean stacking using chunked processing and memory-mapped files.
//!
//! This implementation decodes images to a binary cache format, then processes
//! the images in horizontal strips to keep memory usage bounded.

mod scalar;

#[cfg(feature = "bench")]
pub mod bench;

use std::path::{Path, PathBuf};

use crate::astro_image::AstroImage;
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

    // Process in chunks using shared infrastructure
    let clip = config.clip;
    let result = cache.process_chunked(config.chunk_rows, |values| {
        scalar::sigma_clipped_mean(values, &clip)
    });

    // Cleanup
    if !config.keep_cache {
        cache.cleanup();
    }

    result
}
