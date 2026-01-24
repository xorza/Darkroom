//! Mean stacking implementation.

#[cfg(feature = "bench")]
pub mod bench;

use std::path::Path;

use rayon::prelude::*;

use crate::astro_image::ImageDimensions;
use crate::stacking::FrameType;
use crate::{AstroImage, math};

/// Chunk size for parallel accumulation (16KB of f32s for good cache locality).
const ACCUMULATE_CHUNK_SIZE: usize = 4096;

/// Stack frames from paths using running mean (memory efficient).
///
/// Loads images one at a time and accumulates sum, then divides by count.
/// Uses parallel processing for accumulation.
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
            // Accumulate pixel values in parallel chunks using SIMD
            accumulate_parallel(&mut sum, &frame.pixels);
        } else {
            dimensions = Some(frame.dimensions);
            sum = frame.pixels.clone();
            metadata = Some(frame.metadata.clone());
        }
    }

    // Divide by count in parallel using SIMD
    let inv_count = 1.0 / paths.len() as f32;
    scale_parallel(&mut sum, inv_count);

    AstroImage {
        metadata: metadata.unwrap(),
        pixels: sum,
        dimensions: dimensions.unwrap(),
    }
}

/// Accumulate src into dst in parallel chunks using SIMD.
#[inline]
fn accumulate_parallel(dst: &mut [f32], src: &[f32]) {
    dst.par_chunks_mut(ACCUMULATE_CHUNK_SIZE)
        .zip(src.par_chunks(ACCUMULATE_CHUNK_SIZE))
        .for_each(|(dst_chunk, src_chunk)| {
            math::accumulate(dst_chunk, src_chunk);
        });
}

/// Scale all values by a scalar in parallel using SIMD.
#[inline]
fn scale_parallel(data: &mut [f32], scale_val: f32) {
    data.par_chunks_mut(ACCUMULATE_CHUNK_SIZE)
        .for_each(|chunk| {
            math::scale(chunk, scale_val);
        });
}

#[cfg(test)]
mod tests {
    use crate::math;

    #[test]
    fn test_accumulate() {
        let mut dst = vec![1.0f32, 2.0, 3.0, 4.0];
        let src = vec![0.5f32, 0.5, 0.5, 0.5];
        math::accumulate(&mut dst, &src);
        assert_eq!(dst, vec![1.5, 2.5, 3.5, 4.5]);
    }

    #[test]
    fn test_scale() {
        let mut data = vec![2.0f32, 4.0, 6.0, 8.0];
        math::scale(&mut data, 0.5);
        assert_eq!(data, vec![1.0, 2.0, 3.0, 4.0]);
    }
}
