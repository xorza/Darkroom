//! Background estimation for star detection.
//!
//! Estimates the sky background using a tiled approach with sigma-clipped
//! statistics, then bilinearly interpolates to create a smooth background map.

#[cfg(feature = "bench")]
pub mod bench;

#[cfg(test)]
mod tests;

use rayon::prelude::*;

/// Background map with per-pixel background and noise estimates.
#[derive(Debug)]
pub struct BackgroundMap {
    /// Per-pixel background values.
    pub background: Vec<f32>,
    /// Per-pixel noise (sigma) estimates.
    pub noise: Vec<f32>,
    /// Image width.
    #[allow(dead_code)]
    pub width: usize,
    /// Image height.
    #[allow(dead_code)]
    pub height: usize,
}

impl BackgroundMap {
    /// Get background value at a pixel position.
    #[inline]
    #[allow(dead_code)]
    pub fn get_background(&self, x: usize, y: usize) -> f32 {
        self.background[y * self.width + x]
    }

    /// Get noise estimate at a pixel position.
    #[inline]
    #[allow(dead_code)]
    pub fn get_noise(&self, x: usize, y: usize) -> f32 {
        self.noise[y * self.width + x]
    }

    /// Get background-subtracted value at a pixel position.
    #[inline]
    #[allow(dead_code)]
    pub fn subtract(&self, pixels: &[f32], x: usize, y: usize) -> f32 {
        let idx = y * self.width + x;
        pixels[idx] - self.background[idx]
    }
}

/// Tile statistics computed during background estimation.
#[derive(Clone, Copy)]
struct TileStats {
    median: f32,
    sigma: f32,
}

/// Tile grid with precomputed centers for interpolation.
struct TileGrid {
    stats: Vec<TileStats>,
    centers_x: Vec<f32>,
    centers_y: Vec<f32>,
    tiles_x: usize,
    tiles_y: usize,
}

impl TileGrid {
    #[inline]
    fn get(&self, tx: usize, ty: usize) -> TileStats {
        self.stats[ty * self.tiles_x + tx]
    }
}

/// Estimate background using tiled sigma-clipped statistics.
///
/// # Algorithm
///
/// 1. Divide image into tiles of size `tile_size × tile_size`
/// 2. For each tile, compute sigma-clipped median and standard deviation
/// 3. Bilinearly interpolate between tile centers to get per-pixel values
///
/// # Arguments
/// * `pixels` - Grayscale image data
/// * `width` - Image width
/// * `height` - Image height
/// * `tile_size` - Size of tiles (typically 32-128 pixels)
pub fn estimate_background(
    pixels: &[f32],
    width: usize,
    height: usize,
    tile_size: usize,
) -> BackgroundMap {
    assert!(
        (16..=256).contains(&tile_size),
        "Tile size must be between 16 and 256"
    );
    assert!(
        width >= tile_size && height >= tile_size,
        "Image must be at least tile_size x tile_size"
    );

    let tiles_x = width.div_ceil(tile_size);
    let tiles_y = height.div_ceil(tile_size);
    let max_tile_pixels = tile_size * tile_size;

    // Compute statistics for each tile in parallel with per-thread scratch buffers
    let tile_stats: Vec<TileStats> = (0..tiles_y * tiles_x)
        .into_par_iter()
        .map_init(
            || {
                (
                    Vec::with_capacity(max_tile_pixels),
                    Vec::with_capacity(max_tile_pixels),
                )
            },
            |(values_buf, deviations_buf), idx| {
                let ty = idx / tiles_x;
                let tx = idx % tiles_x;

                let x_start = tx * tile_size;
                let y_start = ty * tile_size;
                let x_end = (x_start + tile_size).min(width);
                let y_end = (y_start + tile_size).min(height);

                compute_tile_stats(
                    pixels,
                    width,
                    x_start,
                    x_end,
                    y_start,
                    y_end,
                    values_buf,
                    deviations_buf,
                )
            },
        )
        .collect();

    // Build tile grid with precomputed centers
    let grid = TileGrid {
        stats: tile_stats,
        centers_x: (0..tiles_x)
            .map(|tx| {
                let x_start = tx * tile_size;
                let x_end = (x_start + tile_size).min(width);
                (x_start + x_end) as f32 * 0.5
            })
            .collect(),
        centers_y: (0..tiles_y)
            .map(|ty| {
                let y_start = ty * tile_size;
                let y_end = (y_start + tile_size).min(height);
                (y_start + y_end) as f32 * 0.5
            })
            .collect(),
        tiles_x,
        tiles_y,
    };

    // Allocate output buffers
    let mut background = vec![0.0f32; width * height];
    let mut noise = vec![0.0f32; width * height];

    // Process rows in parallel with chunking to reduce false sharing
    const ROWS_PER_CHUNK: usize = 8;
    background
        .par_chunks_mut(width * ROWS_PER_CHUNK)
        .zip(noise.par_chunks_mut(width * ROWS_PER_CHUNK))
        .enumerate()
        .for_each(|(chunk_idx, (bg_chunk, noise_chunk))| {
            let y_start = chunk_idx * ROWS_PER_CHUNK;
            let rows_in_chunk = bg_chunk.len() / width;

            for local_y in 0..rows_in_chunk {
                let y = y_start + local_y;
                let row_offset = local_y * width;
                let bg_row = &mut bg_chunk[row_offset..row_offset + width];
                let noise_row = &mut noise_chunk[row_offset..row_offset + width];

                interpolate_row(bg_row, noise_row, y, &grid);
            }
        });

    BackgroundMap {
        background,
        noise,
        width,
        height,
    }
}

