//! Stacking: turn a set of sub-exposures into one calibrated, aligned, combined
//! deep-sky image. The stage and shared support modules are:
//!
//! - [`calibration_masters`] — master dark/flat/bias + defect maps, per-frame calibration.
//! - [`star_detection`] — sub-pixel star detection feeding registration.
//! - [`registration`] — star-pattern alignment + image warp into a common frame.
//! - [`combine`] — statistical per-pixel frame combination (rejection/normalization/weighting).
//! - [`drizzle`] — Fruchter & Hook variable-pixel reconstruction (dithered/super-resolution sets).
//! - [`pipeline`] — end-to-end orchestration (`align_and_stack`, `calibrate_align_stack`).
//! - [`progress`] — progress reporting shared by stacking stages.

pub(crate) mod calibration_masters;
pub(crate) mod combine;
pub(crate) mod drizzle;
pub(crate) mod frame_store;
pub(crate) mod pipeline;
pub(crate) mod product;
pub(crate) mod progress;
pub(crate) mod registration;
pub(crate) mod star_detection;
