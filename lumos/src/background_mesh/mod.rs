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
pub(crate) mod tile_stats;
pub(crate) mod workspace;

use crate::background_mesh::spline::solve_natural_spline_d2;
use crate::background_mesh::tile_stats::{TileComponent, TileD2y, TileStats};
use crate::background_mesh::workspace::TileScratch;
use crate::bit_buffer2::BitBuffer2;
use crate::concurrency::JobScratchPool;
use crate::math::size2us::Size2us;
use crate::math::statistics::median_f32_mut;
use crate::math::urect::URect;
use crate::math::vec2us::Vec2us;
use imaginarium::Buffer2;
use rayon::prelude::*;

/// Tile grid with precomputed centers and spline coefficients for interpolation.
#[derive(Debug)]
pub(crate) struct TileGrid {
    pub(crate) stats: Buffer2<TileStats>,
    /// Second derivatives in Y for the natural cubic spline, both planes per tile.
    /// Layout: tiles_x * tiles_y, row-major (same as stats).
    d2y: Vec<TileD2y>,
    /// Precomputed X-coordinates of tile centers (one per tile column).
    pub(crate) centers_x: Vec<f32>,
    pub(crate) centers_y: Vec<f32>,
    tile_size: usize,
    dimensions: Size2us,
}

impl TileGrid {
    /// Create an uninitialized TileGrid with preallocated buffers.
    ///
    /// `tile_size` is clamped to the image dimensions rather than panicking on a small image:
    /// a sub-tile_size image yields a coarse (possibly single-tile) grid, which the spline
    /// interpolation path handles correctly (1-tile dimensions degenerate to a constant fill).
    ///
    /// Constructed and populated only by `MeshWorkspace`.
    fn new_uninit(dimensions: Size2us, tile_size: usize) -> Self {
        assert!(
            dimensions.width > 0 && dimensions.height > 0 && tile_size > 0,
            "TileGrid needs non-zero dimensions and tile size, got {}x{} tile {tile_size}",
            dimensions.width,
            dimensions.height
        );
        let tile_size = tile_size.min(dimensions.width).min(dimensions.height);
        let tiles_x = dimensions.width.div_ceil(tile_size);
        let tiles_y = dimensions.height.div_ceil(tile_size);
        let n = tiles_x * tiles_y;

        // Precompute tile center X-coordinates (invariant across rows)
        let centers_x: Vec<f32> = (0..tiles_x)
            .map(|tx| {
                let x_start = tx * tile_size;
                let x_end = (x_start + tile_size).min(dimensions.width);
                (x_start + x_end) as f32 * 0.5
            })
            .collect();
        let centers_y: Vec<f32> = (0..tiles_y)
            .map(|ty| {
                let y_start = ty * tile_size;
                let y_end = (y_start + tile_size).min(dimensions.height);
                (y_start + y_end) as f32 * 0.5
            })
            .collect();
        Self {
            stats: Buffer2::new_default(tiles_x, tiles_y),
            d2y: vec![TileD2y::default(); n],
            centers_x,
            centers_y,
            tile_size,
            dimensions,
        }
    }

    fn matches_layout(&self, dimensions: Size2us, tile_size: usize) -> bool {
        self.dimensions == dimensions
            && self.tile_size == tile_size.min(dimensions.width).min(dimensions.height)
    }

    /// Second derivative in Y at `tile` for the natural cubic spline, for one plane.
    #[inline]
    pub(crate) fn d2y(&self, component: TileComponent, tile: Vec2us) -> f32 {
        self.d2y[tile.to_index(self.stats.width())].get(component)
    }

    /// Find the tile index whose center is at or before the given Y position.
    #[inline]
    pub(crate) fn find_lower_tile_y(&self, pos: f32) -> usize {
        // tiles_y >= 1 always (the grid is built from an image with at least one tile row).
        let tiles_y = self.stats.height();

        // Binary search for largest tile index with center <= pos
        let mut lo = 0;
        let mut hi = tiles_y;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.centers_y[mid] <= pos {
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
        tile_scratch: &JobScratchPool<TileScratch>,
    ) {
        let tiles_x = self.stats.width();
        let tile_size = self.tile_size;
        let width = self.dimensions.width;
        let height = self.dimensions.height;

        self.stats
            .pixels_mut()
            .par_iter_mut()
            .enumerate()
            .for_each_init(
                || tile_scratch.acquire(),
                |scratch, (idx, out)| {
                    let TileScratch { values, deviations } = &mut **scratch;
                    let tx = idx % tiles_x;
                    let ty = idx / tiles_x;

                    let start = Vec2us::new(tx * tile_size, ty * tile_size);
                    let tile = URect::new(
                        start,
                        Vec2us::new(
                            (start.x + tile_size).min(width),
                            (start.y + tile_size).min(height),
                        ),
                    );

                    *out = TileStats::compute(
                        pixels,
                        mask,
                        tile,
                        sigma_clip_iterations,
                        values,
                        deviations,
                    );
                },
            );
    }

    fn apply_median_filter(&mut self, scratch: &mut Buffer2<TileStats>) {
        let tiles_x = self.stats.width();
        let tiles_y = self.stats.height();

        if tiles_x < 3 || tiles_y < 3 {
            return;
        }

        let src = self.stats.pixels();
        let dst = scratch.pixels_mut();

        dst.par_iter_mut().enumerate().for_each(|(idx, out)| {
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

        std::mem::swap(&mut self.stats, scratch);
    }

    /// Precompute second derivatives in Y for natural cubic spline interpolation.
    ///
    /// For each tile column (tx), solves a tridiagonal system to find d²f/dy²
    /// at each tile center. Natural boundary conditions: d²f=0 at endpoints.
    fn compute_y_spline_derivatives(
        &mut self,
        spline_values: &mut [f32],
        spline_d2: &mut [f32],
        spline_scratch: &mut [f32],
    ) {
        let tiles_x = self.stats.width();
        let tiles_y = self.stats.height();

        if tiles_y < 2 {
            // 0 or 1 tile rows: no spline needed, derivatives stay zero
            return;
        }

        // Destructured so the two output buffers can be borrowed mutably alongside the shared
        // `stats` read — the plane loop below needs all three at once.
        let Self {
            stats,
            d2y,
            centers_y,
            ..
        } = self;

        for tx in 0..tiles_x {
            // Both planes for this column before moving on, so the strided `stats` reads for one
            // tile column happen together rather than the whole grid being walked twice.
            for component in TileComponent::ALL {
                for (ty, value) in spline_values.iter_mut().enumerate() {
                    *value = stats[(tx, ty)].get(component);
                }
                solve_natural_spline_d2(spline_values, centers_y, spline_d2, spline_scratch);
                for (ty, &d) in spline_d2.iter().enumerate() {
                    *d2y[ty * tiles_x + tx].get_mut(component) = d;
                }
            }
        }
    }
}
