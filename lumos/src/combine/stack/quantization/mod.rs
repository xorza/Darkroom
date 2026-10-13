//! The quantization σ a stacked master states.
//!
//! A frame's quantization σ is its source's ADC step noise, in its own units, before any flat
//! divided it; calibration leaves it as it is. A master states the step of its inputs: the largest
//! any of them contributes once its normalization scaled it. That is the floor a consumer of the
//! master puts under a noise it measures there — the defect map's residuals among them — and it
//! needs no per-pixel accounting of which frames survived.

use crate::combine::normalization::FrameNorm;
use crate::frame::frame_stats::FrameStats;

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

    /// The step the master states: the largest σ any frame contributes in any slot once
    /// normalization has scaled it.
    pub(super) fn largest(&self, frame_norms: Option<&[FrameNorm]>) -> Option<f32> {
        self.0
            .iter()
            .enumerate()
            .map(|(index, sigma)| {
                frame_norms.map_or(*sigma, |norms| {
                    norms[index]
                        .slots
                        .iter()
                        .map(|slot| slot.gain.abs() * sigma)
                        .fold(0.0, f32::max)
                })
            })
            .reduce(f32::max)
    }
}

#[cfg(test)]
mod tests;
