//! Shared tiled sky-background mesh estimator (SExtractor/SEP style). A [`TileGrid`] divides the
//! image into a grid of boxes and computes one robust sky value + noise per box — per-box ±σ-clip
//! then the crowding-aware Pearson mode `2.5·median − 1.5·mean` (median fallback on skew), with a
//! 3×3 grid median filter — plus natural-cubic-spline coefficients for C²-continuous interpolation.
//!
//! Foundation module (depends only on `math`/`common`): the canonical robust background estimate,
//! reused by `stacking::star_detection::background` (full-res background+noise map for detection)
//! and `background_extraction` (tile-centre samples feeding the gradient surface fit).

pub(crate) mod spline;
#[cfg(test)]
mod tests;
mod tile_stats;

use crate::background_mesh::spline::solve_natural_spline_d2;
use crate::background_mesh::tile_stats::compute_tile_stats;
use crate::math::statistics::median_f32_mut;
use common::BitBuffer2;
use common::Buffer2;
use common::Vec2us;
use rayon::prelude::*;

/// Tile statistics computed during background estimation.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TileStats {
    /// Sky level: SExtractor's crowding-aware estimator (Pearson mode, median fallback when
    /// strongly skewed) over the sigma-clip survivors. Computed by `tile_stats::compute_tile_stats`.
    pub(crate) sky: f32,
    pub(crate) sigma: f32,
}

/// Tile grid with precomputed centers and spline coefficients for interpolation.
#[derive(Debug)]
pub(crate) struct TileGrid {
    stats: Buffer2<TileStats>,
    /// Second derivatives in Y direction for natural cubic spline (sky).
    /// Layout: tiles_x * tiles_y, row-major (same as stats).
    d2y_sky: Vec<f32>,
    /// Second derivatives in Y direction for natural cubic spline (sigma).
    d2y_sigma: Vec<f32>,
    /// Precomputed X-coordinates of tile centers (one per tile column).
    pub(crate) centers_x: Vec<f32>,
    tile_size: usize,
    dimensions: Vec2us,
}

impl TileGrid {
    /// Create an uninitialized TileGrid with preallocated buffers.
    ///
    /// Call `compute` to fill in the tile statistics.
    pub(crate) fn new_uninit(width: usize, height: usize, tile_size: usize) -> Self {
        let tiles_x = width.div_ceil(tile_size);
        let tiles_y = height.div_ceil(tile_size);
        let n = tiles_x * tiles_y;

        // Precompute tile center X-coordinates (invariant across rows)
        let centers_x: Vec<f32> = (0..tiles_x)
            .map(|tx| {
                let x_start = tx * tile_size;
                let x_end = (x_start + tile_size).min(width);
                (x_start + x_end) as f32 * 0.5
            })
            .collect();

        Self {
            stats: Buffer2::new_default(tiles_x, tiles_y),
            d2y_sky: vec![0.0; n],
            d2y_sigma: vec![0.0; n],
            centers_x,
            tile_size,
            dimensions: Vec2us::new(width, height),
        }
    }

    /// Compute tile statistics, reusing the existing buffer. `median_filter` applies the 3×3 grid
    /// median filter — keep it on for detection (de-rings a star-contaminated tile before
    /// interpolation); turn it off when fitting a smooth surface to the tile samples, where it would
    /// bias the boundary tiles of a real gradient.
    pub(crate) fn compute(
        &mut self,
        pixels: &Buffer2<f32>,
        mask: Option<&BitBuffer2>,
        sigma_clip_iterations: usize,
        median_filter: bool,
    ) {
        debug_assert_eq!(pixels.width(), self.dimensions.x);
        debug_assert_eq!(pixels.height(), self.dimensions.y);

        self.fill_tile_stats(pixels, mask, sigma_clip_iterations);
        if median_filter {
            self.apply_median_filter();
        }
        self.compute_y_spline_derivatives();
    }

    #[inline]
    pub(crate) fn get(&self, tx: usize, ty: usize) -> TileStats {
        self.stats[(tx, ty)]
    }

    #[inline]
    pub(crate) fn tiles_x(&self) -> usize {
        self.stats.width()
    }

    #[inline]
    pub(crate) fn tiles_y(&self) -> usize {
        self.stats.height()
    }

    #[inline]
    pub(crate) fn center_y(&self, ty: usize) -> f32 {
        let y_start = ty * self.tile_size;
        let y_end = (y_start + self.tile_size).min(self.dimensions.y);
        (y_start + y_end) as f32 * 0.5
    }

    /// Second derivative of sky in Y at tile (tx, ty) for natural cubic spline.
    #[inline]
    pub(crate) fn d2y_sky(&self, tx: usize, ty: usize) -> f32 {
        self.d2y_sky[ty * self.tiles_x() + tx]
    }

