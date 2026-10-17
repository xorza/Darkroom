//! [`DistortionMap`]: a thin-plate spline's distortion sampled on a grid.

use glam::DVec2;

use crate::math::grid;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;
use crate::registration::distortion::tps::ThinPlateSpline;

/// A spline's distortion at every node of a grid, with its largest and mean magnitude.
#[derive(Debug, Clone)]
struct DistortionMap {
    /// The grid's extent, in nodes.
    grid: Size2us,
    /// The distance between nodes, in pixels; the last cell of an axis may be narrower.
    spacing: usize,
    /// The pixel each column of nodes sits on, ending on the image's last column.
    columns: Vec<usize>,
    /// The pixel each row of nodes sits on, ending on the image's last row.
    rows: Vec<usize>,
    /// The distortion at each node, row by row.
    vectors: Vec<DVec2>,
    max_magnitude: f64,
    mean_magnitude: f64,
}

/// The two nodes of an axis around a coordinate, and how far between them it lies.
#[derive(Debug, Clone, Copy)]
struct Cell {
    lower: usize,
    upper: usize,
    fraction: f64,
}

impl DistortionMap {
    /// The distortion `tps` gives at every node of a grid of `spacing` over `image`: every
    /// `spacing` pixels and the last pixel of each axis ([`grid::nodes`]), so the statistics
    /// cover the frame and nothing past it, where a spline grows like `r² ln r`.
    fn from_tps(tps: &ThinPlateSpline, image: Size2us, spacing: usize) -> Self {
        let columns: Vec<usize> = grid::nodes(image.width, spacing).collect();
        let rows: Vec<usize> = grid::nodes(image.height, spacing).collect();
        let grid = Size2us::new(columns.len(), rows.len());

        let mut vectors = Vec::with_capacity(grid.pixel_count());
        let mut max_magnitude = 0.0f64;
        let mut sum_magnitude = 0.0;
        for &y in &rows {
            for &x in &columns {
                let p = DVec2::new(x as f64, y as f64);
                let d = tps.transform(p) - p;
                let magnitude = d.length();
                vectors.push(d);
                max_magnitude = max_magnitude.max(magnitude);
                sum_magnitude += magnitude;
            }
        }
        let mean_magnitude = sum_magnitude / vectors.len() as f64;

        Self {
            grid,
            spacing,
            columns,
            rows,
            vectors,
            max_magnitude,
            mean_magnitude,
        }
    }

    /// The distortion at grid node `point`; `None` past the grid.
    fn get(&self, point: Vec2us) -> Option<DVec2> {
        self.grid
            .contains(point)
            .then(|| self.vectors[self.grid.index_of(point)])
    }

    /// Interpolate the distortion at an arbitrary position: bilinear inside the grid, and the
    /// nearest edge's value past it, never a zero the map did not measure.
    fn interpolate(&self, p: DVec2) -> DVec2 {
        let x = self.cell(&self.columns, p.x);
        let y = self.cell(&self.rows, p.y);
        let at = |x, y| self.vectors[self.grid.index_of(Vec2us::new(x, y))];
        let (v00, v10, v01, v11) = (
            at(x.lower, y.lower),
            at(x.upper, y.lower),
            at(x.lower, y.upper),
            at(x.upper, y.upper),
        );
        (1.0 - x.fraction) * (1.0 - y.fraction) * v00
            + x.fraction * (1.0 - y.fraction) * v10
            + (1.0 - x.fraction) * y.fraction * v01
            + x.fraction * y.fraction * v11
    }

    /// The two nodes of `nodes` around `coordinate`, and how far between them it lies, held to
    /// the grid: both the last node, and 0, past its far end.
    #[expect(
        clippy::cast_sign_loss,
        reason = "the cell index is clamped to the grid, so it is non-negative"
    )]
    fn cell(&self, nodes: &[usize], coordinate: f64) -> Cell {
        let last = nodes.len() - 1;
        if last == 0 {
            return Cell {
                lower: 0,
                upper: 0,
                fraction: 0.0,
            };
        }
        let lower = ((coordinate / self.spacing as f64).floor().max(0.0) as usize).min(last - 1);
        let (from, to) = (nodes[lower] as f64, nodes[lower + 1] as f64);
        let fraction = ((coordinate - from) / (to - from)).clamp(0.0, 1.0);
        Cell {
            lower,
            upper: lower + 1,
            fraction,
        }
    }
}

#[cfg(test)]
mod tests;
