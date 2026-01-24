//! CPU processing for median stacking with SIMD dispatch.

use super::scalar;

use rayon::prelude::*;

use crate::astro_image::{AstroImage, ImageDimensions};
use crate::stacking::cache::ImageCache;

/// Process cached images in horizontal chunks using parallel CPU processing.
pub fn process_chunked(cache: &ImageCache, chunk_rows: usize) -> AstroImage {
    let dims = cache.dimensions();
    let frame_count = cache.frame_count();

    let mut output_pixels = vec![0.0f32; dims.pixel_count()];

    process_chunks(
        cache,
        &mut output_pixels,
        dims,
        frame_count,
        chunk_rows,
        scalar::median_f32,
    );

    AstroImage {
        metadata: cache.metadata().clone(),
        pixels: output_pixels,
        dimensions: dims,
    }
}

/// Generic chunk processing with a combine function.
fn process_chunks<F>(
    cache: &ImageCache,
    output_pixels: &mut [f32],
    dims: ImageDimensions,
    frame_count: usize,
    chunk_rows: usize,
    combine: F,
) where
    F: Fn(&[f32]) -> f32 + Sync,
{
    let width = dims.width;
    let height = dims.height;
    let channels = dims.channels;

    let num_chunks = height.div_ceil(chunk_rows);

    for chunk_idx in 0..num_chunks {
        let start_row = chunk_idx * chunk_rows;
        let end_row = (start_row + chunk_rows).min(height);
        let rows_in_chunk = end_row - start_row;
        let pixels_in_chunk = rows_in_chunk * width * channels;

        // Get slices from all frames (zero-copy from mmap)
        let chunks: Vec<&[f32]> = (0..frame_count)
            .map(|frame_idx| cache.read_chunk(frame_idx, start_row, end_row))
            .collect();

        let output_slice = &mut output_pixels[start_row * width * channels..][..pixels_in_chunk];

        output_slice
            .par_chunks_mut(width * channels)
            .enumerate()
            .for_each(|(row_in_chunk, row_output)| {
                // One buffer per thread, reused for all pixels in row
                let mut values = vec![0.0f32; frame_count];
                let row_offset = row_in_chunk * width * channels;

                for (pixel_in_row, out) in row_output.iter_mut().enumerate() {
                    let pixel_idx = row_offset + pixel_in_row;
                    // Gather values from all frames
                    for (frame_idx, chunk) in chunks.iter().enumerate() {
                        values[frame_idx] = chunk[pixel_idx];
                    }
                    *out = combine(&values);
                }
            });
    }
}
