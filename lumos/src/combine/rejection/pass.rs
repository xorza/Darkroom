//! [`Pass`]: what one rejection pass sees, and the [`Narrowing`] it gives back.

use std::ops::Range;

use crate::combine::cache::sample_noise::NoiseColumns;
use crate::combine::rejection::normal_scores::NormalScores;
use crate::combine::rejection::sigma_bounds::SigmaBounds;
use crate::math::statistics::spread::Spread;

/// One pass over a pixel's sorted samples: the window still kept, and the noise the frames were
/// measured to have there.
#[derive(Debug)]
pub(crate) struct Pass<'a> {
    /// All of the pixel's samples, ascending. A method that needs a sample's rank among all of them
    /// reads it from here. A pass with a band for each sample moves the samples it rejects out of
    /// order, so only a method that never makes one may read ranks: the robust scales.
    pub(crate) sorted: &'a [f32],
    /// Each sorted sample's gather position, which indexes `noise`.
    pub(crate) positions: &'a [u32],
    pub(crate) window: Range<usize>,
    /// Counts from 0.
    pub(crate) index: usize,
    pub(crate) min_survivors: usize,
    /// The samples' noise models, by gather position, when the combine gathered them.
    pub(crate) noise: Option<NoiseColumns<'a>>,
}

/// The window a pass keeps, and the centre that the survivor rule measures nearness from when the
/// window holds too few samples.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Proposal {
    pub(crate) window: Range<usize>,
    pub(crate) centre: f32,
}

/// What a pass decides: one band for every sample, which keeps a run of the sorted window, or a
/// band for each sample in units of its own noise model, which can keep any subset of it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Narrowing {
    Window(Proposal),
    PerSample { centre: f32, bounds: SigmaBounds },
}

impl Pass<'_> {
    /// The samples still kept, ascending.
    pub(crate) fn samples(&self) -> &[f32] {
        &self.sorted[self.window.clone()]
    }

    /// Keep the samples within `bounds` of `centre`, in units of `sigma`. On sorted samples the
    /// band is one run, so the result is a narrower window.
    pub(crate) fn keep(&self, bounds: SigmaBounds, centre: f32, sigma: f32) -> Proposal {
        debug_assert!(sigma > 0.0);
        let samples = self.samples();
        let low = centre - bounds.low * sigma;
        let high = centre + bounds.high * sigma;
        let start = self.window.start;
        Proposal {
            window: start + samples.partition_point(|&v| v < low)
                ..start + samples.partition_point(|&v| v <= high),
            centre,
        }
    }

    /// The σ the frames' noise models give the kept samples at `centre`: the root mean square of
    /// each one's model variance there, the photon noise of the signal above the sky included
    /// where the frame states its gain. 0 when the combine gathered no noise.
    ///
    /// The floor under every σ a pass measures: a spread measured from a few samples can fall far
    /// below the noise the frames are known to carry, and an outlier cannot lower the model.
    pub(crate) fn model_sigma(&self, centre: f32) -> f32 {
        let Some(noise) = self.noise else {
            return 0.0;
        };
        let positions = &self.positions[self.window.clone()];
        let variance = positions
            .iter()
            .map(|&position| noise.variance_at(position as usize, centre))
            .sum::<f32>()
            / positions.len() as f32;
        variance.sqrt()
    }

    /// `spread`'s σ, raised to the model σ at its centre and to the resolution there.
    pub(crate) fn floored(&self, spread: Spread) -> f32 {
        spread.floored(self.model_sigma(spread.centre))
    }

    /// The sigma-clip step: a band about the median. The first pass scales it by the MAD σ of every
    /// sample; a later one by [`Self::rank_fit`]'s σ, because the window it measures is what the
    /// earlier passes left, whose tails they cut: its own MAD, scaled as a complete sample, would
    /// shrink at every pass and reject clean samples at several times the Gaussian tail share.
    pub(crate) fn clip_about_median(
        &self,
        bounds: SigmaBounds,
        scores: &mut NormalScores,
    ) -> Proposal {
        let spread = if self.index == 0 {
            Spread::of_sorted(self.samples())
        } else {
            Spread {
                centre: Spread::median_of_sorted(self.samples()),
                sigma: self.rank_fit(scores).sigma,
            }
        };
        self.keep(bounds, spread.centre, self.floored(spread))
    }

    /// The least-squares line through the kept samples against their normal scores among all of
    /// the pixel's samples: the intercept is the centre and the slope the σ. The scores are of the
    /// full count, so the window's ranks place it where it sits in the whole sample, and a window
    /// whose tails a pass cut still measures the σ of the whole. Sums run in f64 about the means,
    /// so neither cancels.
    pub(crate) fn rank_fit(&self, scores: &mut NormalScores) -> Spread {
        debug_assert!(
            self.sorted.is_sorted(),
            "ranks are read only where no per-sample band reordered the samples"
        );
        let scores = &scores.of_count(self.sorted.len())[self.window.clone()];
        let samples = self.samples();
        debug_assert!(samples.len() >= 2);
        let count = samples.len() as f64;
        let score_mean = scores.iter().sum::<f64>() / count;
        let sample_mean = samples.iter().map(|&v| f64::from(v)).sum::<f64>() / count;
        let mut score_squares = 0.0f64;
        let mut products = 0.0f64;
        for (&score, &sample) in scores.iter().zip(samples) {
            let score = score - score_mean;
            score_squares += score * score;
            products += score * (f64::from(sample) - sample_mean);
        }
        let slope = products / score_squares;
        Spread {
            centre: (sample_mean - slope * score_mean) as f32,
            sigma: slope as f32,
        }
    }

    /// The sigma-clip step in units of each sample's own noise model at the median, as IRAF
    /// `imcombine reject=ccdclip` does: a sample is kept while `|x − c| / σᵢ` lies within `bounds`.
    /// The samples' variances differ — a warp's confidence and a frame's gain set them — so one
    /// band would be too loose for the precise samples and too tight for the noisy ones.
    pub(crate) fn clip_by_model(&self, bounds: SigmaBounds) -> Narrowing {
        debug_assert!(
            self.noise.is_some(),
            "a spread-measuring combine gathers the noise models"
        );
        Narrowing::PerSample {
            centre: Spread::median_of_sorted(self.samples()),
            bounds,
        }
    }
}
