//! Shared tiled sky-background mesh estimator (SExtractor/SEP style). A [`BgMesh`] divides
//! the image into a grid of boxes and computes one robust sky value + noise per box — per-box
//! ±σ-clip then the crowding-aware Pearson mode `2.5·median − 1.5·mean` (median fallback on skew),
//! with a 3×3 grid median filter — and [`mesh_spline::MeshSpline`], the natural bicubic spline
//! that carries a mesh's statistics to every pixel, C²-continuous.
//!
//! The canonical robust background estimate, reused by `star_detection::bg` (full-res
//! background+noise map for detection), `image_ops::extract_bg` (tile-centre samples
//! feeding the gradient surface fit), and per color of a mosaic by [`color_mesh`] for the
//! hot-pixel and cosmic-ray scans.

pub(crate) mod color_mesh;
pub(crate) mod mesh_axis;
pub(crate) mod mesh_spline;
pub(crate) mod mesh_surface;
pub(crate) mod mesh_workspace;
pub(crate) mod spline;
pub(crate) mod tile_stats;

use std::mem;
use std::ops::RangeInclusive;

use common::par;
use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::bg_mesh::mesh_axis::MeshAxis;
use crate::bg_mesh::tile_stats::{TileInput, TileScratch, TileStats};
use crate::math::size2us::Size2us;
use crate::math::statistics;
use crate::math::urect::URect;
use crate::math::vec2us::Vec2us;
use crate::scratch_pool::ScratchPool;

/// Tile grid with each tile's statistics and centre.
#[derive(Debug)]
pub(crate) struct BgMesh {
    pub(crate) stats: Buffer2<TileStats>,
    /// Precomputed X-coordinates of tile centers (one per tile column).
    pub(crate) centers_x: Vec<f32>,
    pub(crate) centers_y: Vec<f32>,
    tile_size: usize,
    dimensions: Size2us,
    /// Which tiles measured their own statistics; the others were filled from them.
    measured: Vec<bool>,
}

impl BgMesh {
    /// Create an uninitialized `BgMesh` with preallocated buffers.
    ///
    /// `tile_size` is clamped to the image dimensions rather than panicking on a small image:
    /// a sub-tile_size image yields a coarse (possibly single-tile) grid, which the spline
    /// interpolation path handles correctly (1-tile dimensions degenerate to a constant fill).
    ///
    /// Constructed and populated only by [`MeshWorkspace`](mesh_workspace::MeshWorkspace).
    fn new_uninit(dimensions: Size2us, tile_size: usize) -> Self {
        debug_assert!(
            dimensions.width > 0 && dimensions.height > 0 && tile_size > 0,
            "BackgroundMesh needs non-zero dimensions and tile size, got {}x{} tile {tile_size}",
            dimensions.width,
            dimensions.height
        );
        let tile_size = clamped_tile_size(dimensions, tile_size);
        let cols = MeshAxis::new(dimensions.width, tile_size);
        let rows = MeshAxis::new(dimensions.height, tile_size);
        let (tiles_x, tiles_y) = (cols.count(), rows.count());
        let n = tiles_x * tiles_y;

        let centers_x = (0..tiles_x).map(|tile| cols.centre(tile)).collect();
        let centers_y = (0..tiles_y).map(|tile| rows.centre(tile)).collect();
        Self {
            stats: Buffer2::new_default(tiles_x, tiles_y),
            centers_x,
            centers_y,
            tile_size,
            dimensions,
            measured: vec![false; n],
        }
    }

    fn matches_layout(&self, dimensions: Size2us, tile_size: usize) -> bool {
        self.dimensions == dimensions && self.tile_size == clamped_tile_size(dimensions, tile_size)
    }

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

    /// Every tile's statistics, a bad tile's taken from its good neighbours (see
    /// [`Self::fill_bad_tiles`]). When the mask leaves no good tile at all, it covers too much to
    /// leave a sky anywhere, and every tile is measured without it.
    fn fill_tile_stats(&mut self, input: &TileInput<'_>, tile_scratch: &ScratchPool<TileScratch>) {
        let tiles_x = self.stats.width();
        let cols = MeshAxis::new(self.dimensions.width, self.tile_size);
        let rows = MeshAxis::new(self.dimensions.height, self.tile_size);
        let tile_at = |idx: usize| {
            let (tx, ty) = (idx % tiles_x, idx / tiles_x);
            URect::new(
                Vec2us::new(cols.start(tx), rows.start(ty)),
                Vec2us::new(cols.end(tx), rows.end(ty)),
            )
        };

        self.measured.resize(self.stats.pixels().len(), false);
        par::iter_mut(self.stats.pixels_mut())
            .zip(par::iter_mut(&mut self.measured))
            .enumerate()
            .for_each_init(
                || tile_scratch.acquire(),
                |scratch, (idx, (out, measured))| {
                    let stats = TileStats::compute(input, tile_at(idx), scratch);
                    *measured = stats.is_some();
                    *out = stats.unwrap_or_default();
                },
            );
        if self.measured.iter().all(|&measured| measured) {
            return;
        }
        if !self.measured.contains(&true) {
            let unmasked = TileInput {
                mask: None,
                ..*input
            };
            par::iter_mut(self.stats.pixels_mut())
                .enumerate()
                .for_each_init(
                    || tile_scratch.acquire(),
                    |scratch, (idx, out)| {
                        // An eligibility test that takes no pixel of a tile leaves it nothing to
                        // measure, mask or not.
                        *out = TileStats::compute(&unmasked, tile_at(idx), scratch)
                            .unwrap_or_default();
                    },
                );
            return;
        }
        self.fill_bad_tiles();
    }