    /// Second derivative of sigma in Y at tile (tx, ty) for natural cubic spline.
    #[inline]
    pub(crate) fn d2y_sigma(&self, tx: usize, ty: usize) -> f32 {
        self.d2y_sigma[ty * self.tiles_x() + tx]
    }

    /// Find the tile index whose center is at or before the given Y position.
    #[inline]
    pub(crate) fn find_lower_tile_y(&self, pos: f32) -> usize {
        // tiles_y >= 1 always (the grid is built from an image with at least one tile row).
        let tiles_y = self.tiles_y();

        // Binary search for largest tile index with center <= pos
        let mut lo = 0;
        let mut hi = tiles_y;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.center_y(mid) <= pos {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo.saturating_sub(1)
    }

    fn fill_tile_stats(
        &mut self,
        pixels: &Buffer2<f32>,
        mask: Option<&BitBuffer2>,
        sigma_clip_iterations: usize,
    ) {
        let tiles_x = self.tiles_x();
        let tile_size = self.tile_size;
        let width = self.dimensions.x;
        let height = self.dimensions.y;
        let max_tile_pixels = tile_size * tile_size;

        self.stats
            .pixels_mut()
            .par_iter_mut()
            .enumerate()
            .for_each_init(
                || {
                    (
                        Vec::with_capacity(max_tile_pixels),
                        Vec::with_capacity(max_tile_pixels),
                    )
                },
                |(values, deviations), (idx, out)| {
                    let tx = idx % tiles_x;
                    let ty = idx / tiles_x;

                    let x_start = tx * tile_size;
                    let y_start = ty * tile_size;
                    let x_end = (x_start + tile_size).min(width);
                    let y_end = (y_start + tile_size).min(height);

                    *out = compute_tile_stats(
                        pixels,
                        mask,
                        x_start,
                        x_end,
                        y_start,
                        y_end,
                        sigma_clip_iterations,
                        values,
                        deviations,
                    );
                },
            );
    }

    fn apply_median_filter(&mut self) {
        let tiles_x = self.tiles_x();
        let tiles_y = self.tiles_y();

        if tiles_x < 3 || tiles_y < 3 {
            return;
        }

        let src = self.stats.pixels();
        let mut dst: Buffer2<TileStats> = Buffer2::new_default(tiles_x, tiles_y);

        dst.pixels_mut()
            .par_iter_mut()
            .enumerate()
            .for_each(|(idx, out)| {
                let tx = idx % tiles_x;
                let ty = idx / tiles_x;

                let mut skies = [0.0f32; 9];
                let mut sigmas = [0.0f32; 9];
                let mut count = 0;

                for dy in -1i32..=1 {
                    for dx in -1i32..=1 {
                        let nx = tx as i32 + dx;
                        let ny = ty as i32 + dy;

                        if nx >= 0 && nx < tiles_x as i32 && ny >= 0 && ny < tiles_y as i32 {
                            let neighbor = src[ny as usize * tiles_x + nx as usize];
                            skies[count] = neighbor.sky;
                            sigmas[count] = neighbor.sigma;
                            count += 1;
                        }
                    }
                }

                out.sky = median_f32_mut(&mut skies[..count]);
                out.sigma = median_f32_mut(&mut sigmas[..count]);
            });

        std::mem::swap(&mut self.stats, &mut dst);
    }

    /// Precompute second derivatives in Y for natural cubic spline interpolation.
    ///
    /// For each tile column (tx), solves a tridiagonal system to find d²f/dy²
    /// at each tile center. Natural boundary conditions: d²f=0 at endpoints.
    fn compute_y_spline_derivatives(&mut self) {
        let tiles_x = self.tiles_x();
        let tiles_y = self.tiles_y();

        if tiles_y < 2 {
            // 0 or 1 tile rows: no spline needed, derivatives stay zero
            return;
        }

        // Collect Y centers
        let centers_y: Vec<f32> = (0..tiles_y).map(|ty| self.center_y(ty)).collect();

        // Scratch buffers for tridiagonal solver (reused per column)
        let mut values = vec![0.0f32; tiles_y];
        let mut d2 = vec![0.0f32; tiles_y];
        let mut scratch = vec![0.0f32; tiles_y.saturating_sub(2)];

        for tx in 0..tiles_x {
            // Solve for sky
            for (ty, val) in values.iter_mut().enumerate() {
                *val = self.stats[(tx, ty)].sky;
            }
            solve_natural_spline_d2(&values, &centers_y, &mut d2, &mut scratch);
            for (ty, &d) in d2.iter().enumerate() {
                self.d2y_sky[ty * tiles_x + tx] = d;
            }

            // Solve for sigma
            for (ty, val) in values.iter_mut().enumerate() {
                *val = self.stats[(tx, ty)].sigma;
            }
            solve_natural_spline_d2(&values, &centers_y, &mut d2, &mut scratch);
            for (ty, &d) in d2.iter().enumerate() {
                self.d2y_sigma[ty * tiles_x + tx] = d;
            }
        }
    }
}
