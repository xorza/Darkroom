//! Mean stacking implementation.

#[cfg(feature = "bench")]
pub mod bench;

use std::io;
use std::path::Path;

use rayon::prelude::*;

use crate::astro_image::ImageDimensions;
use crate::stacking::FrameType;
use crate::stacking::error::Error;
use crate::{AstroImage, math};

/// Chunk size for parallel accumulation (16KB of f32s for good cache locality).
const ACCUMULATE_CHUNK_SIZE: usize = 4096;

/// Stack frames from paths using running mean (memory efficient).
///
/// Loads images one at a time and accumulates sum, then divides by count.
/// Uses parallel processing for accumulation.
///
/// # Errors
///
/// Returns an error if:
/// - No paths are provided
/// - Image loading fails
/// - Image dimensions don't match
pub fn stack_mean_from_paths<P: AsRef<Path>>(
    paths: &[P],
    frame_type: FrameType,
) -> Result<AstroImage, Error> {
    if paths.is_empty() {
        return Err(Error::NoPaths);
    }

    let mut sum: Vec<f32> = Vec::new();
    let mut dimensions: Option<ImageDimensions> = None;
    let mut metadata = None;

    for (i, path) in paths.iter().enumerate() {
        let path_ref = path.as_ref();
        let frame = AstroImage::from_file(path_ref).map_err(|e| Error::ImageLoad {
            path: path_ref.to_path_buf(),
            source: io::Error::other(e.to_string()),
        })?;

        if let Some(dims) = dimensions {
            if frame.dimensions != dims {
                return Err(Error::DimensionMismatch {
                    frame_type,
                    index: i,
                    expected: dims,
                    actual: frame.dimensions,
                });
            }
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

    Ok(AstroImage {
        metadata: metadata.unwrap(),
        pixels: sum,
        dimensions: dimensions.unwrap(),
    })
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
