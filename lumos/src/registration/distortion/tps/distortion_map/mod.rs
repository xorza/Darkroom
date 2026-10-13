//! [`DistortionMap`]: a thin-plate spline's distortion sampled on a grid.

use glam::DVec2;

use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;
use crate::registration::distortion::tps::ThinPlateSpline;

/// A spline's distortion at every node of a grid, with its largest and mean magnitude.
#[derive(Debug, Clone)]
struct DistortionMap {
    /// The grid's extent, in nodes.
    grid: Size2us,
    /// The distance between nodes, in pixels.
    spacing: f64,
    /// The distortion at each node, row by row.
    vectors: Vec<DVec2>,
    max_magnitude: f64,
    mean_magnitude: f64,
}

impl DistortionMap {
    /// The distortion `tps` gives at every node of a grid of `grid_spacing` over `image`, its far
    /// edges included.
    #[expect(
        clippy::cast_sign_loss,
        reason = "a size over a positive grid spacing is non-negative"
    )]
    fn from_tps(tps: &ThinPlateSpline, image: Size2us, grid_spacing: f64) -> Self {
        let grid = Size2us::new(
            (image.width as f64 / grid_spacing).ceil() as usize + 1,
            (image.height as f64 / grid_spacing).ceil() as usize + 1,
        );

        let mut vectors = Vec::with_capacity(grid.pixel_count());
        let mut max_magnitude = 0.0f64;
        let mut sum_magnitude = 0.0;

        for gy in 0..grid.height {
            for gx in 0..grid.width {
                let p = DVec2::new(gx as f64 * grid_spacing, gy as f64 * grid_spacing);
                let t = tps.transform(p);
                let d = t - p;
                let magnitude = d.length();

                vectors.push(d);
                max_magnitude = max_magnitude.max(magnitude);
                sum_magnitude += magnitude;
            }
        }

        let mean_magnitude = sum_magnitude / vectors.len() as f64;

        Self {
            grid,
            spacing: grid_spacing,
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
    #[expect(
        clippy::cast_sign_loss,
        reason = "the grid coordinate is clamped to the grid, so it is non-negative"
    )]
    fn interpolate(&self, p: DVec2) -> DVec2 {
        let gx = (p.x / self.spacing).clamp(0.0, (self.grid.width - 1) as f64);
        let gy = (p.y / self.spacing).clamp(0.0, (self.grid.height - 1) as f64);

        let gx0 = gx.floor() as usize;
        let gy0 = gy.floor() as usize;
        let gx1 = (gx0 + 1).min(self.grid.width - 1);
        let gy1 = (gy0 + 1).min(self.grid.height - 1);

        let fx = gx - gx0 as f64;
        let fy = gy - gy0 as f64;

        let at = |x, y| self.vectors[self.grid.index_of(Vec2us::new(x, y))];
        let (v00, v10, v01, v11) = (at(gx0, gy0), at(gx1, gy0), at(gx0, gy1), at(gx1, gy1));

        (1.0 - fx) * (1.0 - fy) * v00
            + fx * (1.0 - fy) * v10
            + (1.0 - fx) * fy * v01
            + fx * fy * v11
    }
}

#[cfg(test)]
mod tests;
