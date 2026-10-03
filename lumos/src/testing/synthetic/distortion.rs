//! Radial lens distortion for SIP tests: a grid of reference points, and where a lens with barrel
//! (`k > 0`) or pincushion (`k < 0`) distortion and a linear transform puts each of them.

use glam::DVec2;

use crate::stacking::registration::transform::Transform;

/// `p ↦ T(p + d·k·|d|² + d·k4·|d|⁴)` with `d = p − centre`, sampled on the grid
/// `start, start + step, …, extent` on each axis. An order-3 SIP holds the `k` term exactly; the
/// `k4` term needs order 5.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RadialField {
    pub(crate) centre: DVec2,
    pub(crate) k: f64,
    pub(crate) k4: f64,
    pub(crate) transform: Transform,
    pub(crate) start: usize,
    pub(crate) step: usize,
    pub(crate) extent: usize,
}

/// A field's reference grid and the targets it maps them to, paired by index.
#[derive(Debug, Clone)]
pub(crate) struct RadialPairs {
    pub(crate) reference: Vec<DVec2>,
    pub(crate) target: Vec<DVec2>,
}

impl RadialField {
    /// The cubic field `d·k·|d|²` about `centre` alone, on a 100 px grid over `[0, 1000]²`.
    pub(crate) fn new(centre: DVec2, k: f64) -> Self {
        Self {
            centre,
            k,
            k4: 0.0,
            transform: Transform::identity(),
            start: 0,
            step: 100,
            extent: 1000,
        }
    }

    /// The distortion at `p`, before the transform.
    pub(crate) fn displacement(&self, p: DVec2) -> DVec2 {
        let d = p - self.centre;
        let r2 = d.length_squared();
        d * (self.k * r2 + self.k4 * r2 * r2)
    }

    /// Where the field puts `p`.
    pub(crate) fn image(&self, p: DVec2) -> DVec2 {
        self.transform.apply(p + self.displacement(p))
    }

    pub(crate) fn pairs(&self) -> RadialPairs {
        let mut reference = Vec::new();
        for y in (self.start..=self.extent).step_by(self.step) {
            for x in (self.start..=self.extent).step_by(self.step) {
                reference.push(DVec2::new(x as f64, y as f64));
            }
        }
        let target = reference.iter().map(|&p| self.image(p)).collect();
        RadialPairs { reference, target }
    }
}
