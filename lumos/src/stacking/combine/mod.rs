//! Statistical per-pixel frame combination.
//!
//! One engine behind every combine the pipeline runs: [`cache`] gathers the frames into a tier
//! (resident or memory-mapped) and walks the output in row chunks, [`rejection`] decides which
//! samples at a pixel survive, [`normalization`] measures the per-frame gain/offset that puts them
//! on a common scale, and [`stack`] is the entry point that ties a [`config::StackConfig`] to
//! those three. Calibration masters and registered light stacks differ only in whether the frames
//! carry warp quality planes; where they do, [`pixel_coverage`] is the single rule deciding which
//! of them reaches a given pixel.

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
