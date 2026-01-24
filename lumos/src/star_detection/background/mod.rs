//! Background estimation for star detection.
//!
//! Estimates the sky background using a tiled approach with sigma-clipped
//! statistics, then bilinearly interpolates to create a smooth background map.

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
struct TileStats {
    /// Sigma-clipped median of the tile.
    median: f32,
    /// Sigma-clipped standard deviation.
    sigma: f32,
    /// Center X of tile.
    center_x: f32,
    /// Center Y of tile.
    center_y: f32,
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
    assert!(tile_size >= 8, "Tile size must be at least 8");
    assert!(width > 0 && height > 0, "Image dimensions must be positive");

    // Compute number of tiles
    let tiles_x = width.div_ceil(tile_size);
    let tiles_y = height.div_ceil(tile_size);

    // Compute statistics for each tile in parallel
    let tile_stats: Vec<TileStats> = (0..tiles_y)
        .into_par_iter()
        .flat_map(|ty| {
            (0..tiles_x).into_par_iter().map(move |tx| {
                let x_start = tx * tile_size;
                let y_start = ty * tile_size;
                let x_end = (x_start + tile_size).min(width);
                let y_end = (y_start + tile_size).min(height);

                compute_tile_stats(pixels, width, x_start, y_start, x_end, y_end)
            })
        })
        .collect();

    // Create grid of tile statistics for interpolation
    let tile_grid: Vec<Vec<&TileStats>> = (0..tiles_y)
        .map(|ty| {
            (0..tiles_x)
                .map(|tx| &tile_stats[ty * tiles_x + tx])
                .collect()
        })
        .collect();

    // Interpolate to create per-pixel background and noise maps
    let mut background = vec![0.0f32; width * height];
    let mut noise = vec![0.0f32; width * height];

    // Process rows in parallel
    background
        .par_chunks_mut(width)
        .zip(noise.par_chunks_mut(width))
        .enumerate()
        .for_each(|(y, (bg_row, noise_row))| {
            for x in 0..width {
                let (bg, n) = interpolate_at(&tile_grid, x, y, tile_size, tiles_x, tiles_y);
                bg_row[x] = bg;
                noise_row[x] = n;
            }
        });

    BackgroundMap {
        background,
        noise,
        width,
        height,
    }
}

/// Compute sigma-clipped statistics for a single tile.
fn compute_tile_stats(
    pixels: &[f32],
    width: usize,
    x_start: usize,
    y_start: usize,
    x_end: usize,
    y_end: usize,
) -> TileStats {
    // Extract tile pixels
    let mut values: Vec<f32> = Vec::with_capacity((x_end - x_start) * (y_end - y_start));
    for y in y_start..y_end {
        for x in x_start..x_end {
            values.push(pixels[y * width + x]);
        }
    }

    // Sigma-clipped statistics (3 iterations, 3-sigma clip)
    let (median, sigma) = sigma_clipped_stats(&mut values, 3.0, 3);

    // Tile center
    let center_x = (x_start + x_end) as f32 / 2.0;
    let center_y = (y_start + y_end) as f32 / 2.0;

    TileStats {
        median,
        sigma,
        center_x,
        center_y,
    }
}

/// Compute sigma-clipped median and standard deviation.
///
/// Uses iterative sigma clipping to reject outliers (e.g., stars) and estimate
/// the background statistics.
fn sigma_clipped_stats(values: &mut [f32], kappa: f32, iterations: usize) -> (f32, f32) {
    if values.is_empty() {
        return (0.0, 0.0);
    }

    let mut len = values.len();

    for _ in 0..iterations {
        if len < 3 {
            break;
        }

        let active = &mut values[..len];

        // Compute median (this sorts active in place, but preserves values)
        let median = crate::math::median_f32_mut(active);

        // Compute MAD using a temporary buffer to avoid destroying original values
        let mut deviations: Vec<f32> = active.iter().map(|v| (v - median).abs()).collect();
        let mad = crate::math::median_f32_mut(&mut deviations);
        let sigma = mad * 1.4826; // MAD to sigma conversion

        if sigma < f32::EPSILON {
            return (median, 0.0);
        }

        // Clip values outside threshold (keep original values, just compact them)
        let threshold = kappa * sigma;
        let mut write_idx = 0;
        for i in 0..len {
            if (values[i] - median).abs() <= threshold {
                values[write_idx] = values[i];
                write_idx += 1;
            }
        }

        if write_idx == len {
            // No values clipped, converged
            break;
        }
        len = write_idx;
    }

    // Final statistics on clipped data
    let active = &mut values[..len];
    if active.is_empty() {
        return (0.0, 0.0);
    }

    let median = crate::math::median_f32_mut(active);

    // Compute standard deviation using MAD (more robust than sample std)
    let mut deviations: Vec<f32> = active.iter().map(|v| (v - median).abs()).collect();
    let mad = crate::math::median_f32_mut(&mut deviations);
    let sigma = mad * 1.4826;

    (median, sigma)
}

/// Bilinearly interpolate background/noise at a pixel position.
fn interpolate_at(
    tile_grid: &[Vec<&TileStats>],
    x: usize,
    y: usize,
    tile_size: usize,
    tiles_x: usize,
    tiles_y: usize,
) -> (f32, f32) {
    // Find the four nearest tile centers
    let fx = x as f32;
    let fy = y as f32;

    // Tile indices (clamped)
    let tx = ((x / tile_size) as isize).clamp(0, tiles_x as isize - 1) as usize;
    let ty = ((y / tile_size) as isize).clamp(0, tiles_y as isize - 1) as usize;

    // For simplicity, use nearest-neighbor at edges, bilinear in interior
    let tile = tile_grid[ty][tx];

    if tiles_x == 1 || tiles_y == 1 {
        return (tile.median, tile.sigma);
    }

    // Determine interpolation weights based on position relative to tile centers
    let tx_f = (fx - tile.center_x) / tile_size as f32 + 0.5;
    let ty_f = (fy - tile.center_y) / tile_size as f32 + 0.5;

    // Clamp to valid range
    let tx0 = tx.saturating_sub(if tx_f < 0.5 { 1 } else { 0 });
    let ty0 = ty.saturating_sub(if ty_f < 0.5 { 1 } else { 0 });
    let tx1 = (tx0 + 1).min(tiles_x - 1);
    let ty1 = (ty0 + 1).min(tiles_y - 1);

    // Get four corner tiles
    let t00 = tile_grid[ty0][tx0];
    let t10 = tile_grid[ty0][tx1];
    let t01 = tile_grid[ty1][tx0];
    let t11 = tile_grid[ty1][tx1];

    // Compute interpolation weights
    let wx = if tx1 != tx0 {
        ((fx - t00.center_x) / (t10.center_x - t00.center_x)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let wy = if ty1 != ty0 {
        ((fy - t00.center_y) / (t01.center_y - t00.center_y)).clamp(0.0, 1.0)
    } else {
        0.0
    };

    // Bilinear interpolation
    let bg = (1.0 - wx) * (1.0 - wy) * t00.median
        + wx * (1.0 - wy) * t10.median
        + (1.0 - wx) * wy * t01.median
        + wx * wy * t11.median;

    let noise = (1.0 - wx) * (1.0 - wy) * t00.sigma
        + wx * (1.0 - wy) * t10.sigma
        + (1.0 - wx) * wy * t01.sigma
        + wx * wy * t11.sigma;

    (bg, noise)
}
