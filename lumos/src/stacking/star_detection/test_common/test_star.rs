//! Gaussian star specification shared by the detection and deblending fixtures.

use crate::math::vec2us::Vec2us;

/// One Gaussian star to render into a synthetic test fixture.
///
/// Replaces a `(usize, usize, f32, f32)` tuple whose four fields were positional: the two
/// coordinates were transposable, and `amplitude` and `sigma` are both `f32` with no unit to
/// tell them apart.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TestStar {
    /// Pixel the profile is centred on.
    pub(crate) center: Vec2us,
    /// Peak value above the fixture's background.
    pub(crate) amplitude: f32,
    /// Gaussian standard deviation, in pixels.
    pub(crate) sigma: f32,
}

impl TestStar {
    pub(crate) const fn new(center: Vec2us, amplitude: f32, sigma: f32) -> Self {
        Self {
            center,
            amplitude,
            sigma,
        }
    }

    /// Radius past which the profile contributes less than the fixtures' 0.001 cutoff.
    pub(crate) fn radius(self) -> i32 {
        (self.sigma * 4.0).ceil() as i32
    }

    /// Profile value at `offset` pixels from the centre.
    pub(crate) fn value_at(self, offset_x: i32, offset_y: i32) -> f32 {
        let radius_squared = (offset_x * offset_x + offset_y * offset_y) as f32;
        self.amplitude * (-radius_squared / (2.0 * self.sigma * self.sigma)).exp()
    }
}
