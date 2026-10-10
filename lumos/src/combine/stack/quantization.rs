//! Carrying the sources' quantization uncertainty through the combine.
//!
//! Each input frame may declare the σ of its own digitization; what the stack inherits depends on
//! how its samples were reduced. A weighted mean of independent sources combines them in
//! quadrature, over the frames that survived rejection; a median is not a linear combination, so
//! it gets the order-statistic factor when every source shares one σ and a conservative bound
//! otherwise.
//!
//! Rejection and coverage keep a different set of frames at every pixel, so the figure the master
//! carries is the least-reduced pixel's — seeded from every frame and raised wherever a pixel had
//! fewer, which is what [`MaxSigma`] accumulates.

use std::sync::atomic::{AtomicU32, Ordering};

use crate::combine::normalization::FrameNorm;
use crate::frame_store::frame_stats::FrameStats;

/// Every input frame's declared quantization σ, in frame order.
///
/// Only exists when *every* frame declared a usable one: a set where one source's digitization is
/// unknown has no figure to propagate, so the whole concern is skipped rather than guessed at.
#[derive(Debug)]
pub(super) struct SourceSigmas(Vec<f32>);

impl SourceSigmas {
    /// The declared σ of each frame, or `None` if any frame lacks a usable one.
    pub(super) fn measure<'a>(stats: impl IntoIterator<Item = &'a FrameStats>) -> Option<Self> {
        stats
            .into_iter()
            .map(|stats| {
                stats
                    .quantization_sigma
                    .filter(|sigma| sigma.is_finite() && *sigma > 0.0)
            })
            .collect::<Option<Vec<f32>>>()
            .map(Self)
    }

    /// Frames combine in quadrature under the weights and the gains of `channel` they were actually
    /// combined with, so a weighted mean over the `(frame, weight)` survivors carries
    /// `√Σ(wᵢ·gainᵢ·σᵢ)² / Σwᵢ`.
    pub(super) fn combined_mean(
        &self,
        frame_norms: Option<&[FrameNorm]>,
        channel: usize,
        survivors: impl IntoIterator<Item = (usize, f32)>,
    ) -> Option<f32> {
        let mut total_weight = 0.0f32;
        let mut variance = 0.0f32;
        for (index, weight) in survivors {
            let gain = frame_norms.map_or(1.0, |norms| norms[index].channels[channel].gain);
            total_weight += weight;
            variance += (weight * gain * self.0[index]).powi(2);
        }
        (total_weight > 0.0).then(|| variance.sqrt() / total_weight)
    }

    /// The largest σ any frame contributes in any channel once normalization has scaled it — the
    /// bound to fall back on when the reduction has no fixed linear coefficients to propagate
    /// through.
    pub(super) fn conservative(&self, frame_norms: Option<&[FrameNorm]>) -> Option<f32> {
        self.0
            .iter()
            .enumerate()
            .map(|(index, sigma)| {
                frame_norms.map_or(*sigma, |norms| {
                    norms[index]
                        .channels
                        .iter()
                        .map(|channel| channel.gain.abs() * sigma)
                        .fold(0.0, f32::max)
                })
            })
            .reduce(f32::max)
    }

    /// A median's σ, which only has a closed form when every source shares one and normalization
    /// has not scaled them apart; anything else falls back on [`Self::conservative`].
    pub(super) fn combined_median(&self, frame_norms: Option<&[FrameNorm]>) -> Option<f32> {
        let conservative = self.conservative(frame_norms)?;
        if frame_norms.is_some() {
            return Some(conservative);
        }
        let (&source_sigma, rest) = self.0.split_first()?;
        if rest
            .iter()
            .any(|sigma| sigma.to_bits() != source_sigma.to_bits())
        {
            return Some(conservative);
        }
        let n = self.0.len() as f32;
        let factor = if self.0.len().is_multiple_of(2) {
            (3.0 * n / ((n + 1.0) * (n + 2.0))).sqrt()
        } else {
            (3.0 / (n + 2.0)).sqrt()
        };
        Some(source_sigma * factor)
    }
}

