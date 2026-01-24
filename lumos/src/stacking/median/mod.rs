//! Memory-efficient median stacking using chunked processing and memory-mapped files.

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
pub fn stack_median_from_paths<P: AsRef<Path>>(
    paths: &[P],
    frame_type: FrameType,
    config: &MedianStackConfig,
) -> AstroImage {
    assert!(!paths.is_empty(), "No paths provided for stacking");

    std::fs::create_dir_all(&config.cache_dir).expect("Failed to create cache directory");

    let cache = ImageCache::from_paths(paths, &config.cache_dir, frame_type);
    let result = cache.process_chunked(config.chunk_rows, cpu::median_f32);

    if !config.keep_cache {
        cache.cleanup();
    }

    result
}
