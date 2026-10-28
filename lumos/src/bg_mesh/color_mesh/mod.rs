//! [`ColorMesh`]: the local sky and σ of each color of a mosaic, between tile centres.

use arrayvec::ArrayVec;
use common::Buffer2;

use crate::bg_mesh::mesh_workspace::{
    DEFAULT_SIGMA_CLIP_MAX_ITERATIONS, MedianFilter, MeshParams, MeshWorkspace,
};
use crate::bg_mesh::tile_stats::{ColorSites, TileStats};
use crate::image::cfa::cfa_type::CfaType;
use crate::image::masked_plane::MaskedPlane;
use glam::USizeVec2;

/// A tile mesh per color of a mosaic, one for a mono plane, read bilinearly between the tile
/// centres.
///
/// Bilinear rather than the bicubic [`MeshSurface`](crate::bg_mesh::mesh_surface::MeshSurface)
/// star detection reads: its readers take one pixel at a time in no row order — a hot pixel's cut,
/// a cosmic-ray candidate's noise — where the spline solves a whole row of tiles for each, and a
/// local level for a k-σ cut needs no continuous slope.
///
/// Each color's tiles measure that color's photosites alone, so a flat-fielded frame whose colors
/// sit at different levels gives each its own sky, and a color's σ is not inflated by the others'
/// offsets. The 3×3 median over the tiles keeps a tile that a cluster of hot pixels or a bright
/// object spoiled from reaching its neighbours.
#[derive(Debug)]
pub(crate) struct ColorMesh {
    colors: ArrayVec<Buffer2<TileStats>, 3>,
    x: Vec<Span>,
    y: Vec<Span>,
}

/// The background at one pixel of one color: the sky, and the noise about the tiles' planes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LocalBg {
    pub(crate) sky: f32,
    pub(crate) noise: f32,
}

/// Where one coordinate falls between two tile centres.
#[derive(Debug, Clone, Copy)]
struct Span {
    lower: usize,
    upper: usize,
    fraction: f32,
}

impl ColorMesh {
    /// The mesh of `plane` under `cfa`, in tiles of `tile_size`, leaving out the pixels its mask
    /// names.
    pub(crate) fn measure(plane: MaskedPlane<'_>, cfa: &CfaType, tile_size: usize) -> Self {
        let MaskedPlane {
            pixels,
            mask: excluded,
        } = plane;
        let workspace = &mut MeshWorkspace::default();
        let params = MeshParams {
            tile_size,
            sigma_clip_max_iterations: DEFAULT_SIGMA_CLIP_MAX_ITERATIONS,
            median_filter: MedianFilter::Apply,
        };
        let mut colors = ArrayVec::new();
        let mut x = Vec::new();
        let mut y = Vec::new();
        for color in 0..cfa.color_count() {
            let mosaic = cfa.mosaic().map(|mosaic| ColorSites { mosaic, color });
            let grid = workspace.measure(pixels, excluded, mosaic, params);
            colors.push(grid.stats.clone());
            // Every color's grid has the one layout of the frame.
            if x.is_empty() {
                x = spans(pixels.width(), &grid.centers_x);
                y = spans(pixels.height(), &grid.centers_y);
            }
        }
        Self { colors, x, y }
    }

    /// The sky and noise of `color` at `pos`, bilinear between the four nearest tile centres,
    /// and continued linearly past the outer ones.
    pub(crate) fn at(&self, color: usize, pos: USizeVec2) -> LocalBg {
        let tiles = &self.colors[color];
        let x = self.x[pos.x];
        let y = self.y[pos.y];
        let corner = |tx: usize, ty: usize| tiles[(tx, ty)];
        let lerp = |a: f32, b: f32, t: f32| a + t * (b - a);
        let mix = |get: fn(TileStats) -> f32| {
            let top = lerp(
                get(corner(x.lower, y.lower)),
                get(corner(x.upper, y.lower)),
                x.fraction,
            );
            let bottom = lerp(
                get(corner(x.lower, y.upper)),
                get(corner(x.upper, y.upper)),
                x.fraction,
            );
            lerp(top, bottom, y.fraction)
        };
        LocalBg {
            sky: mix(|tile| tile.sky),
            // The linear continuation can carry σ below zero past a steep edge, which no noise is.
            noise: mix(|tile| tile.noise).max(0.0),
        }
    }
}

/// For every coordinate of an axis of `length`, the tile centres it falls between.
fn spans(length: usize, centres: &[f32]) -> Vec<Span> {
    if centres.len() == 1 {
        return vec![
            Span {
                lower: 0,
                upper: 0,
                fraction: 0.0,
            };
            length
        ];
    }
    (0..length)
        .map(|pos| {
            let pos = pos as f32;
            let upper = centres
                .partition_point(|&centre| centre <= pos)
                .clamp(1, centres.len() - 1);
            let lower = upper - 1;
            Span {
                lower,
                upper,
                fraction: (pos - centres[lower]) / (centres[upper] - centres[lower]),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;
