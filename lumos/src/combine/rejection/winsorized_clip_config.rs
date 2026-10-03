//! Winsorized sigma clipping: a Huber estimate of the centre and σ, then a clip about them,
//! repeated until a clip rejects nothing.

use crate::combine::rejection::pass::{Pass, Proposal};
use crate::combine::rejection::sigma_bounds::SigmaBounds;
use crate::error::InvalidConfigField;
use crate::math::statistics::spread::Spread;

/// Configuration for winsorized sigma clipping, after PixInsight's `WinsorizedSigmaClipping`.
///
/// Each pass starts from the median and the floored MAD σ of the samples still kept. It then
/// clamps a copy of them to ±1.5σ about the centre, takes the centre again as the mean of the
/// clamped copy and σ as its corrected standard deviation, and repeats on the clamped copy until σ
/// moves by at most 0.05%. Then it rejects the samples outside the bounds about that centre. Passes repeat until one
/// rejects nothing. The clamped copy is only an estimate: no sample is replaced in the mean.
///
/// The robust start departs from Siril, which starts from the plain standard deviation: with three
/// outliers at 10σ among ten samples that start puts all three inside the clamp, and none is
/// rejected. PixInsight starts from a robust σ too (its Sn).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WinsorizedClipConfig {
    /// How far either side of the robust centre a value may sit, in sigma.
    pub sigma: SigmaBounds,
}

/// Huber's clamp, in σ.
const HUBER_C: f64 = 1.5;
/// `1/√v`, where `v = (2Φ(c) − 1) − 2cφ(c) + 2c²(1 − Φ(c))` is the variance of a unit Gaussian
/// clamped to ±c, at c = 1.5. PixInsight rounds it to 1.134.
const WINSORIZED_CORRECTION: f64 = 1.133_392_655_462_487;
/// The relative change of σ at which the clamp iteration stops.
const CONVERGENCE: f64 = 0.0005;
/// A bound on the clamp iteration, so that no input can hold a pixel in it.
const MAX_CLAMP_STEPS: u32 = 50;

impl Default for WinsorizedClipConfig {
    fn default() -> Self {
        Self {
            sigma: SigmaBounds::symmetric(2.5),
        }
    }
}

impl WinsorizedClipConfig {
    pub const fn new(sigma: f32) -> Self {
        Self {
            sigma: SigmaBounds::symmetric(sigma),
        }
    }

    pub const fn new_asymmetric(sigma_low: f32, sigma_high: f32) -> Self {
        Self {
            sigma: SigmaBounds::asymmetric(sigma_low, sigma_high),
        }
    }

    /// Validate the clip thresholds.
    pub(crate) fn validate(self) -> Result<(), InvalidConfigField> {
        self.sigma.validate()
    }

    pub(crate) fn narrow(self, pass: &Pass<'_>, clamped: &mut Vec<f32>) -> Proposal {
        let estimate = Self::estimate(pass.samples(), pass.background, clamped);
        pass.keep(self.sigma, estimate.centre, estimate.sigma)
    }

    /// The Huber estimate of an ascending window, in `clamped`'s working copy, with σ floored by
    /// [`Spread::floored`] at every step.
    ///
    /// Each step clamps the last step's copy, as Siril and PixInsight do: a sample once cut short
    /// stays cut short when σ grows. Clamping the samples themselves at each step is Huber's
    /// proposal 2, which breaks down here: with three samples at 10σ among ten, the outliers pull
    /// the centre and σ up step by step until the band holds them. The copy stays sorted, since the
    /// clamp is monotonic. Sums run in f64: the mean of samples a few steps apart at 0.5 would lose
    /// those steps in an f32 sum.
    pub(crate) fn estimate(sorted: &[f32], background: f32, clamped: &mut Vec<f32>) -> Spread {
        debug_assert!(sorted.len() >= 2);
        let start = Spread::of_sorted(sorted);
        let mut centre = start.centre;
        let mut sigma = start.floored(background);
        clamped.clear();
        clamped.extend_from_slice(sorted);
        for _ in 0..MAX_CLAMP_STEPS {
            let reach = (HUBER_C * f64::from(sigma)) as f32;
            let low = centre - reach;
            let high = centre + reach;
            for value in clamped.iter_mut() {
                *value = value.clamp(low, high);
            }
            let count = clamped.len() as f64;
            let mean = clamped.iter().map(|&v| f64::from(v)).sum::<f64>() / count;
            let squares = clamped
                .iter()
                .map(|&v| (f64::from(v) - mean).powi(2))
                .sum::<f64>();
            let next = Spread {
                centre: mean as f32,
                sigma: (WINSORIZED_CORRECTION * (squares / (count - 1.0)).sqrt()) as f32,
            };
            centre = next.centre;
            let next_sigma = next.floored(background);
            let converged =
                (f64::from(next_sigma) - f64::from(sigma)).abs() <= f64::from(sigma) * CONVERGENCE;
            sigma = next_sigma;
            if converged {
                break;
            }
        }
        Spread { centre, sigma }
    }
}
