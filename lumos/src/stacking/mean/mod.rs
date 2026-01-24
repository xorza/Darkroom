//! Mean stacking implementation with SIMD optimizations.

mod cpu;

#[cfg(target_arch = "aarch64")]
mod neon;

#[cfg(target_arch = "x86_64")]
mod sse;

use std::path::Path;

use crate::AstroImage;
use crate::astro_image::ImageDimensions;
use crate::stacking::FrameType;

/// Chunk size for parallel accumulation (16KB of f32s for good cache locality).
const ACCUMULATE_CHUNK_SIZE: usize = 4096;

/// Stack frames from paths using running mean (memory efficient).
///
/// Loads images one at a time and accumulates sum, then divides by count.
/// Uses parallel processing for accumulation and SIMD for vector operations.
pub fn stack_mean_from_paths<P: AsRef<Path>>(paths: &[P], frame_type: FrameType) -> AstroImage {
    assert!(!paths.is_empty(), "No paths provided for stacking");

    let mut sum: Vec<f32> = Vec::new();
    let mut dimensions: Option<ImageDimensions> = None;
    let mut metadata = None;

    for (i, path) in paths.iter().enumerate() {
        let frame = AstroImage::from_file(path).expect("Failed to load image");

        if let Some(dims) = dimensions {
            assert!(
                frame.dimensions == dims,
                "{} frame {} has different dimensions: {:?} vs {:?}",
                frame_type,
                i,
                frame.dimensions,
                dims
            );
            // Accumulate pixel values in parallel chunks
            cpu::accumulate_parallel(&mut sum, &frame.pixels, ACCUMULATE_CHUNK_SIZE);
        } else {
            dimensions = Some(frame.dimensions);
            sum = frame.pixels.clone();
            metadata = Some(frame.metadata.clone());
        }
    }

    // Divide by count in parallel
    let inv_count = 1.0 / paths.len() as f32;
    cpu::divide_parallel(&mut sum, inv_count, ACCUMULATE_CHUNK_SIZE);

    AstroImage {
        metadata: metadata.unwrap(),
        pixels: sum,
        dimensions: dimensions.unwrap(),
    }
}
