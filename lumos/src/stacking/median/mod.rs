//! Memory-efficient median stacking using chunked processing and memory-mapped files.

#[cfg(feature = "bench")]
pub mod bench;

use std::path::Path;

use crate::astro_image::AstroImage;
use crate::math;
use crate::stacking::cache::ImageCache;
use crate::stacking::{CacheConfig, FrameType};

/// Configuration for memory-efficient median stacking.
pub type MedianStackConfig = CacheConfig;

/// Stack images using median with bounded memory usage.
pub fn stack_median_from_paths<P: AsRef<Path>>(
    paths: &[P],
    frame_type: FrameType,
    config: &MedianStackConfig,
) -> AstroImage {
    assert!(!paths.is_empty(), "No paths provided for stacking");

    std::fs::create_dir_all(&config.cache_dir).expect("Failed to create cache directory");

    let cache = ImageCache::from_paths(paths, &config.cache_dir, frame_type);
    let result = cache.process_chunked(config.chunk_rows, math::median_f32_mut);

    if !config.keep_cache {
        cache.cleanup();
    }

    result
}
