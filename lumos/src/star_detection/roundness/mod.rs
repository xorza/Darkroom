//! The DAOFIND roundness metrics.

use serde::{Deserialize, Serialize};

/// The pair of DAOFIND roundness metrics measured for one source, as photutils'
/// `DAOStarFinder` defines them.
///
/// Both are zero for a circular, symmetric source, and they catch different departures from it,
/// which is why they travel together. Both read DAOFIND's cutout about the centre pixel, of radius
/// `max(2, ⌊1.5σ⌋)` for the expected PSF's σ, so a neighbour a few FWHM away does not reach them.
/// Both read the unconvolved samples: DAOFIND takes SROUND from the convolved ones, which read a
/// round star's sub-pixel phase more strongly — up to 0.45 at FWHM 2, against 0.36 here.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Roundness {
    /// GROUND, photutils' `roundness2`: `2·(Hx − Hy)/(Hx + Hy)`, where `Hx` and `Hy` are the
    /// heights of the expected PSF fitted, with a free sky, to the cutout's weighted x and y
    /// marginals. Circular → 0, extended along x → negative, along y → positive.
    pub ground: f32,
    /// SROUND, photutils' `roundness1`: `2·(−Q₁ + Q₂ − Q₃ + Q₄)/Σ|v|` over the four pinwheel
    /// quadrants of the cutout about its centre pixel, which is left out. A shift of the source
    /// adds to two quadrants of opposite sign alike, so it cancels to first order; elongation
    /// along an axis or a diagonal fills two quadrants of one sign.
    pub sround: f32,
}

/// A source's stamp, row-major and square, about its centre pixel.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RoundnessStamp<'a> {
    /// The signal above the sky.
    pub(crate) values: &'a [f64],
    pub(crate) radius: usize,
    /// σ of the expected PSF, the Gaussian GROUND fits; it sets the cutout.
    pub(crate) psf_sigma: f64,
}

/// DAOFIND's cutout reaches 1.5σ of the expected PSF, and two pixels at the least.
const CUTOUT_SIGMAS: f64 = 1.5;
const MIN_CUTOUT_RADIUS: usize = 2;

impl Roundness {
    /// Both metrics of `stamp`'s DAOFIND cutout; `None` when a marginal holds no positive height
    /// above its sky, as DAOFIND rejects such a source.
    pub(crate) fn measure(stamp: RoundnessStamp<'_>) -> Option<Self> {
        let cutout = Cutout::of(stamp);
        let hx = cutout.marginal_height(Axis::X)?;
        let hy = cutout.marginal_height(Axis::Y)?;
        Some(Self {
            ground: (2.0 * (hx - hy) / (hx + hy)) as f32,
            sround: cutout.pinwheel() as f32,
        })
    }
}

/// The axis a marginal runs along.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    X,
    Y,
}

/// The centre `2·radius + 1` square of a stamp.
#[derive(Debug, Clone, Copy)]
struct Cutout<'a> {
    values: &'a [f64],
    stride: usize,
    /// The stamp's row and column of the cutout's first.
    origin: usize,
    radius: usize,
    psf_sigma: f64,
}

impl<'a> Cutout<'a> {
    #[expect(
        clippy::cast_sign_loss,
        reason = "a PSF σ is positive, and photutils truncates its radius the same way"
    )]
    fn of(stamp: RoundnessStamp<'a>) -> Self {
        let stride = 2 * stamp.radius + 1;
        debug_assert_eq!(stamp.values.len(), stride * stride);
        let radius = ((CUTOUT_SIGMAS * stamp.psf_sigma) as usize).max(MIN_CUTOUT_RADIUS);
        debug_assert!(
            radius <= stamp.radius,
            "a stamp of radius {} holds no cutout of {radius}",
            stamp.radius
        );
        Self {
            values: stamp.values,
            stride,
            origin: stamp.radius - radius,
            radius,
            psf_sigma: stamp.psf_sigma,
        }
    }

    const fn at(&self, row: usize, column: usize) -> f64 {
        self.values[(self.origin + row) * self.stride + self.origin + column]
    }

    /// `2·(−Q₁ + Q₂ − Q₃ + Q₄)/Σ|v|`, with the centre pixel left out. Rows and columns from the
    /// cutout's corner, the centre at `c`: Q₁ rows `≤ c`, columns `> c`; Q₂ rows `< c`, columns
    /// `≤ c`; Q₃ rows `≥ c`, columns `< c`; Q₄ rows `> c`, columns `≥ c`. Each takes one half-axis,
    /// so the four tile the cutout.
    fn pinwheel(&self) -> f64 {
        let c = self.radius;
        let size = 2 * c + 1;
        let (mut signed, mut absolute) = (0.0f64, 0.0f64);
        for row in 0..size {
            for column in 0..size {
                if row == c && column == c {
                    continue;
                }
                let value = self.at(row, column);
                let sign = if row <= c && column > c {
                    -1.0
                } else if row < c && column <= c {
                    1.0
                } else if row >= c && column < c {
                    -1.0
                } else {
                    1.0
                };
                signed += sign * value;
                absolute += value.abs();
            }
        }
        if absolute > 0.0 {
            2.0 * signed / absolute
        } else {
            0.0
        }
    }

    /// The height of the expected PSF fitted to the cutout's marginal along `axis`, by DAOFIND's
    /// weighted linear least squares with a free sky: each pixel weighs `c + 1 − |i − c|` along
    /// both axes, a triangle of 1 at the edges, the marginal sums the other axis under its
    /// weights, and the height solves `marginal ≈ sky + h·psf_marginal` with the weights along
    /// `axis`. `None` when the height is not positive.
    fn marginal_height(&self, axis: Axis) -> Option<f64> {
        let c = self.radius;
        let size = 2 * c + 1;
        let triangle = |i: usize| (c + 1 - i.abs_diff(c)) as f64;
        let inv_two_sigma_sq = 1.0 / (2.0 * self.psf_sigma * self.psf_sigma);
        let gaussian = |i: usize| {
            let d = i.abs_diff(c) as f64;
            (-d * d * inv_two_sigma_sq).exp()
        };
        // The PSF is separable, so its weighted marginal is its profile along `axis` times one sum.
        let across: f64 = (0..size).map(|j| gaussian(j) * triangle(j)).sum();
        let (mut weights, mut psf, mut psf_sq, mut data, mut data_psf) =
            (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
        for i in 0..size {
            let marginal: f64 = (0..size)
                .map(|j| {
                    let value = match axis {
                        Axis::X => self.at(j, i),
                        Axis::Y => self.at(i, j),
                    };
                    value * triangle(j)
                })
                .sum();
            let profile = gaussian(i) * across;
            let weight = triangle(i);
            weights += weight;
            psf += weight * profile;
            psf_sq += weight * profile * profile;
            data += weight * marginal;
            data_psf += weight * marginal * profile;
        }
        let numerator = data_psf - data * psf / weights;
        let denominator = psf_sq - psf * psf / weights;
        (numerator > 0.0 && denominator > 0.0).then(|| numerator / denominator)
    }
}

#[cfg(test)]
mod tests;
