//! Carrying the sources' quantization uncertainty through the combine.
//!
//! Each input frame may declare the σ of its own digitization; what the stack inherits depends on
//! how its samples were reduced. A weighted mean of independent sources combines them in
//! quadrature; a median is not a linear combination, so it gets the order-statistic factor when
//! every source shares one σ and a conservative bound otherwise; and Winsorization, which
//! replaces samples with order statistics, has no fixed coefficient set to propagate through at
//! all.
//!
//! Rejection keeps a different survivor set at every pixel, so the figure the master carries is
//! the least-reduced pixel's — seeded from "nothing rejected" and raised wherever a pixel lost
//! frames, which is what [`MaxSigma`] accumulates.

use std::sync::atomic::{AtomicU32, Ordering};

use crate::stacking::combine::normalization::FrameNorm;
use crate::stacking::frame_store::frame_stats::FrameStats;

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

    /// Frames combine in quadrature under the weights and gains they were actually combined with,
    /// so a weighted mean over `survivor_indices` carries `√Σ(wᵢ·gainᵢ·σᵢ)² / Σwᵢ`.
    pub(super) fn combined_mean(
        &self,
        weights: Option<&[f32]>,
        frame_norms: Option<&[FrameNorm]>,
        survivor_indices: impl IntoIterator<Item = usize>,
    ) -> Option<f32> {
        let mut total_weight = 0.0f32;
        let mut variance = 0.0f32;
        for index in survivor_indices {
            let weight = weights.map_or(1.0, |values| values[index]);
            let gain = frame_norms.map_or(1.0, |norms| norms[index].channels[0].gain);
            total_weight += weight;
            variance += (weight * gain * self.0[index]).powi(2);
        }
        (total_weight > 0.0).then(|| variance.sqrt() / total_weight)
    }

    /// The largest σ any frame contributes once normalization has scaled it — the bound to fall
    /// back on when the reduction has no fixed linear coefficients to propagate through.
    pub(super) fn conservative(&self, frame_norms: Option<&[FrameNorm]>) -> Option<f32> {
        self.0
            .iter()
            .enumerate()
            .map(|(index, sigma)| {
                frame_norms.map_or(*sigma, |norms| norms[index].channels[0].gain.abs() * sigma)
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
    pub(super) fn seeded(sigma: f32) -> Self {
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
pub(crate) mod internals {
    use crate::stacking::combine::stack::quantization::SourceSigmas;

    impl SourceSigmas {
        /// A set built straight from per-frame σ values, so a test can pin the arithmetic without
        /// standing up the frames that would otherwise have declared them.
        pub(crate) fn from_values(sigmas: impl IntoIterator<Item = f32>) -> Self {
            Self(sigmas.into_iter().collect())
        }
    }
}
