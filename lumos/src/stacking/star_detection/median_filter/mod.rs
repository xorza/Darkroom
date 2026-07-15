//! 3x3 median filter for removing Bayer pattern artifacts.
//!
//! This filter is essential for images from color sensors where alternating rows
//! have different sensitivities due to the Bayer color filter array.

pub(crate) mod simd;

#[cfg(test)]
mod tests;

use imaginarium::Buffer2;
use rayon::prelude::*;

/// Apply 3x3 median filter to remove Bayer pattern artifacts.
///
/// Uses parallel processing for large images. Separates interior pixels
/// (full 9-element neighborhood) from edge pixels for better performance.
pub(crate) fn median_filter_3x3(pixels: &Buffer2<f32>, output: &mut Buffer2<f32>) {
    let width = pixels.width();
    let height = pixels.height();
    debug_assert_eq!(width, output.width());
    debug_assert_eq!(height, output.height());

    if width < 3 || height < 3 {
        output.copy_from_slice(pixels);
        return;
    }

    output
        .pixels_mut()
        .par_chunks_mut(width)
        .enumerate()
        .for_each(|(y, row)| {
            if y == 0 || y == height - 1 {
                // Edge row - use generic edge handling
                filter_edge_row(pixels, width, height, y, row);
            } else {
                // Interior row - fast path for most pixels
                filter_interior_row(pixels, width, y, row);
            }
        });
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

    // Edge rows span exactly 2 input rows × {2, 3} columns, so the neighborhood is always 4 or 6.
    match count {
        4 => median4(&mut neighbors[..4]),
        6 => median6(&mut neighbors[..6]),
        _ => unreachable!("edge/corner neighborhood is always 4 or 6, got {count}"),
    }
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