    /// Give each bad tile the median sky, σ and noise of the good tiles on the nearest square
    /// ring around it that holds any, as SExtractor fills a bad mesh from its neighbours. Read from
    /// the measured tiles only, so a fill never feeds another.
    fn fill_bad_tiles(&mut self) {
        let (tiles_x, tiles_y) = (self.stats.width(), self.stats.height());
        let reach = tiles_x.max(tiles_y);
        let mut skies = Vec::new();
        let mut sigmas = Vec::new();
        let mut noises = Vec::new();
        for idx in 0..self.measured.len() {
            if self.measured[idx] {
                continue;
            }
            let (tx, ty) = (idx % tiles_x, idx / tiles_x);
            for radius in 1..=reach {
                skies.clear();
                sigmas.clear();
                noises.clear();
                let ring = ty.saturating_sub(radius)..=(ty + radius).min(tiles_y - 1);
                for y in ring {
                    for x in tx.saturating_sub(radius)..=(tx + radius).min(tiles_x - 1) {
                        let on_ring = x.abs_diff(tx).max(y.abs_diff(ty)) == radius;
                        if on_ring && self.measured[y * tiles_x + x] {
                            let stats = self.stats[(x, y)];
                            skies.push(stats.sky);
                            sigmas.push(stats.sigma);
                            noises.push(stats.noise);
                        }
                    }
                }
                if !skies.is_empty() {
                    break;
                }
            }
            debug_assert!(!skies.is_empty(), "some tile is measured");
            self.stats[(tx, ty)] = TileStats {
                sky: statistics::median_mut(&mut skies),
                sigma: statistics::median_mut(&mut sigmas),
                noise: statistics::median_mut(&mut noises),
            };
        }
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

        par::iter_mut(dst).enumerate().for_each(|(idx, out)| {
            let tile = Vec2us::new(idx % tiles_x, idx / tiles_x);

            let mut skies = [0.0f32; 9];
            let mut sigmas = [0.0f32; 9];
            let mut noises = [0.0f32; 9];
            let mut count = 0;

            for dy in -1isize..=1 {
                for dx in -1isize..=1 {
                    let x = Reflection::of(tile.x, dx, last.x);
                    let y = Reflection::of(tile.y, dy, last.y);
                    let edge = src[(x.pivot, y.pivot)];
                    let reflected = if x.pivot == x.mirror && y.pivot == y.mirror {
                        edge
                    } else {
                        let inner = src[(x.mirror, y.mirror)];
                        TileStats {
                            sky: 2.0 * edge.sky - inner.sky,
                            sigma: 2.0 * edge.sigma - inner.sigma,
                            noise: 2.0 * edge.noise - inner.noise,
                        }
                    };
                    skies[count] = reflected.sky;
                    sigmas[count] = reflected.sigma;
                    noises[count] = reflected.noise;
                    count += 1;
                }
            }

            out.sky = statistics::median_mut(&mut skies);
            // A steep σ gradient can reflect below zero, which no noise level is.
            out.sigma = statistics::median_mut(&mut sigmas).max(0.0);
            out.noise = statistics::median_mut(&mut noises).max(0.0);
        });

        mem::swap(&mut self.stats, scratch);
    }
}

/// The tile a window offset reads along one axis: `pivot` and `mirror` are both the neighbour
/// itself inside `0..=last`, else the edge tile it reflects through and that tile's inner
/// neighbour, whose difference continues the grid linearly.
#[derive(Debug, Clone, Copy)]
struct Reflection {
    pivot: usize,
    mirror: usize,
}

impl Reflection {
    /// The neighbour `delta` from `idx` on an axis whose last tile is `last` ≥ 1.
    const fn of(idx: usize, delta: isize, last: usize) -> Self {
        let (pivot, mirror) = match idx.checked_add_signed(delta) {
            Some(neighbour) if neighbour <= last => (neighbour, neighbour),
            Some(_) => (last, last - 1),
            None => (0, 1),
        };
        Self { pivot, mirror }
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

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
mod tests;