/// The largest σ any pixel ended up with, raised concurrently as the combine runs.
///
/// Held as the float's bit pattern in one `AtomicU32`: for non-negative floats that orders
/// identically to the value, so a `fetch_max` on the bits is a `max` on the σ.
#[derive(Debug)]
pub(super) struct MaxSigma(AtomicU32);

impl MaxSigma {
    /// Seed with the σ every pixel would carry if nothing were rejected — the floor the pixels
    /// that do lose frames then raise.
    pub(super) const fn seeded(sigma: f32) -> Self {
        Self(AtomicU32::new(sigma.to_bits()))
    }

    /// Raise the running maximum.
    ///
    /// The `load` is not redundant with the `fetch_max` that follows it. This runs per *pixel*,
    /// from every worker at once, and only pixels that actually lost frames reach it; the load is
    /// a shared read of the cache line, while `fetch_max` is a read-modify-write that has to take
    /// it exclusive. Guarding means the common case — a pixel whose sigma does not beat the
    /// running maximum — costs a shared read instead of a contended RMW across all cores.
    pub(super) fn record(&self, sigma: Option<f32>) {
        if let Some(sigma) = sigma {
            let bits = sigma.to_bits();
            if bits > self.0.load(Ordering::Relaxed) {
                self.0.fetch_max(bits, Ordering::Relaxed);
            }
        }
    }

    /// The maximum reached, or `None` if nothing was ever recorded.
    pub(super) fn get(&self) -> Option<f32> {
        let bits = self.0.load(Ordering::Relaxed);
        (bits != 0).then(|| f32::from_bits(bits))
    }
}

#[cfg(test)]
mod tests {
    use arrayvec::ArrayVec;

    use crate::combine::normalization::{ChannelNorm, FrameNorm};
    use crate::combine::stack::quantization::{MaxSigma, SourceSigmas};
    use crate::frame_store::capture_conditions::CaptureConditions;
    use crate::frame_store::frame_facts::FrameFacts;
    use crate::frame_store::frame_stats::FrameStats;
    use crate::io::image::unverified_conditions::UnverifiedConditions;
    use crate::math::statistics::{MedianMad, mad_to_sigma};

    fn stats(quantization_sigma: Option<f32>) -> FrameStats {
        FrameStats {
            channels: [MedianMad {
                median: 0.5,
                mad: 0.1,
            }]
            .into_iter()
            .collect(),
            noise: [mad_to_sigma(0.1)].into_iter().collect(),
            sky: [0.5].into_iter().collect(),
            quantization_sigma,
            electrons_per_unit: None,
            facts: FrameFacts {
                domain: None,
                row_order: None,
                cfa_type: None,
                saturation_flagged: false,
                conditions: CaptureConditions::default(),
                unverified_dark: UnverifiedConditions::NONE,
            },
        }
    }

    fn norms(gains: &[f32]) -> Vec<FrameNorm> {
        gains
            .iter()
            .map(|&gain| {
                let mut channels = ArrayVec::new();
                channels.push(ChannelNorm { gain, offset: 0.0 });
                FrameNorm { channels }
            })
            .collect()
    }

    /// A set has sigmas to propagate only when every frame declares a finite, positive one.
    #[test]
    fn source_sigmas_need_every_frame_to_declare_one() {
        let measure = |sigmas: &[Option<f32>]| {
            SourceSigmas::measure(&sigmas.iter().map(|&sigma| stats(sigma)).collect::<Vec<_>>())
                .map(|sigmas| sigmas.0)
        };
        assert_eq!(measure(&[Some(0.25), Some(0.5)]), Some(vec![0.25, 0.5]));
        for unusable in [
            None,
            Some(0.0),
            Some(-0.25),
            Some(f32::NAN),
            Some(f32::INFINITY),
        ] {
            assert_eq!(measure(&[Some(0.25), unusable]), None, "{unusable:?}");
        }
    }

