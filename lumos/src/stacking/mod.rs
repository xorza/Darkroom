//! Stacking: turn a set of sub-exposures into one calibrated, aligned, combined
//! deep-sky image. Each submodule is a stage in that flow:
//!
//! - [`calibration_masters`] — master dark/flat/bias + defect maps, per-frame calibration.
//! - [`star_detection`] — sub-pixel star detection feeding registration.
//! - [`registration`] — star-pattern alignment + image warp into a common frame.
//! - [`combine`] — statistical per-pixel frame combination (rejection/normalization/weighting).
//! - [`drizzle`] — Fruchter & Hook variable-pixel reconstruction (dithered/super-resolution sets).
//! - [`pipeline`] — end-to-end orchestration (`align_and_stack`, `calibrate_align_stack`).

pub(crate) mod calibration_masters;
pub(crate) mod combine;
pub(crate) mod drizzle;
pub(crate) mod pipeline;
pub(crate) mod product;
pub(crate) mod registration;
pub(crate) mod star_detection;
