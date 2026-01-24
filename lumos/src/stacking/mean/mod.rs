//! Mean stacking implementation.

#[cfg(feature = "bench")]
pub mod bench;

use std::path::Path;

use rayon::prelude::*;

use crate::AstroImage;
use crate::astro_image::ImageDimensions;
use crate::stacking::FrameType;

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
            // Accumulate pixel values in parallel chunks
            accumulate_parallel(&mut sum, &frame.pixels);
        } else {
            dimensions = Some(frame.dimensions);
            sum = frame.pixels.clone();
            metadata = Some(frame.metadata.clone());
        }
    }

    // Divide by count in parallel
    let inv_count = 1.0 / paths.len() as f32;
    divide_parallel(&mut sum, inv_count);

    AstroImage {
        metadata: metadata.unwrap(),
        pixels: sum,
        dimensions: dimensions.unwrap(),
    }
}

/// Accumulate src into dst in parallel chunks.
#[inline]
fn accumulate_parallel(dst: &mut [f32], src: &[f32]) {
    dst.par_chunks_mut(ACCUMULATE_CHUNK_SIZE)
        .zip(src.par_chunks(ACCUMULATE_CHUNK_SIZE))
        .for_each(|(dst_chunk, src_chunk)| {
            for (d, &s) in dst_chunk.iter_mut().zip(src_chunk.iter()) {
                *d += s;
            }
        });
}

/// Divide all values by a scalar in parallel.
#[inline]
fn divide_parallel(data: &mut [f32], inv_count: f32) {
    data.par_chunks_mut(ACCUMULATE_CHUNK_SIZE)
        .for_each(|chunk| {
            for d in chunk.iter_mut() {
                *d *= inv_count;
            }
        });
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_accumulate() {
        let mut dst = [1.0, 2.0, 3.0, 4.0];
        let src = [0.5, 0.5, 0.5, 0.5];
        for (d, &s) in dst.iter_mut().zip(src.iter()) {
            *d += s;
        }
        assert_eq!(dst, [1.5, 2.5, 3.5, 4.5]);
    }

    #[test]
    fn test_divide() {
        let mut data = [2.0, 4.0, 6.0, 8.0];
        for d in data.iter_mut() {
            *d *= 0.5;
        }
        assert_eq!(data, [1.0, 2.0, 3.0, 4.0]);
    }
}
