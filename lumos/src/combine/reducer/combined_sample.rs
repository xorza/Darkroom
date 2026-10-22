//! [`CombinedSample`]: one reduced channel sample.

use crate::combine::reducer::pixel_samples::PixelSamples;
use crate::stack_product::PixelQuality;

/// One reduced channel sample: the combined value, how many samples reached it, and — when the
/// caller asked for the quality planes — the survivors' weight, the inverse variance of the value
/// and the survivors' dispersion.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CombinedSample {
    pub(crate) value: f32,
    pub(crate) weight: f32,
    pub(crate) inverse_variance: f32,
    pub(crate) dispersion: f32,
    /// Whether the pixel was combined by the small-sample fallback, too few frames having reached
    /// it for the rejection asked for.
    pub(crate) small_n_fallback: bool,
}

impl CombinedSample {
    /// A weighted mean `value` over the samples of `samples` at `survivors`: the weight is `Σwᵢ`,
    /// and the inverse variance is `(Σwᵢ)² / Σwᵢ²·vᵢ` with each sample's model variance `vᵢ` taken
    /// at the combined value, the estimate of the true signal. Taken at each sample's own value
    /// instead, an upward fluctuation would carry a larger variance and pull the figure down.
    /// Without noise columns, for a reducer whose request has no inverse variance plane, it reads
    /// `+∞`, which nothing reads.
    ///
    /// The dispersion is the variance of the same mean as the samples' scatter shows it, with no
    /// noise model: `Σwᵢ(xᵢ − x̄)² / ((n − 1)·Σwᵢ)`. Where each sample's variance is `c / wᵢ`, the
    /// weighted sum of squares has expectation `(n − 1)·c` and the mean's variance is `c / Σwᵢ`, so
    /// the figure is unbiased; noise weighting makes the weights so at the sky where no warp
    /// averaged and no flat amplified the samples, and equal weights make it the squared standard
    /// error of the mean. NaN for fewer than two survivors, whose scatter says nothing.
    pub(crate) fn from_survivors(
        value: f32,
        samples: &PixelSamples<'_>,
        survivors: impl IntoIterator<Item = usize>,
    ) -> Self {
        let PixelSamples {
            values,
            weights,
            noise,
        } = samples;
        let mut count = 0usize;
        let mut weight = 0.0f32;
        let mut weighted_variance = 0.0f32;
        let mut weighted_squares = 0.0f32;
        for idx in survivors {
            let survivor_weight = weights[idx];
            count += 1;
            weight += survivor_weight;
            let deviation = values[idx] - value;
            weighted_squares += survivor_weight * deviation * deviation;
            if let Some(noise) = noise {
                weighted_variance +=
                    survivor_weight * survivor_weight * noise.variance_at(idx, value);
            }
        }
        Self {
            value,
            weight,
            // `+∞` where the model says the samples hold no noise: an exact pixel, honestly.
            inverse_variance: if weight > 0.0 {
                weight * weight / weighted_variance
            } else {
                PixelQuality::UNMEASURED.inverse_variance
            },
            dispersion: if count > 1 && weight > 0.0 {
                weighted_squares / ((count - 1) as f32 * weight)
            } else {
                PixelQuality::UNMEASURED.dispersion
            },
            small_n_fallback: false,
        }
    }

    /// The median of `weights.len()` samples, which keeps them all: its weight is their `Σwᵢ`. It
    /// is no linear combination, so it has no inverse variance or dispersion to report.
    pub(crate) fn median(value: f32, weights: &[f32]) -> Self {
        Self {
            value,
            weight: weights.iter().sum(),
            inverse_variance: PixelQuality::UNMEASURED.inverse_variance,
            dispersion: PixelQuality::UNMEASURED.dispersion,
            small_n_fallback: false,
        }
    }

    /// A pixel no sample reached: nothing combined, nothing weighed, and no scatter to read.
    pub(crate) const fn uncovered() -> Self {
        let PixelQuality {
            weight,
            inverse_variance,
            dispersion,
            ..
        } = PixelQuality::UNMEASURED;
        Self {
            value: 0.0,
            weight,
            inverse_variance,
            dispersion,
            small_n_fallback: false,
        }
    }

    /// A reduction for a combine that asked for no quality planes: the walk over survivor weights
    /// would produce two numbers nothing reads, and it costs one pass over the frames per pixel.
    pub(crate) const fn value_only(value: f32) -> Self {
        Self {
            value,
            weight: 0.0,
            inverse_variance: 0.0,
            dispersion: 0.0,
            small_n_fallback: false,
        }
    }
}
