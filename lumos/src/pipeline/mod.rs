//! End-to-end registered stacking: lights detected, registered to a reference, warped and
//! combined.
//!
//! Three entries — lights in memory ([`AlignStackConfig::align_stack_images`]), calibrated lights
//! on disk ([`AlignStackConfig::align_stack_paths`]), and RAW lights calibrated on the way
//! ([`AlignStackConfig::calibrate_align_stack_paths`]) — run one of two schedules: every light
//! detected before any registers, or, for lights on disk against a named reference, each light
//! through all of it in one pass. Either chooses a memory tier ([`frame_parking::FrameParking`])
//! first, and the difference between an all-RAM run and a memory-bounded one is where a
//! [`pipeline_frame::PipelineFrame`] lives, not which code executes.
//!
//! [`AlignStackConfig::align_stack_images`]: align_stack_config::AlignStackConfig::align_stack_images
//! [`AlignStackConfig::align_stack_paths`]: align_stack_config::AlignStackConfig::align_stack_paths
//! [`AlignStackConfig::calibrate_align_stack_paths`]: align_stack_config::AlignStackConfig::calibrate_align_stack_paths

pub(crate) mod align_stack_config;
pub(crate) mod align_stack_result;
mod align_stack_run;
mod calibration_notes;
mod detected_frame;
pub(crate) mod error;
mod frame_parking;
mod frame_registrar;
pub(crate) mod frame_registration;
mod light_files;
mod light_shape;
mod light_source;
mod mosaic_preparation;
mod pipeline_frame;
mod registered_set;

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
mod tests;
