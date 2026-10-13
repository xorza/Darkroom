//! End-to-end registered stacking orchestration.
//!
//! [`AlignStackConfig::align_stack_images`] registers, warps, and combines already-calibrated
//! images; [`AlignStackConfig::calibrate_align_stack_paths`] prepends RAW calibration. Both choose
//! a memory tier ([`frame_parking::FrameParking`]) and then run the same body — the difference
//! between an all-RAM run and a memory-bounded one is where a [`pipeline_frame::PipelineFrame`]
//! lives, not which code executes.
//!
//! [`AlignStackConfig::align_stack_images`]: align_stack_config::AlignStackConfig::align_stack_images
//! [`AlignStackConfig::calibrate_align_stack_paths`]: align_stack_config::AlignStackConfig::calibrate_align_stack_paths

pub(crate) mod align_stack_config;
pub(crate) mod align_stack_result;
pub(crate) mod align_stack_run;
pub(crate) mod calibration_notes;
pub(crate) mod detected_frame;
pub(crate) mod detector_pool;
pub(crate) mod error;
pub(crate) mod frame_parking;
pub(crate) mod frame_registrar;
pub(crate) mod frame_registration;
pub(crate) mod light_source;
pub(crate) mod pipeline_frame;
pub(crate) mod registered_set;

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
mod tests;
