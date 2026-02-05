//! Mean stacking implementation.


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
            if frame.dimensions() != dims {
                return Err(Error::DimensionMismatch {
                    frame_type,
                    index: i,
                    expected: dims,
                    actual: frame.dimensions(),
                });
            }
            // Accumulate pixel values in parallel chunks using SIMD
            accumulate_parallel(&mut sum, &frame.into_interleaved_pixels());
        } else {
            dimensions = Some(frame.dimensions());
            metadata = Some(frame.metadata.clone());
            sum = frame.into_interleaved_pixels();
        }
    }

    // Divide by count in parallel using SIMD
    let inv_count = 1.0 / paths.len() as f32;
    scale_parallel(&mut sum, inv_count);

    let dims = dimensions.unwrap();
    let mut result = AstroImage::from_pixels(
        ImageDimensions::new(dims.width, dims.height, dims.channels),
        sum,
    );
    result.metadata = metadata.unwrap();
    Ok(result)
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
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn test_empty_paths_returns_no_paths_error() {
        let paths: Vec<PathBuf> = vec![];
        let result = stack_mean_from_paths(&paths, FrameType::Dark);

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::NoPaths));
    }

    #[test]
    fn test_nonexistent_file_returns_image_load_error() {
        let paths = vec![PathBuf::from("/nonexistent/image.fits")];
        let result = stack_mean_from_paths(&paths, FrameType::Flat);

        assert!(result.is_err());
        match result.unwrap_err() {
            Error::ImageLoad { path, .. } => {
                assert!(path.to_string_lossy().contains("nonexistent"));
            }
            e => panic!("Expected ImageLoad error, got {:?}", e),
        }
    }

    #[test]
    fn test_accumulate_parallel_correctness() {
        let mut dst = vec![1.0, 2.0, 3.0, 4.0];
        let src = vec![0.5, 0.5, 0.5, 0.5];
        accumulate_parallel(&mut dst, &src);
        assert_eq!(dst, vec![1.5, 2.5, 3.5, 4.5]);
    }

    #[test]
    fn test_scale_parallel_correctness() {
        let mut data = vec![2.0, 4.0, 6.0, 8.0];
        scale_parallel(&mut data, 0.5);
        assert_eq!(data, vec![1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn test_accumulate_parallel_large_array() {
        let size = ACCUMULATE_CHUNK_SIZE * 3 + 100; // Multiple chunks plus remainder
        let mut dst = vec![1.0; size];
        let src = vec![2.0; size];
        accumulate_parallel(&mut dst, &src);
        assert!(dst.iter().all(|&x| (x - 3.0).abs() < f32::EPSILON));
    }

    #[test]
    fn test_scale_parallel_large_array() {
        let size = ACCUMULATE_CHUNK_SIZE * 3 + 100;
        let mut data = vec![4.0; size];
        scale_parallel(&mut data, 0.25);
        assert!(data.iter().all(|&x| (x - 1.0).abs() < f32::EPSILON));
    }
}
