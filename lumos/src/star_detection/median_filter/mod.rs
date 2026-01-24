//! 3x3 median filter for removing Bayer pattern artifacts.
//!
//! This filter is essential for images from color sensors where alternating rows
//! have different sensitivities due to the Bayer color filter array.

#[cfg(feature = "bench")]
pub mod bench;

#[cfg(test)]
mod tests;

use rayon::prelude::*;

/// Apply 3x3 median filter to remove Bayer pattern artifacts.
///
/// Uses parallel processing for large images.
pub fn median_filter_3x3(pixels: &[f32], width: usize, height: usize) -> Vec<f32> {
    assert_eq!(
        pixels.len(),
        width * height,
        "Pixel count must match width * height"
    );

    if width < 3 || height < 3 {
        // Image too small for meaningful filtering, return copy
        return pixels.to_vec();
    }

    let mut output = vec![0.0f32; width * height];

    // Process rows in parallel for large images
    const PARALLEL_THRESHOLD: usize = 64;
    if height >= PARALLEL_THRESHOLD {
        output
            .par_chunks_mut(width)
            .enumerate()
            .for_each(|(y, row)| {
                filter_row(pixels, width, height, y, row);
            });
    } else {
        for y in 0..height {
            let row = &mut output[y * width..(y + 1) * width];
            filter_row(pixels, width, height, y, row);
        }
    }

    output
}

/// Filter a single row.
#[inline]
fn filter_row(pixels: &[f32], width: usize, height: usize, y: usize, output_row: &mut [f32]) {
    for (x, out) in output_row.iter_mut().enumerate() {
        *out = median_at(pixels, width, height, x, y);
    }
}

/// Compute median of 3x3 neighborhood at (x, y).
#[inline]
fn median_at(pixels: &[f32], width: usize, height: usize, x: usize, y: usize) -> f32 {
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

    // Fast median for small arrays
    median_of_n(&mut neighbors[..count])
}

/// Compute median of a small array (up to 9 elements).
///
/// Uses sorting networks for fixed sizes, falls back to partial sort for edges.
#[inline]
fn median_of_n(values: &mut [f32]) -> f32 {
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
            values.sort_by(|a, b| a.partial_cmp(b).unwrap());
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
