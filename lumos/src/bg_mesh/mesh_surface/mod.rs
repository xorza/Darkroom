//! [`MeshSurface`]: one component of a mesh carried to every pixel by the natural bicubic spline
//! through the tile centres, read a row or a pixel at a time rather than kept as a plane.

mod simd;

use std::mem;
use std::ops::RangeInclusive;

use crate::bg_mesh::mesh_axis::MeshAxis;
use crate::bg_mesh::mesh_surface::simd::{InterpolateSegment, SegmentRamp};
use crate::bg_mesh::spline::natural_spline::NaturalSpline;
use crate::bg_mesh::spline::spline_segment::{SplineNode, SplineSegment};
use crate::scratch_pool::{Pooled, Scratch, ScratchPool, Shapeless};
use ::simd::Kernel;
use common::{Size2us, Vec2us};

/// One component of a mesh — its sky or its σ — as the natural bicubic spline through the tile
/// centres carries it to every pixel of the frame: the tiles' values, the y pass's second
/// derivatives down each tile column, the centres, and the range a σ is held to.
///
/// A copy of what [`MeshSpline::solve`](crate::bg_mesh::mesh_spline::MeshSpline::solve) read off
/// the mesh, so the mesh's workspace can measure another plane while this one is read. A row or a
/// pixel is the value the whole plane would hold there, bit for bit.
#[derive(Debug, Default)]
pub(crate) struct MeshSurface {
    /// The component at each tile, row-major.
    pub(super) values: Vec<f32>,
    /// The y pass's second derivative at each tile, row-major.
    pub(super) d2y: Vec<f32>,
    pub(super) centers_x: Vec<f32>,
    pub(super) centers_y: Vec<f32>,
    /// The frame the surface covers.
    pub(super) size: Size2us,
    /// The range a σ is held to: the spline overshoots its nodes, and past the outer tiles it
    /// extrapolates, so on a field whose σ changes fast it reaches below zero. photutils clips its
    /// maps to the mesh's range for that reason. The sky is not clipped: its extrapolation is
    /// what follows a gradient past the outer tiles.
    pub(super) clip: Option<RangeInclusive<f32>>,
    /// The scratch each reader of a row takes, kept at the mesh's tile columns.
    pub(super) rows: ScratchPool<RowScratch>,
}

impl Shapeless for MeshSurface {}

/// One row's spline in x: the tile columns' values at the row, their second derivatives, the
/// solve's scratch, and the row a reader fills.
#[derive(Debug, Default)]
pub(crate) struct RowScratch {
    nodes: Vec<f32>,
    d2x: Vec<f32>,
    solve: Vec<f32>,
    row: Vec<f32>,
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

/// The pixels of one interval between tile centres along a row: from `start`, below `end`, the
/// first and last running on to the row's ends.
#[derive(Debug, Clone, Copy)]
struct Interval {
    tile: usize,
    start: usize,
    end: usize,
}

/// An interval's cubic, and the parameter of each of its pixels.
#[derive(Debug, Clone, Copy)]
struct IntervalCubic {
    segment: SplineSegment,
    ramp: SegmentRamp,
}

impl MeshSurface {
    /// The bytes a surface over a frame of `size` holds, its mesh of `tile_size` tiles, read by
    /// `workers` at once: the tiles' values and second derivatives, the centres, and each reader's
    /// row scratch and row.
    pub(crate) const fn heap_bytes(size: Size2us, tile_size: usize, workers: usize) -> usize {
        let tiles_x = MeshAxis::new(size.width, tile_size).count();
        let tiles_y = MeshAxis::new(size.height, tile_size).count();
        let reader = 3 * tiles_x + size.width;
        (2 * tiles_x * tiles_y + tiles_x + tiles_y + workers * reader) * size_of::<f32>()
    }

    const fn tiles_x(&self) -> usize {
        self.centers_x.len()
    }

    /// The last tile row whose centre is at or before `pos`, or the first when none is.
    fn lower_tile_row(&self, pos: f32) -> usize {
        self.centers_y
            .partition_point(|&centre| centre <= pos)
            .saturating_sub(1)
    }

    /// A reader of the surface with the scratch of one worker.
    pub(crate) fn reader(&self) -> SurfaceReader<'_> {
        SurfaceReader {
            surface: self,
            scratch: self.rows.acquire(),
        }
    }

    /// Into `scratch`, each tile column's y spline at image row `y`, and the x spline's second
    /// derivatives through them.
    fn solve_row(&self, y: usize, scratch: &mut RowScratch) {
        let fy = y as f32;
        let tiles_x = self.tiles_x();
        let tiles_y = self.centers_y.len();
        let ty0 = self.lower_tile_row(fy).min(tiles_y.saturating_sub(2));
        let ty1 = (ty0 + 1).min(tiles_y - 1);
        let cy0 = self.centers_y[ty0];
        let hy = self.centers_y[ty1] - cy0;
        let ty = if ty1 == ty0 { 0.0 } else { (fy - cy0) / hy };
        let nodes = &mut scratch.nodes[..tiles_x];
        for (tx, node) in nodes.iter_mut().enumerate() {
            let at = |ty: usize| SplineNode {
                value: self.values[ty * tiles_x + tx],
                d2: self.d2y[ty * tiles_x + tx],
            };
            *node = SplineSegment::new(at(ty0), at(ty1), hy).eval(ty);
        }
        NaturalSpline {
            values: nodes,
            centers: &self.centers_x,
        }
        .second_derivatives(&mut scratch.d2x[..tiles_x], &mut scratch.solve);
    }

