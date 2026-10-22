//! [`MeshSpline`]: a mesh's tile statistics carried to every pixel by the natural bicubic spline
//! through the tile centres, as SExtractor and SEP interpolate their background maps.

mod simd;

use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::bg_mesh::BgMesh;
use crate::bg_mesh::mesh_spline::simd::{InterpolateSegment, SegmentRamp};
use crate::bg_mesh::spline::natural_spline::NaturalSpline;
use crate::bg_mesh::spline::spline_segment::{SplineNode, SplineSegment};
use crate::bg_mesh::tile_stats::TileComponent;
use crate::scratch_pool::{Scratch, ScratchPool};
use crate::simd::Kernel;

/// The natural bicubic spline through a mesh's tile centres, evaluated at every pixel: the y
/// pass's second derivatives down each tile column, then per row the columns' values there, their
/// x pass and the segment kernel. Its scratch is kept across frames.
///
/// Each interpolation solves its own y pass from the mesh it is handed, so no derivative can
/// outlive the statistics it was solved from.
#[derive(Debug, Default)]
pub(crate) struct MeshSpline {
    /// The y pass's second derivative at each tile, row-major as the mesh's statistics.
    d2y: Vec<f32>,
    /// One tile column's values, second derivatives and solve scratch.
    col: Vec<f32>,
    col_d2: Vec<f32>,
    col_scratch: Vec<f32>,
    rows: ScratchPool<RowScratch>,
}

/// One row's spline in x: the tile columns' values at the row, their second derivatives, and the
/// solve's scratch.
#[derive(Debug, Default)]
struct RowScratch {
    nodes: Vec<f32>,
    d2x: Vec<f32>,
    solve: Vec<f32>,
}

impl Scratch for RowScratch {
    /// The mesh's tile columns.
    type Shape = usize;

    fn build(tiles_x: usize) -> Self {
        let mut scratch = Self::default();
        scratch.fit(tiles_x);
        scratch
    }

    fn fit(&mut self, tiles_x: usize) {
        self.nodes.resize(tiles_x, 0.0);
        self.d2x.resize(tiles_x, 0.0);
        self.solve.resize(tiles_x.saturating_sub(2), 0.0);
    }

    fn has_shape(&self, tiles_x: usize) -> bool {
        self.nodes.len() == tiles_x
            && self.d2x.len() == tiles_x
            && self.solve.len() == tiles_x.saturating_sub(2)
    }
}

impl MeshSpline {
    /// `component` of `mesh` at every pixel of `out`, a plane of the mesh's frame.
    ///
    /// The σ plane is held to the tiles' range: the spline overshoots its nodes, and past the outer
    /// tiles it extrapolates, so on a field whose σ changes fast it reaches below zero. photutils
    /// clips its maps to the mesh's range for that reason. The sky is not clipped: its
    /// extrapolation is what follows a gradient past the outer tiles.
    pub(crate) fn interpolate(
        &mut self,
        mesh: &BgMesh,
        component: TileComponent,
        out: &mut Buffer2<f32>,
    ) {
        self.solve_y(mesh, component);
        let width = out.width();
        let tiles_x = mesh.stats.width();
        self.rows.reshape(tiles_x);
        let clip = match component {
            TileComponent::Sky => None,
            TileComponent::Sigma => Some(mesh.sigma_range()),
        };
        let col = ColSpline {
            mesh,
            component,
            d2y: &self.d2y,
        };
        let rows = &self.rows;
        out.pixels_mut()
            .par_chunks_mut(width)
            .enumerate()
            .for_each_init(
                || rows.acquire(),
                |scratch, (y, row)| {
                    col.interpolate_row(y, row, scratch);
                    if let Some(range) = &clip {
                        for value in row.iter_mut() {
                            *value = value.clamp(*range.start(), *range.end());
                        }
                    }
                },
            );
    }

