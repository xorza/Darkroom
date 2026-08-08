//! Linear-fit clipping: fit a line through the sorted samples and reject by residual, which
//! tolerates a gradient across the stack that a median-based method reads as spread.

use crate::error::InvalidConfigField;
use crate::math::statistics::{mad_f32_fast, mad_to_sigma, median_f32_fast};
use crate::stacking::combine::cache::ScratchBuffers;
use crate::stacking::combine::rejection::{
    compact_within, reset_indices, sort_with_indices, validate_sigma_bounds,
};

/// Configuration for linear fit clipping.
///
/// Fits a linear relationship between each pixel and a reference value,
/// then rejects pixels that deviate significantly from the fit.
/// Works well with images containing sky gradients.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LinearFitClipConfig {
    /// Sigma threshold for low outliers (below the fit).
    pub sigma_low: f32,
    /// Sigma threshold for high outliers (above the fit).
    pub sigma_high: f32,
    /// Maximum number of iterations.
    pub max_iterations: u32,
}

impl Default for LinearFitClipConfig {
    fn default() -> Self {
        Self {
            sigma_low: 3.0,
            sigma_high: 3.0,
            max_iterations: 3,
        }
    }
}

impl LinearFitClipConfig {
    pub fn new(sigma_low: f32, sigma_high: f32, max_iterations: u32) -> Self {
        Self {
            sigma_low,
            sigma_high,
            max_iterations,
        }
    }

    /// Validate the clip thresholds and iteration count.
    pub fn validate(&self) -> Result<(), InvalidConfigField> {
        validate_sigma_bounds(self.sigma_low, self.sigma_high)?;
        InvalidConfigField::check(
            self.max_iterations >= 1,
            "max_iterations",
            "at least 1",
            self.max_iterations as f64,
        )
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
        debug_assert!(!values.is_empty());

        reset_indices(&mut scratch.indices, values.len());

        if values.len() <= 3 {
            return values.len();
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
                let center = median_f32_fast(&mut scratch.estimate_values);
                let mad = mad_f32_fast(&values[..len], center, &mut scratch.estimate_values);
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
                    self.sigma_low * sigma,
                    self.sigma_high * sigma,
                    |_| center,
                );
            } else {
                // Subsequent passes: linear fit rejection

                // Sort remaining values with index co-array
                sort_with_indices(values, scratch, len);

                // Fit line y = a + b*x through sorted values, x = sorted position
                let n = len as f32;
                let mut sum_x = 0.0f32;
                let mut sum_y = 0.0f32;
                let mut sum_xy = 0.0f32;
                let mut sum_xx = 0.0f32;

                for (i, &v) in values[..len].iter().enumerate() {
                    let x = i as f32;
                    sum_x += x;
                    sum_y += v;
                    sum_xy += x * v;
                    sum_xx += x * x;
                }

                let denom = n * sum_xx - sum_x * sum_x;
                if denom.abs() < f32::EPSILON {
                    break;
                }

                let b = (n * sum_xy - sum_x * sum_y) / denom;
                let a = (sum_y - b * sum_x) / n;

                // Sigma = mean absolute deviation of residuals from fit
                let sigma: f32 = values[..len]
                    .iter()
                    .enumerate()
                    .map(|(i, &v)| (v - (a + b * i as f32)).abs())
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
                    self.sigma_low * sigma,
                    self.sigma_high * sigma,
                    |i| a + b * i as f32,
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