    /// Every σ here is dyadic, so each figure is exact up to its one square root.
    /// - Four equal sources in a mean: √(4·σ²)/4 = σ/2.
    /// - Weights 3/4 and 1/4 on σ 1/2 and 1: √(0.375² + 0.25²) = √0.203125.
    /// - A two-sample median averages both, σ·√(6/12); a three-sample one is the order statistic,
    ///   σ·√(3/5). Unequal sources fall back on the largest σ.
    /// - Normalized, a median takes the largest gain-scaled σ, by magnitude: gains −2 and 1 on σ 1/4
    ///   give 1/2.
    /// - Each channel takes its own gain: two frames of σ 1/4 with gains 1 and 2 in channels 0 and 1
    ///   give √(2·(1/4)²)/2 = √2/8 in channel 0 and √(2·(1/2)²)/2 = √2/4 in channel 1, and the
    ///   conservative bound takes channel 1's 1/2.
    #[test]
    fn combined_sigmas_follow_the_reduction() {
        let sigma = 0.25f32;
        let equal = SourceSigmas(vec![sigma; 4]);
        assert_eq!(
            equal.combined_mean(None, 0, (0..4).map(|frame| (frame, 1.0))),
            Some(sigma / 2.0)
        );

        let unequal = SourceSigmas(vec![0.5, 1.0]);
        assert_eq!(
            unequal.combined_mean(None, 0, [(0, 0.75), (1, 0.25)]),
            Some(0.203_125f32.sqrt())
        );
        assert_eq!(unequal.combined_median(None), Some(1.0));

        assert_eq!(
            SourceSigmas(vec![sigma; 2]).combined_median(None),
            Some(sigma * 0.5f32.sqrt())
        );
        assert_eq!(
            SourceSigmas(vec![sigma; 3]).combined_median(None),
            Some(sigma * (3.0f32 / 5.0).sqrt())
        );

        let scaled = SourceSigmas(vec![sigma; 2]);
        assert_eq!(
            scaled.combined_median(Some(&norms(&[-2.0, 1.0]))),
            Some(0.5)
        );
        assert_eq!(scaled.conservative(Some(&norms(&[-2.0, 1.0]))), Some(0.5));
        // No survivors carry no weight, and so no figure.
        assert_eq!(scaled.combined_mean(None, 0, []), None);

        let two_channels: Vec<FrameNorm> = (0..2)
            .map(|_| FrameNorm {
                channels: [1.0, 2.0]
                    .into_iter()
                    .map(|gain| ChannelNorm { gain, offset: 0.0 })
                    .collect(),
            })
            .collect();
        let pair = SourceSigmas(vec![sigma; 2]);
        assert_eq!(
            pair.combined_mean(Some(&two_channels), 0, [(0, 1.0), (1, 1.0)]),
            Some(2.0f32.sqrt() / 8.0)
        );
        assert_eq!(
            pair.combined_mean(Some(&two_channels), 1, [(0, 1.0), (1, 1.0)]),
            Some(2.0f32.sqrt() / 4.0)
        );
        assert_eq!(pair.conservative(Some(&two_channels)), Some(0.5));
    }

    /// The running maximum of the pixels' σ, ordered by the floats' bits: a smaller σ or none
    /// leaves it, a larger one raises it, and a seed of 0 that nothing raised reads as no figure.
    #[test]
    fn max_sigma_keeps_the_largest_recorded() {
        let max = MaxSigma::seeded(0.25);
        max.record(None);
        max.record(Some(0.125));
        assert_eq!(max.get(), Some(0.25));
        max.record(Some(0.5));
        max.record(Some(0.375));
        assert_eq!(max.get(), Some(0.5));
        assert_eq!(MaxSigma::seeded(0.0).get(), None);
    }
}
