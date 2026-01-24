//! Median stacking with automatic memory management.
//!
//! Automatically chooses between in-memory and disk-based processing
//! based on available system memory.

#[cfg(feature = "bench")]
pub mod bench;

use std::path::Path;

use crate::astro_image::AstroImage;
use crate::math;
use crate::stacking::cache::ImageCache;
use crate::stacking::error::StackError;
use crate::stacking::{CacheConfig, FrameType};

/// Configuration for median stacking.
pub type MedianConfig = CacheConfig;

/// Stack images using median.
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
pub fn stack_median_from_paths<P: AsRef<Path>>(
    paths: &[P],
    frame_type: FrameType,
    config: &MedianConfig,
) -> Result<AstroImage, StackError> {
    let cache = ImageCache::from_paths(paths, config, frame_type)?;
    let result = cache.process_chunked(math::median_f32_mut);

    if !config.keep_cache {
        cache.cleanup();
    }

    Ok(result)
}
