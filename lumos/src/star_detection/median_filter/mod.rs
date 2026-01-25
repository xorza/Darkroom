//! 3x3 median filter for removing Bayer pattern artifacts.
//!
//! This filter is essential for images from color sensors where alternating rows
//! have different sensitivities due to the Bayer color filter array.

#[cfg(feature = "bench")]
pub mod bench;

pub mod simd;

#[cfg(test)]
mod tests;

use rayon::prelude::*;

use super::constants::ROWS_PER_CHUNK;

/// Apply 3x3 median filter to remove Bayer pattern artifacts.
///
/// Uses parallel processing for large images. Separates interior pixels
/// (full 9-element neighborhood) from edge pixels for better performance.
pub fn median_filter_3x3(pixels: &[f32], width: usize, height: usize) -> Vec<f32> {
    assert_eq!(
        pixels.len(),
        width * height,
        "Pixel count must match width * height"
    );

    if width < 3 || height < 3 {
        return pixels.to_vec();
    }

    let mut output = vec![0.0f32; width * height];

    output
        .par_chunks_mut(width * ROWS_PER_CHUNK)
        .enumerate()
        .for_each(|(chunk_idx, chunk)| {
            let y_start = chunk_idx * ROWS_PER_CHUNK;
            let rows_in_chunk = chunk.len() / width;

            for local_y in 0..rows_in_chunk {
                let y = y_start + local_y;
                let row = &mut chunk[local_y * width..(local_y + 1) * width];

                if y == 0 || y == height - 1 {
                    // Edge row - use generic edge handling
                    filter_edge_row(pixels, width, height, y, row);
                } else {
                    // Interior row - fast path for most pixels
                    filter_interior_row(pixels, width, y, row);
                }
            }
        });

    output
}

/// Filter an interior row (y is not 0 or height-1).
/// Uses SIMD fast path for interior pixels with full 9-element neighborhood.
#[inline]
fn filter_interior_row(pixels: &[f32], width: usize, y: usize, output_row: &mut [f32]) {
    // Left edge pixel (x=0): 6 neighbors
    output_row[0] = median_at_left_edge(pixels, width, y);

    // Interior pixels (x=1 to width-2): 9 neighbors each
    let row_above = &pixels[(y - 1) * width..y * width];
    let row_curr = &pixels[y * width..(y + 1) * width];
    let row_below = &pixels[(y + 1) * width..(y + 2) * width];

    // Use SIMD-accelerated row processing
    simd::median_filter_row_simd(row_above, row_curr, row_below, output_row, width);

    // Right edge pixel (x=width-1): 6 neighbors
    output_row[width - 1] = median_at_right_edge(pixels, width, y);
}

/// Filter an edge row (y=0 or y=height-1).
#[inline]
fn filter_edge_row(pixels: &[f32], width: usize, height: usize, y: usize, output_row: &mut [f32]) {
    for (x, out) in output_row.iter_mut().enumerate() {
        *out = median_at_edge(pixels, width, height, x, y);
    }
}

/// Compute median at left edge (x=0) for interior row.
#[inline]
fn median_at_left_edge(pixels: &[f32], width: usize, y: usize) -> f32 {
    let mut v = [0.0f32; 6];
    v[0] = pixels[(y - 1) * width];
    v[1] = pixels[(y - 1) * width + 1];
    v[2] = pixels[y * width];
    v[3] = pixels[y * width + 1];
    v[4] = pixels[(y + 1) * width];
    v[5] = pixels[(y + 1) * width + 1];
    median6(&mut v)
}

/// Compute median at right edge (x=width-1) for interior row.
#[inline]
fn median_at_right_edge(pixels: &[f32], width: usize, y: usize) -> f32 {
    let x = width - 1;
    let mut v = [0.0f32; 6];
    v[0] = pixels[(y - 1) * width + x - 1];
    v[1] = pixels[(y - 1) * width + x];
    v[2] = pixels[y * width + x - 1];
    v[3] = pixels[y * width + x];
    v[4] = pixels[(y + 1) * width + x - 1];
    v[5] = pixels[(y + 1) * width + x];
    median6(&mut v)
}

/// Compute median for edge/corner pixels with variable neighborhood size.
#[inline]
fn median_at_edge(pixels: &[f32], width: usize, height: usize, x: usize, y: usize) -> f32 {
    let mut neighbors = [0.0f32; 9];
    let mut count = 0;

    let y_start = y.saturating_sub(1);
    let y_end = (y + 2).min(height);
    let x_start = x.saturating_sub(1);
    let x_end = (x + 2).min(width);

    for ny in y_start..y_end {
        let row_offset = ny * width;
        for nx in x_start..x_end {
            neighbors[count] = pixels[row_offset + nx];
            count += 1;
        }
    }

    median_of_n(&mut neighbors[..count])
}

