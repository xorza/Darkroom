//! Shared tiled sky-background mesh estimator (SExtractor/SEP style). A [`TileGrid`] divides the
//! image into a grid of boxes and computes one robust sky value + noise per box — per-box ±σ-clip
//! then the crowding-aware Pearson mode `2.5·median − 1.5·mean` (median fallback on skew), with a
//! 3×3 grid median filter — plus natural-cubic-spline coefficients for C²-continuous interpolation.
//!
//! Foundation module (depends only on `math`/`common`): the canonical robust background estimate,
//! reused by `stacking::star_detection::background` (full-res background+noise map for detection)
//! and `background_extraction` (tile-centre samples feeding the gradient surface fit).

pub(crate) mod mesh_axis;
pub(crate) mod spline;
pub(crate) mod tile_stats;
pub(crate) mod workspace;

use crate::background_mesh::mesh_axis::MeshAxis;
use crate::background_mesh::spline::solve_natural_spline_d2;
use crate::background_mesh::tile_stats::{TileComponent, TileD2y, TileStats};
use crate::background_mesh::workspace::TileScratch;
use crate::bit_buffer2::BitBuffer2;
use crate::concurrency::JobScratchPool;
use crate::math::size2us::Size2us;
use crate::math::statistics::median_mut;
use crate::math::urect::URect;
use crate::math::vec2us::Vec2us;
use imaginarium::Buffer2;
use rayon::prelude::*;
use std::mem;
use std::ops::RangeInclusive;

/// Tile grid with precomputed centers and spline coefficients for interpolation.
#[derive(Debug)]
pub(crate) struct TileGrid {
    pub(crate) stats: Buffer2<TileStats>,
    /// Second derivatives in Y for the natural cubic spline, both planes per tile.
    /// Layout: `tiles_x` * `tiles_y`, row-major (same as stats).
    d2y: Vec<TileD2y>,
    /// Precomputed X-coordinates of tile centers (one per tile column).
    pub(crate) centers_x: Vec<f32>,
    pub(crate) centers_y: Vec<f32>,
    tile_size: usize,
    dimensions: Size2us,
}

impl TileGrid {
    /// Create an uninitialized `TileGrid` with preallocated buffers.
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
        let tile_size = clamped_tile_size(dimensions, tile_size);
        let columns = MeshAxis::new(dimensions.width, tile_size);
        let rows = MeshAxis::new(dimensions.height, tile_size);
        let (tiles_x, tiles_y) = (columns.count(), rows.count());
        let n = tiles_x * tiles_y;

        let centers_x = (0..tiles_x).map(|tile| columns.centre(tile)).collect();
        let centers_y = (0..tiles_y).map(|tile| rows.centre(tile)).collect();
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
        self.dimensions == dimensions && self.tile_size == clamped_tile_size(dimensions, tile_size)
    }

    /// Second derivative in Y at `tile` for the natural cubic spline, for one plane.
    #[inline]
    pub(crate) fn d2y(&self, component: TileComponent, tile: Vec2us) -> f32 {
        self.d2y[tile.y * self.stats.width() + tile.x].get(component)
    }

    /// Find the tile index whose center is at or before the given Y position.
    #[inline]
    /// The least and the greatest tile σ.
    pub(crate) fn sigma_range(&self) -> RangeInclusive<f32> {
        let stats = self.stats.pixels();
        let low = stats
            .iter()
            .map(|tile| tile.sigma)
            .fold(f32::INFINITY, f32::min);
        let high = stats
            .iter()
            .map(|tile| tile.sigma)
            .fold(f32::NEG_INFINITY, f32::max);
        low..=high
    }

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
        let columns = MeshAxis::new(self.dimensions.width, self.tile_size);
        let rows = MeshAxis::new(self.dimensions.height, self.tile_size);

        self.stats
            .pixels_mut()
            .par_iter_mut()
            .enumerate()
            .for_each_init(
                || tile_scratch.acquire(),
                |scratch, (idx, out)| {
                    let tx = idx % tiles_x;
                    let ty = idx / tiles_x;

                    let tile = URect::new(
                        Vec2us::new(columns.start(tx), rows.start(ty)),
                        Vec2us::new(columns.end(tx), rows.end(ty)),
                    );

                    *out = TileStats::compute(pixels, mask, tile, sigma_clip_iterations, scratch);
                },
            );
    }

    /// The 3×3 median of every tile, to reject tiles a bright object spoiled.
    ///
    /// Past the grid's edge the window reads the grid point-reflected through the nearest edge
    /// tile, `2·v(edge) − v(mirror)`: the linear continuation of the sky there. A window cut at the
    /// edge instead — SExtractor's — is lopsided, so its median pulls every edge tile half a tile
    /// toward the interior on any sky gradient; the reflected window is symmetric about its centre
    /// on a plane, whose median is then the centre exactly, at the corners too. One spoiled edge
    /// tile still loses the vote: at a corner it and its reflections make 4 of the 9 values.
    fn apply_median_filter(&mut self, scratch: &mut Buffer2<TileStats>) {
        let tiles_x = self.stats.width();
        let tiles_y = self.stats.height();

        if tiles_x < 3 || tiles_y < 3 {
            return;
        }

        let src = &self.stats;
        let dst = scratch.pixels_mut();
        let last = Vec2us::new(tiles_x - 1, tiles_y - 1);

        dst.par_iter_mut().enumerate().for_each(|(idx, out)| {
            let tile = Vec2us::new(idx % tiles_x, idx / tiles_x);

            let mut skies = [0.0f32; 9];
            let mut sigmas = [0.0f32; 9];
            let mut count = 0;

            for dy in -1isize..=1 {
                for dx in -1isize..=1 {
                    let (pivot_x, mirror_x) = reflect(tile.x, dx, last.x);
                    let (pivot_y, mirror_y) = reflect(tile.y, dy, last.y);
                    let edge = src[(pivot_x, pivot_y)];
                    let (sky, sigma) = if (pivot_x, pivot_y) == (mirror_x, mirror_y) {
                        (edge.sky, edge.sigma)
                    } else {
                        let inner = src[(mirror_x, mirror_y)];
                        (2.0 * edge.sky - inner.sky, 2.0 * edge.sigma - inner.sigma)
                    };
                    skies[count] = sky;
                    sigmas[count] = sigma;
                    count += 1;
                }
            }

            out.sky = median_mut(&mut skies);
            // A steep σ gradient can reflect below zero, which no noise level is.
            out.sigma = median_mut(&mut sigmas).max(0.0);
        });

        mem::swap(&mut self.stats, scratch);
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

/// The tile a window offset `delta` from `index` reads, as `(pivot, mirror)`: both the neighbour
/// itself inside `0..=last`, else the edge tile it reflects through and that tile's inner
/// neighbour, whose difference continues the grid linearly. `last` ≥ 1.
const fn reflect(index: usize, delta: isize, last: usize) -> (usize, usize) {
    match index.checked_add_signed(delta) {
        Some(neighbour) if neighbour <= last => (neighbour, neighbour),
        Some(_) => (last, last - 1),
        None => (0, 1),
    }
}

/// The tile size a grid over `dimensions` uses: `tile_size`, cut to the image's shorter side so
/// a tile never exceeds the frame.
const fn clamped_tile_size(dimensions: Size2us, tile_size: usize) -> usize {
    let side = if dimensions.width < dimensions.height {
        dimensions.width
    } else {
        dimensions.height
    };
    if tile_size < side { tile_size } else { side }
}

#[cfg(all(test, feature = "bench"))]
mod bench;
#[cfg(test)]
mod tests;