/// Compute sigma-clipped statistics for a single tile using provided scratch buffers.
#[allow(clippy::too_many_arguments)]
fn compute_tile_stats(
    pixels: &[f32],
    width: usize,
    x_start: usize,
    x_end: usize,
    y_start: usize,
    y_end: usize,
    values: &mut Vec<f32>,
    deviations: &mut Vec<f32>,
) -> TileStats {
    // Clear and fill values buffer
    values.clear();
    for y in y_start..y_end {
        let row_start = y * width + x_start;
        values.extend_from_slice(&pixels[row_start..row_start + (x_end - x_start)]);
    }

    // Sigma-clipped statistics (3 iterations, 3-sigma clip)
    sigma_clipped_stats(values, deviations, 3.0, 3)
}

/// Compute sigma-clipped median and standard deviation using scratch buffers.
fn sigma_clipped_stats(
    values: &mut [f32],
    deviations: &mut Vec<f32>,
    kappa: f32,
    iterations: usize,
) -> TileStats {
    if values.is_empty() {
        return TileStats {
            median: 0.0,
            sigma: 0.0,
        };
    }

    let mut len = values.len();

    for _ in 0..iterations {
        if len < 3 {
            break;
        }

        let active = &mut values[..len];

        // Compute median
        let median = crate::math::median_f32_mut(active);

        // Compute MAD using deviations buffer
        deviations.clear();
        deviations.extend(active.iter().map(|v| (v - median).abs()));
        let mad = crate::math::median_f32_mut(deviations);
        let sigma = mad * 1.4826;

        if sigma < f32::EPSILON {
            return TileStats { median, sigma: 0.0 };
        }

        // Clip values outside threshold
        let threshold = kappa * sigma;
        let mut write_idx = 0;
        for i in 0..len {
            if (values[i] - median).abs() <= threshold {
                values[write_idx] = values[i];
                write_idx += 1;
            }
        }

        if write_idx == len {
            break;
        }
        len = write_idx;
    }

    // Final statistics
    let active = &mut values[..len];
    if active.is_empty() {
        return TileStats {
            median: 0.0,
            sigma: 0.0,
        };
    }

    let median = crate::math::median_f32_mut(active);

    deviations.clear();
    deviations.extend(active.iter().map(|v| (v - median).abs()));
    let mad = crate::math::median_f32_mut(deviations);
    let sigma = mad * 1.4826;

    TileStats { median, sigma }
}

/// Interpolate an entire row using segment-based bilinear interpolation.
///
/// Instead of computing tile indices per-pixel, we process the row in segments
/// where each segment has constant tile corners. This amortizes tile lookups
/// and Y-weight calculations across many pixels.
fn interpolate_row(bg_row: &mut [f32], noise_row: &mut [f32], y: usize, grid: &TileGrid) {
    let fy = y as f32;
    let width = bg_row.len();

    // Compute Y tile indices and weight once for the entire row
    let ty0 = find_lower_tile(fy, &grid.centers_y);
    let ty1 = (ty0 + 1).min(grid.tiles_y - 1);

    let wy = if ty1 != ty0 {
        ((fy - grid.centers_y[ty0]) / (grid.centers_y[ty1] - grid.centers_y[ty0])).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let wy_inv = 1.0 - wy;

    // Process row in segments between tile center X boundaries
    let mut x = 0usize;

    for tx0 in 0..grid.tiles_x {
        let tx1 = (tx0 + 1).min(grid.tiles_x - 1);

        // Segment runs from current x to next tile center (or end of row)
        let segment_end = if tx0 + 1 < grid.tiles_x {
            // Segment ends at next tile center
            (grid.centers_x[tx0 + 1].floor() as usize).min(width)
        } else {
            width
        };

        // Skip if segment is empty or we've passed it
        if segment_end <= x {
            continue;
        }

        // Fetch the four corner tiles for this segment
        let t00 = grid.get(tx0, ty0);
        let t10 = grid.get(tx1, ty0);
        let t01 = grid.get(tx0, ty1);
        let t11 = grid.get(tx1, ty1);

        // Precompute Y-blended values for left and right tile columns
        let left_bg = wy_inv * t00.median + wy * t01.median;
        let right_bg = wy_inv * t10.median + wy * t11.median;
        let left_noise = wy_inv * t00.sigma + wy * t01.sigma;
        let right_noise = wy_inv * t10.sigma + wy * t11.sigma;

        // Precompute X interpolation constants
        let (x_scale, x_offset) = if tx1 != tx0 {
            let inv_dx = 1.0 / (grid.centers_x[tx1] - grid.centers_x[tx0]);
            (inv_dx, grid.centers_x[tx0])
        } else {
            (0.0, 0.0)
        };

        // Process all pixels in this segment with simple linear interpolation
        for px in x..segment_end {
            let wx = if tx1 != tx0 {
                ((px as f32 - x_offset) * x_scale).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let wx_inv = 1.0 - wx;

            bg_row[px] = wx_inv * left_bg + wx * right_bg;
            noise_row[px] = wx_inv * left_noise + wx * right_noise;
        }

        x = segment_end;
        if x >= width {
            break;
        }
    }
}

/// Find the tile index whose center is at or before the given position.
#[inline]
fn find_lower_tile(pos: f32, centers: &[f32]) -> usize {
    // Linear search is efficient for small tile counts (typically < 20)
    for i in (0..centers.len()).rev() {
        if centers[i] <= pos {
            return i;
        }
    }
    0
}