    /// Each interval between tile centres along a row, in order.
    #[expect(clippy::cast_sign_loss, reason = "a tile centre is non-negative")]
    fn intervals(&self) -> impl Iterator<Item = Interval> + '_ {
        let tiles_x = self.tiles_x();
        (0..tiles_x - 1).scan(0, move |start, tile| {
            let end = if tile + 2 < tiles_x {
                (self.centers_x[tile + 1].ceil() as usize).min(self.size.width)
            } else {
                self.size.width
            };
            let interval = Interval {
                tile,
                start: *start,
                end,
            };
            *start = end;
            Some(interval)
        })
    }

    /// The cubic of `interval` through the row `scratch` solved.
    fn cubic(&self, interval: Interval, scratch: &RowScratch) -> IntervalCubic {
        let (tx0, tx1) = (interval.tile, interval.tile + 1);
        let cx0 = self.centers_x[tx0];
        let hx = self.centers_x[tx1] - cx0;
        let inv_hx = 1.0 / hx;
        let node = |tx: usize| SplineNode {
            value: scratch.nodes[tx],
            d2: scratch.d2x[tx],
        };
        IntervalCubic {
            segment: SplineSegment::new(node(tx0), node(tx1), hx),
            ramp: SegmentRamp {
                start: (interval.start as f32 - cx0) * inv_hx,
                step: inv_hx,
            },
        }
    }

    /// Image row `y` into `row`, the frame's width: each tile column's y spline evaluated at the
    /// row, the x spline through those values, and each interval between tile centres run as a
    /// vector kernel. Past the outer tile centres the end interval's cubic continues, on every
    /// side alike, as SEP extrapolates: holding the edge value instead would bend a sky gradient
    /// flat there.
    fn row_into(&self, y: usize, row: &mut [f32], scratch: &mut RowScratch) {
        debug_assert_eq!(row.len(), self.size.width, "a row spans the frame");
        self.solve_row(y, scratch);
        if self.tiles_x() == 1 {
            row.fill(scratch.nodes[0]);
        } else {
            for interval in self.intervals() {
                let IntervalCubic { segment, ramp } = self.cubic(interval, scratch);
                InterpolateSegment {
                    out: &mut row[interval.start..interval.end],
                    segment,
                    ramp,
                }
                .dispatch();
            }
        }
        if let Some(range) = &self.clip {
            for value in row.iter_mut() {
                *value = value.clamp(*range.start(), *range.end());
            }
        }
    }

    /// The value at `pos`, the one [`Self::row_into`] writes there.
    fn at(&self, pos: Vec2us, scratch: &mut RowScratch) -> f32 {
        self.solve_row(pos.y, scratch);
        let value = if self.tiles_x() == 1 {
            scratch.nodes[0]
        } else {
            let interval = self
                .intervals()
                .find(|interval| pos.x < interval.end)
                .expect("the last interval runs to the row's end");
            let IntervalCubic { segment, ramp } = self.cubic(interval, scratch);
            segment.eval(ramp.start + (pos.x - interval.start) as f32 * ramp.step)
        };
        match &self.clip {
            Some(range) => value.clamp(*range.start(), *range.end()),
            None => value,
        }
    }
}

/// A [`MeshSurface`] read a row or a pixel at a time, with the scratch of one worker.
#[derive(Debug)]
pub(crate) struct SurfaceReader<'a> {
    surface: &'a MeshSurface,
    scratch: Pooled<RowScratch>,
}

impl SurfaceReader<'_> {
    /// Image row `y`, in a row the reader keeps.
    pub(crate) fn row(&mut self, y: usize) -> &[f32] {
        let mut row = mem::take(&mut self.scratch.row);
        row.resize(self.surface.size.width, 0.0);
        self.surface.row_into(y, &mut row, &mut self.scratch);
        self.scratch.row = row;
        &self.scratch.row
    }

    /// The value at `pos`.
    pub(crate) fn at(&mut self, pos: Vec2us) -> f32 {
        self.surface.at(pos, &mut self.scratch)
    }
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use crate::bg_mesh::mesh_surface::MeshSurface;
    use crate::scratch_pool::ScratchPool;
    use common::Size2us;

    impl MeshSurface {
        /// A surface of `value` at every pixel of a frame of `size`: a mesh of one tile, whose
        /// rows are its one value.
        pub(crate) fn uniform(value: f32, size: Size2us) -> Self {
            Self {
                values: vec![value],
                d2y: vec![0.0],
                centers_x: vec![(size.width as f32 - 1.0) / 2.0],
                centers_y: vec![(size.height as f32 - 1.0) / 2.0],
                size,
                clip: None,
                rows: ScratchPool::new(1),
            }
        }

        /// The surface read out into a plane.
        #[cfg(test)]
        pub(crate) fn plane(&self) -> common::Buffer2<f32> {
            let mut plane = common::Buffer2::new_default(self.size);
            let mut reader = self.reader();
            for (y, row) in plane.pixels_mut().chunks_mut(self.size.width).enumerate() {
                row.copy_from_slice(reader.row(y));
            }
            plane
        }

        /// Another surface holding what this one holds.
        #[cfg(test)]
        pub(crate) fn duplicate(&self) -> Self {
            Self {
                values: self.values.clone(),
                d2y: self.d2y.clone(),
                centers_x: self.centers_x.clone(),
                centers_y: self.centers_y.clone(),
                size: self.size,
                clip: self.clip.clone(),
                rows: ScratchPool::new(self.centers_x.len()),
            }
        }
    }
}

#[cfg(test)]
mod tests;
