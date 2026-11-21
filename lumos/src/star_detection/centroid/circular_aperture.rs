//! [`CircularAperture`]: a circle over a pixel grid, each pixel weighed by its exact area inside.

use common::Libm;
use glam::DVec2;

/// A circle of `radius` pixels about `centre`, over pixels whose centres sit on the integers: the
/// aperture a star's flux and SNR are summed over. A pixel counts by the exact area of its unit
/// square inside the circle, as photutils' exact overlap does, so the sum moves continuously with
/// the radius and the centre, where a whole-pixel stencil jumps as a pixel enters it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct CircularAperture {
    pub(super) centre: DVec2,
    pub(super) radius: f64,
}

impl CircularAperture {
    /// The area of pixel `(x, y)` inside the circle, in `[0, 1]`.
    pub(super) fn overlap(self, x: f64, y: f64) -> f64 {
        let (x0, x1) = (x - 0.5 - self.centre.x, x + 0.5 - self.centre.x);
        let (y0, y1) = (y - 0.5 - self.centre.y, y + 0.5 - self.centre.y);
        let corner = |x, y| self.corner(x, y);
        (corner(x1, y1) - corner(x0, y1) - corner(x1, y0) + corner(x0, y0)).clamp(0.0, 1.0)
    }

    /// The signed area of the circle within the rectangle from the centre to `(x, y)`: the disk's
    /// symmetry about both axes turns any rectangle into four such corners.
    fn corner(self, x: f64, y: f64) -> f64 {
        x.signum() * y.signum() * self.quadrant(x.abs(), y.abs())
    }

    /// The circle's area within `[0, x] × [0, y]`, both non-negative: `∫₀ˣ min(y, √(r² − t²)) dt`.
    fn quadrant(self, x: f64, y: f64) -> f64 {
        let r = self.radius;
        let (x, y) = (x.min(r), y.min(r));
        if x * x + y * y <= r * r {
            return x * y;
        }
        // Past `t₀` the arc lies under `y`; `∫√(r² − t²) dt = (t·√(r² − t²) + r²·asin(t/r))/2`.
        let t0 = (r * r - y * y).max(0.0).sqrt();
        let arc = |t: f64| {
            f64::midpoint(
                t * (r * r - t * t).max(0.0).sqrt(),
                r * r * (t / r).libm_asin(),
            )
        };
        y * t0 + arc(x) - arc(t0)
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::PI;

    use glam::DVec2;

    use crate::star_detection::centroid::circular_aperture::CircularAperture;

    /// The overlaps of the pixels covering a circle sum to its area, `π·r²`, wherever its centre
    /// falls; a pixel wholly inside counts 1 and one wholly outside 0. A circle of radius 0.5 at a
    /// pixel's centre lies within it: its area, π/4.
    #[test]
    fn the_overlaps_sum_to_the_circle() {
        for (centre, radius) in [
            (DVec2::new(0.0, 0.0), 3.0),
            (DVec2::new(0.3, -0.2), 2.71),
            (DVec2::new(0.5, 0.5), 7.02),
            (DVec2::new(-0.45, 0.1), 0.9),
        ] {
            let aperture = CircularAperture { centre, radius };
            let reach = radius.ceil() as i32 + 1;
            let sum: f64 = (-reach..=reach)
                .flat_map(|y| (-reach..=reach).map(move |x| (f64::from(x), f64::from(y))))
                .map(|(x, y)| aperture.overlap(x, y))
                .sum();
            assert!(
                (sum - PI * radius * radius).abs() < 1e-12 * radius * radius,
                "{centre} r {radius}: {sum}"
            );
        }
        let aperture = CircularAperture {
            centre: DVec2::ZERO,
            radius: 3.0,
        };
        assert_eq!(aperture.overlap(0.0, 0.0), 1.0);
        assert_eq!(aperture.overlap(5.0, 0.0), 0.0);
        let small = CircularAperture {
            centre: DVec2::ZERO,
            radius: 0.5,
        };
        assert!((small.overlap(0.0, 0.0) - PI / 4.0).abs() < 1e-15);
    }
}
