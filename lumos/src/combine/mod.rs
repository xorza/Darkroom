//! Statistical per-pixel frame combination.
//!
//! One engine behind every combine the pipeline runs: [`frame_cache`] gathers the frames into a
//! tier (resident or memory-mapped) and walks the output in row chunks, reading them through
//! [`frame_read`]; [`rejection`] decides which samples at a pixel survive, over the
//! [`reducer`] interface the gather fills; [`normalization`] measures the per-frame gain/offset
//! that puts them on a common scale; and [`stack`] reduces the cache under the settings
//! [`stack_config::StackConfig`] resolves. Calibration masters and registered light stacks differ
//! only in whether the frames carry frame quality planes; where they do, [`pixel_coverage`] is the
//! single rule deciding which of them reaches a given pixel.

pub(crate) mod error;
pub(crate) mod frame_cache;
pub(crate) mod frame_read;
pub(crate) mod normalization;
pub(crate) mod pixel_coverage;
pub(crate) mod reducer;
pub(crate) mod rejection;
pub(crate) mod stack;
pub(crate) mod stack_config;
pub(crate) mod stack_frame;

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
mod tests;
