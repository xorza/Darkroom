//! The quantization σ a stacked master states.
//!
//! A frame's quantization σ is its source's ADC step noise, in its own units, before any flat
//! divided it; calibration leaves it as it is. A master states the step of its inputs: the largest
//! any of them contributes once its normalization scaled it. That is the floor a consumer of the
//! master puts under a noise it measures there — the defect map's residuals among them — and it
//! needs no per-pixel accounting of which frames survived.

use crate::combine::normalization::FrameNorm;
use crate::frame::frame_stats::FrameStats;

/// The step a master states: the largest σ any frame contributes in any slot once `frame_norms`
/// scaled it; `None` unless *every* frame of `stats` declares a usable one — a set where one
/// source's digitization is unknown has no figure to propagate, so the whole concern is skipped
/// rather than guessed at.
pub(super) fn largest_step<'a>(
    stats: impl IntoIterator<Item = &'a FrameStats>,
    frame_norms: Option<&[FrameNorm]>,
) -> Option<f32> {
    let mut largest = None;
    for (index, stats) in stats.into_iter().enumerate() {
        let sigma = stats
            .quantization_sigma
            .filter(|sigma| sigma.is_finite() && *sigma > 0.0)?;
        let scaled = frame_norms.map_or(sigma, |norms| {
            norms[index]
                .slots
                .iter()
                .map(|slot| slot.gain.abs() * sigma)
                .fold(0.0, f32::max)
        });
        largest = Some(largest.map_or(scaled, |largest: f32| largest.max(scaled)));
    }
    largest
}

#[cfg(test)]
mod tests;