/// Compute median of a small array (up to 9 elements).
///
/// Uses sorting networks for fixed sizes (3-6, 9) for optimal performance,
/// falls back to partial sort for other sizes (7, 8).
#[inline]
pub fn median_of_n(values: &mut [f32]) -> f32 {
    let n = values.len();
    match n {
        0 => 0.0,
        1 => values[0],
        2 => (values[0] + values[1]) * 0.5,
        3 => median3(values),
        4 => median4(values),
        5 => median5(values),
        6 => median6(values),
        9 => median9(values),
        _ => {
            // Fallback for 7, 8 (edge cases)
            values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            values[n / 2]
        }
    }
}

/// Median of 3 elements.
#[inline]
fn median3(v: &mut [f32]) -> f32 {
    // Sort first two
    if v[0] > v[1] {
        v.swap(0, 1);
    }
    // v[0] <= v[1]
    if v[1] > v[2] {
        v.swap(1, 2);
        // Now v[0] <= old_v[1], v[1] = v[2], v[2] = old_v[1]
        if v[0] > v[1] {
            v.swap(0, 1);
        }
    }
    v[1]
}

/// Median of 4 elements (average of middle two).
#[inline]
fn median4(v: &mut [f32]) -> f32 {
    // Sorting network for 4 elements
    if v[0] > v[1] {
        v.swap(0, 1);
    }
    if v[2] > v[3] {
        v.swap(2, 3);
    }
    if v[0] > v[2] {
        v.swap(0, 2);
    }
    if v[1] > v[3] {
        v.swap(1, 3);
    }
    if v[1] > v[2] {
        v.swap(1, 2);
    }
    (v[1] + v[2]) * 0.5
}

/// Median of 5 elements.
#[inline]
fn median5(v: &mut [f32]) -> f32 {
    // Optimal sorting network for 5 elements (9 comparisons)
    if v[0] > v[1] {
        v.swap(0, 1);
    }
    if v[3] > v[4] {
        v.swap(3, 4);
    }
    if v[2] > v[4] {
        v.swap(2, 4);
    }
    if v[2] > v[3] {
        v.swap(2, 3);
    }
    if v[0] > v[3] {
        v.swap(0, 3);
    }
    if v[0] > v[2] {
        v.swap(0, 2);
    }
    if v[1] > v[4] {
        v.swap(1, 4);
    }
    if v[1] > v[3] {
        v.swap(1, 3);
    }
    if v[1] > v[2] {
        v.swap(1, 2);
    }
    v[2]
}

/// Median of 6 elements (average of middle two).
#[inline]
fn median6(v: &mut [f32]) -> f32 {
    // Sorting network for 6 elements
    if v[0] > v[1] {
        v.swap(0, 1);
    }
    if v[2] > v[3] {
        v.swap(2, 3);
    }
    if v[4] > v[5] {
        v.swap(4, 5);
    }
    if v[0] > v[2] {
        v.swap(0, 2);
    }
    if v[1] > v[3] {
        v.swap(1, 3);
    }
    if v[0] > v[4] {
        v.swap(0, 4);
    }
    if v[1] > v[5] {
        v.swap(1, 5);
    }
    if v[1] > v[2] {
        v.swap(1, 2);
    }
    if v[3] > v[4] {
        v.swap(3, 4);
    }
    if v[2] > v[4] {
        v.swap(2, 4);
    }
    if v[1] > v[2] {
        v.swap(1, 2);
    }
    if v[3] > v[4] {
        v.swap(3, 4);
    }
    if v[2] > v[3] {
        v.swap(2, 3);
    }
    (v[2] + v[3]) * 0.5
}

/// Median of 9 elements (the common case for interior pixels).
#[inline]
fn median9(v: &mut [f32]) -> f32 {
    // Partial sorting network to find median (position 4)
    // We don't need a full sort, just need element 4 in place

    // Sort pairs
    if v[0] > v[1] {
        v.swap(0, 1);
    }
    if v[3] > v[4] {
        v.swap(3, 4);
    }
    if v[6] > v[7] {
        v.swap(6, 7);
    }
    if v[1] > v[2] {
        v.swap(1, 2);
    }
    if v[4] > v[5] {
        v.swap(4, 5);
    }
    if v[7] > v[8] {
        v.swap(7, 8);
    }
    if v[0] > v[1] {
        v.swap(0, 1);
    }
    if v[3] > v[4] {
        v.swap(3, 4);
    }
    if v[6] > v[7] {
        v.swap(6, 7);
    }

    // Cross comparisons
    if v[0] > v[3] {
        v.swap(0, 3);
    }
    if v[3] > v[6] {
        v.swap(3, 6);
    }
    if v[0] > v[3] {
        v.swap(0, 3);
    }

    if v[1] > v[4] {
        v.swap(1, 4);
    }
    if v[4] > v[7] {
        v.swap(4, 7);
    }
    if v[1] > v[4] {
        v.swap(1, 4);
    }

    if v[2] > v[5] {
        v.swap(2, 5);
    }
    if v[5] > v[8] {
        v.swap(5, 8);
    }
    if v[2] > v[5] {
        v.swap(2, 5);
    }

    // Final comparisons to place median
    if v[3] > v[4] {
        v.swap(3, 4);
    }
    if v[4] > v[5] {
        v.swap(4, 5);
    }
    if v[3] > v[4] {
        v.swap(3, 4);
    }

    v[4]
}
