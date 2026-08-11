//! Statistical per-pixel frame combination.
//!
//! One engine behind every combine the pipeline runs: [`cache`] gathers the frames into a tier
//! (resident or memory-mapped) and walks the output in row chunks, [`rejection`] decides which
//! samples at a pixel survive, [`normalization`] measures the per-frame gain/offset that puts them
//! on a common scale, and [`stack`] is the entry point that ties a [`config::StackConfig`] to
//! those three. Calibration masters and registered light stacks differ only in whether the frames
//! carry warp quality planes; where they do, [`pixel_coverage`] is the single rule deciding which
//! of them reaches a given pixel.

/// How many samples a cancellable walk covers between cancel polls.
///
/// Both the validation sweep and the normalization gather chunk their iteration to this rather than
/// testing the token per sample — the divisor was a modulo on every sample of every plane of every
/// frame. The exact figure is not tuned to either: it is a granularity, small enough that a
/// cancelled run stops promptly and large enough that the poll is lost in the work.
pub(crate) const CANCEL_POLL_CHUNK: usize = 16_384;

pub(crate) mod cache;
pub(crate) mod cache_config;
pub(crate) mod config;
pub(crate) mod error;
mod normalization;
pub(crate) mod pixel_coverage;
pub(crate) mod rejection;
pub(crate) mod stack;

#[cfg(all(test, feature = "internals"))]
mod bench;
#[cfg(test)]
mod tests;
