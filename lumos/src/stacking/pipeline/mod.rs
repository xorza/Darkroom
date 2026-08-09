//! End-to-end registered stacking orchestration.
//!
//! [`align::align_and_stack`] registers, warps, and combines already-calibrated images;
//! [`calibrate::calibrate_align_stack`] prepends RAW calibration. Both choose a memory tier
//! ([`tier::FrameTier`]) and then run the same body — the difference between an all-RAM run and
//! a memory-bounded one is where a [`frame::PipelineFrame`] lives, not which code executes.

pub(crate) mod align;
pub(crate) mod calibrate;
pub(crate) mod config;
pub(crate) mod detector_pool;
pub(crate) mod frame;
pub(crate) mod result;
pub(crate) mod tier;

#[cfg(all(test, feature = "internals"))]
mod bench;
#[cfg(test)]
mod tests;
