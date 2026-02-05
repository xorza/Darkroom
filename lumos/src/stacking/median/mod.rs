//! Median stacking with automatic memory management.
//!
//! Automatically chooses between in-memory and disk-based processing
//! based on available system memory.


use std::path::Path;

use crate::astro_image::AstroImage;
use crate::math;
use crate::stacking::cache::ImageCache;
use crate::stacking::error::Error;
use crate::stacking::progress::ProgressCallback;
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
pub fn stack_median_from_paths<P: AsRef<Path> + Sync>(
    paths: &[P],
    frame_type: FrameType,
    config: &MedianConfig,
    progress: ProgressCallback,
) -> Result<AstroImage, Error> {
    let cache = ImageCache::from_paths(paths, config, frame_type, progress)?;
    let result = cache.process_chunked(math::median_f32_mut);

    Ok(result)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn test_empty_paths_returns_no_paths_error() {
        let paths: Vec<PathBuf> = vec![];
        let config = MedianConfig::default();
        let result = stack_median_from_paths(
            &paths,
            FrameType::Dark,
            &config,
            ProgressCallback::default(),
        );

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::NoPaths));
    }

    #[test]
    fn test_nonexistent_file_returns_image_load_error() {
        let paths = vec![PathBuf::from("/nonexistent/median_image.fits")];
        let config = MedianConfig::default();
        let result = stack_median_from_paths(
            &paths,
            FrameType::Bias,
            &config,
            ProgressCallback::default(),
        );

        assert!(result.is_err());
        match result.unwrap_err() {
            Error::ImageLoad { path, .. } => {
                assert!(path.to_string_lossy().contains("nonexistent"));
            }
            e => panic!("Expected ImageLoad error, got {:?}", e),
        }
    }
}
