//! Statistical per-pixel frame combination.
//!
//! One engine behind every combine the pipeline runs: [`frame_cache`] gathers the frames into a
//! tier (resident or memory-mapped) and walks the output in row chunks, [`rejection`] decides which
//! samples at a pixel survive, [`normalization`] measures the per-frame gain/offset that puts them
//! on a common scale, and [`stack`] holds the body of the [`stack_config::StackConfig`] entries,
//! which ties the config to those three. Calibration masters and registered light stacks differ
//! only in whether the frames carry frame quality planes; where they do, [`pixel_coverage`] is the
//! single rule deciding which of them reaches a given pixel.

pub(crate) mod error;
pub(crate) mod frame_cache;
pub(crate) mod normalization;
pub(crate) mod pixel_coverage;
pub(crate) mod rejection;
pub(crate) mod stack;
pub(crate) mod stack_config;

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
mod tests;
