//! Linear-fit clipping: fit a line through the sorted samples and reject by residual, which
//! tolerates a gradient across the stack that a median-based method reads as spread.

use crate::error::InvalidConfigField;
use crate::math::statistics::{mad_fast, mad_to_sigma, median_fast};
use crate::stacking::combine::rejection::scratch_buffers::ScratchBuffers;
use crate::stacking::combine::rejection::sigma_bounds::SigmaBounds;
use crate::stacking::combine::rejection::{
    begin_rejection, compact_within, validate_max_iterations,
};

/// Configuration for linear fit clipping.
///
/// Fits a linear relationship between each pixel and a reference value,
/// then rejects pixels that deviate significantly from the fit.
/// Works well with images containing sky gradients.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LinearFitClipConfig {
    /// How far either side of the fitted line a value may sit, in sigma.
    pub sigma: SigmaBounds,
    /// Maximum number of iterations.
    pub max_iterations: u32,
}

impl Default for LinearFitClipConfig {
    fn default() -> Self {
        Self {
            sigma: SigmaBounds::symmetric(3.0),
            max_iterations: 3,
        }
    }
}

impl LinearFitClipConfig {
    pub const fn new(sigma_low: f32, sigma_high: f32, max_iterations: u32) -> Self {
        Self {
            sigma: SigmaBounds::asymmetric(sigma_low, sigma_high),
            max_iterations,
        }
    }

    /// Validate the clip thresholds and iteration count.
    pub(super) fn validate(&self) -> Result<(), InvalidConfigField> {
        self.sigma.validate()?;
        validate_max_iterations(self.max_iterations)
    }

    /// Partition values by linear fit clipping, returning the number of survivors.
    ///
    /// First pass uses median + MAD for initial rejection (robust starting point).
    /// Subsequent passes sort survivors, fit a line through `(sorted_index, value)`,
    /// compute mean absolute deviation of residuals as sigma, and reject each pixel
    /// against its own fitted value. Matches PixInsight/Siril linear fit rejection.
    ///
    /// After return, `values[..remaining]` contains surviving values and
    /// `indices[..remaining]` contains their original frame indices.
    pub(super) fn reject(&self, values: &mut [f32], scratch: &mut ScratchBuffers) -> usize {
        if let Some(survivors) = begin_rejection(values, scratch, 4) {
            return survivors;
        }

        let mut len = values.len();

        for iteration in 0..self.max_iterations {
            if len <= 3 {
                break;
            }

            if iteration == 0 {
                // Initial pass: median + MAD sigma clipping (robust starting point)
                scratch.estimate_values.clear();
                scratch.estimate_values.extend_from_slice(&values[..len]);
                let center = median_fast(&mut scratch.estimate_values);
                let mad = mad_fast(&values[..len], center, &mut scratch.estimate_values);
                let sigma = mad_to_sigma(mad);

                if sigma < f32::EPSILON {
                    break;
                }

                // No early break when the seed pass rejects nothing: the linear-fit passes below
                // must still run. A trend-hidden outlier (the case LinearFit targets) sits within
                // sigma·MAD here — it's only exposed after fitting out the trend.
                len = compact_within(
                    values,
                    &mut scratch.indices,
                    len,
                    SigmaBounds::asymmetric(self.sigma.low * sigma, self.sigma.high * sigma),
                    |_| center,
                );
            } else {
                // Subsequent passes: linear fit rejection

                // Sort remaining values with index co-array
                scratch.sort_with_indices(values, len);

                // Fit `y = mean + b·(x − x̄)` through the sorted values, `x` the sorted position.
                // Centred on `x̄ = (n − 1)/2`, the positions sum to zero and their squares to
                // `n(n² − 1)/12` exactly, so the slope is `Σ(x − x̄)·y` over that: no position sums
                // to accumulate and no `n·Σx² − (Σx)²` to cancel. `len > 3` keeps it positive.
                let n = len as f32;
                let count = len as f64;
                let centre = (count - 1.0) / 2.0;
                let (sum_y, sum_xy) = values[..len].iter().enumerate().fold(
                    (0.0f64, 0.0f64),
                    |(sum_y, sum_xy), (i, &v)| {
                        let v = f64::from(v);
                        (sum_y + v, sum_xy + (i as f64 - centre) * v)
                    },
                );
                let b = (sum_xy / (count * (count * count - 1.0) / 12.0)) as f32;
                let mean = (sum_y / count) as f32;
                let centre = centre as f32;

                // Sigma = mean absolute deviation of residuals from fit
                let sigma: f32 = values[..len]
                    .iter()
                    .enumerate()
                    .map(|(i, &v)| (v - (mean + b * (i as f32 - centre))).abs())
                    .sum::<f32>()
                    / n;

                if sigma < f32::EPSILON {
                    break;
                }

                // Reject each pixel against its own fitted value.
                let write_idx = compact_within(
                    values,
                    &mut scratch.indices,
                    len,
                    SigmaBounds::asymmetric(self.sigma.low * sigma, self.sigma.high * sigma),
                    |i| mean + b * (i as f32 - centre),
                );

                if write_idx == len {
                    break;
                }
                len = write_idx;
            }
        }

        len
    }
}