    /// Release the scratch, for frames of another layout.
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    /// The y pass: for each tile column, the natural spline's second derivatives at its tile
    /// centres, zero at the ends. A mesh of one tile row has none.
    fn solve_y(&mut self, mesh: &BgMesh, component: TileComponent) {
        let tiles_x = mesh.stats.width();
        let tiles_y = mesh.stats.height();
        self.d2y.clear();
        self.d2y.resize(tiles_x * tiles_y, 0.0);
        if tiles_y < 2 {
            return;
        }
        self.col.resize(tiles_y, 0.0);
        self.col_d2.resize(tiles_y, 0.0);
        self.col_scratch.resize(tiles_y.saturating_sub(2), 0.0);
        for tx in 0..tiles_x {
            for (ty, value) in self.col.iter_mut().enumerate() {
                *value = mesh.stats[(tx, ty)].get(component);
            }
            NaturalSpline {
                values: &self.col,
                centers: &mesh.centers_y,
            }
            .second_derivatives(&mut self.col_d2, &mut self.col_scratch);
            for (ty, &d2) in self.col_d2.iter().enumerate() {
                self.d2y[ty * tiles_x + tx] = d2;
            }
        }
    }
}

/// The y spline of one component through every tile column, as a row reads it.
#[derive(Debug, Clone, Copy)]
struct ColSpline<'a> {
    mesh: &'a BgMesh,
    component: TileComponent,
    d2y: &'a [f32],
}

impl ColSpline<'_> {
    /// Image row `y` of the component into `row`: each tile column's y spline evaluated at the
    /// row, the x spline through those values, and each interval between tile centres run as a
    /// vector kernel. Past the outer tile centres the end interval's cubic continues, on every
    /// side alike, as SEP extrapolates: holding the edge value instead would bend a sky gradient
    /// flat there.
    #[expect(clippy::cast_sign_loss, reason = "a tile centre is non-negative")]
    fn interpolate_row(self, y: usize, row: &mut [f32], scratch: &mut RowScratch) {
        let Self {
            mesh,
            component,
            d2y,
        } = self;
        let fy = y as f32;
        let width = row.len();
        let tiles_x = mesh.stats.width();
        let tiles_y = mesh.stats.height();
        let centers_x = &mesh.centers_x;
        let ty0 = mesh.find_lower_tile_y(fy).min(tiles_y.saturating_sub(2));
        let ty1 = (ty0 + 1).min(tiles_y - 1);
        let cy0 = mesh.centers_y[ty0];
        let cy1 = mesh.centers_y[ty1];
        let hy = cy1 - cy0;
        let ty = if ty1 == ty0 { 0.0 } else { (fy - cy0) / hy };

        let nodes = &mut scratch.nodes[..tiles_x];
        for (tx, node) in nodes.iter_mut().enumerate() {
            let at = |ty: usize| SplineNode {
                value: mesh.stats[(tx, ty)].get(component),
                d2: d2y[ty * tiles_x + tx],
            };
            *node = SplineSegment::new(at(ty0), at(ty1), hy).eval(ty);
        }
        let d2x = &mut scratch.d2x[..tiles_x];
        NaturalSpline {
            values: nodes,
            centers: centers_x,
        }
        .second_derivatives(d2x, &mut scratch.solve);

        if tiles_x == 1 {
            row.fill(nodes[0]);
            return;
        }

        // Each interval takes the pixels below its upper centre; the first and last run on to the
        // row's ends.
        let mut x = 0usize;
        for tx0 in 0..tiles_x - 1 {
            let tx1 = tx0 + 1;
            let segment_end = if tx1 < tiles_x - 1 {
                (centers_x[tx1].ceil() as usize).min(width)
            } else {
                width
            };
            let cx0 = centers_x[tx0];
            let hx = centers_x[tx1] - cx0;
            let inv_hx = 1.0 / hx;
            let node = |tx: usize| SplineNode {
                value: nodes[tx],
                d2: d2x[tx],
            };
            InterpolateSegment {
                out: &mut row[x..segment_end],
                segment: SplineSegment::new(node(tx0), node(tx1), hx),
                ramp: SegmentRamp {
                    start: (x as f32 - cx0) * inv_hx,
                    step: inv_hx,
                },
            }
            .dispatch();
            x = segment_end;
        }
    }
}

#[cfg(test)]
mod tests;
