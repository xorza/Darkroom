//! [`CombinedSample`]: one reduced channel sample.

use crate::combine::reducer::pixel_samples::PixelSamples;
use crate::combine::reducer::sample_quality::SampleQuality;
use crate::math::statistics;
use crate::math::statistics::median_variance::median_variance_factor;
use crate::stack_product::PixelQuality;

/// One reduced channel sample: the combined value and — when the caller asked for the quality
/// planes — the survivors' weight, the inverse variance of the value and the survivors' dispersion.
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
    ///
    /// The three sums run in f64, as the mean's own do ([`crate::math::sum`]), and round once: an
    /// f32 running sum of a thousand frames carries some 2⁻¹⁴ of relative error into each plane.
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
        let mut weight = 0.0f64;
        let mut weighted_variance = 0.0f64;
        let mut weighted_squares = 0.0f64;
        for idx in survivors {
            let survivor_weight = f64::from(weights[idx]);
            count += 1;
            weight += survivor_weight;
            let deviation = f64::from(values[idx]) - f64::from(value);
            weighted_squares += survivor_weight * deviation * deviation;
            if let Some(noise) = noise {
                weighted_variance +=
                    survivor_weight * survivor_weight * f64::from(noise.variance_at(idx, value));
            }
        }
        Self {
            value,
            weight: weight as f32,
            // `+∞` where the model says the samples hold no noise: an exact pixel, honestly.
            inverse_variance: if weight > 0.0 {
                (weight * weight / weighted_variance) as f32
            } else {
                PixelQuality::UNMEASURED.inverse_variance
            },
            dispersion: if count > 1 && weight > 0.0 {
                (weighted_squares / ((count - 1) as f64 * weight)) as f32
            } else {
                PixelQuality::UNMEASURED.dispersion
            },
            small_n_fallback: false,
        }
    }

    /// The median of `samples`, which keeps them all: its weight is their `Σwᵢ`, as a mean's is.
    /// It is no linear combination, so it has no inverse variance or dispersion to report, and its
    /// weight is no inverse variance of it even where the frame weights are inverse variances.
    pub(crate) fn of_median(samples: PixelSamples<'_>, quality: SampleQuality) -> Self {
        let PixelSamples {
            values, weights, ..
        } = samples;
        // Reorders the values in place, away from their weights: only the weights' sum, which no
        // order changes, is read after it.
        let value = statistics::median_mut(values);
        match quality {
            SampleQuality::Measured => Self {
                value,
                weight: weight_sum(weights),
                inverse_variance: PixelQuality::UNMEASURED.inverse_variance,
                dispersion: PixelQuality::UNMEASURED.dispersion,
                small_n_fallback: false,
            },
            SampleQuality::ValueOnly => Self::value_only(value),
        }
    }

    /// The median of a pixel the small-sample fallback combines, three samples or more of a stack
    /// whose rejection needs more, which keeps them all: its weight is their `Σwᵢ`, as a mean's is;
    /// its inverse variance the equal-weight model's, `n / (f(n)·v̄)`, with `v̄` the samples' mean
    /// model variance at the median and `f(n)` the Gaussian median's small-sample factor
    /// ([`median_variance_factor`]): exact for samples of one variance, an approximation
    /// otherwise. Without noise columns it reads `+∞`, as [`Self::from_survivors`]'s does. Its
    /// dispersion is NaN, as any median's: it is no linear combination to read a scatter of.
    pub(crate) fn of_small_median(samples: PixelSamples<'_>, quality: SampleQuality) -> Self {
        let PixelSamples {
            values,
            weights,
            noise,
        } = samples;
        let count = values.len();
        debug_assert!(count >= 3, "one or two samples take their mean");
        // Reorders the values in place, away from their weights and noise: only sums over every
        // sample, which no order changes, are read after it.
        let value = statistics::median_mut(values);
        if quality == SampleQuality::ValueOnly {
            return Self::value_only(value);
        }
        let variance_sum: f64 = noise.map_or(0.0, |noise| {
            (0..count)
                .map(|idx| f64::from(noise.variance_at(idx, value)))
                .sum()
        });
        let count_f64 = count as f64;
        Self {
            value,
            weight: weight_sum(weights),
            inverse_variance: (count_f64 * count_f64
                / (median_variance_factor(count) * variance_sum))
                as f32,
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

/// `Σwᵢ` in f64, rounded once, as [`CombinedSample::from_survivors`] sums its own.
fn weight_sum(weights: &[f32]) -> f32 {
    weights.iter().map(|&weight| f64::from(weight)).sum::<f64>() as f32
}
