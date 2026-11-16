//! Synthetic data generation for testing.
//!
//! Tools for generating synthetic astronomical images and star fields for testing the
//! lumos pipeline.
//!
//! # Forward model (preferred)
//!
//! The [`scene`] / [`camera`] / [`observe`] modules form a **forward model**: build a true
//! [`Scene`](scene::Scene), render it through a [`Camera`](camera::Camera) and an
//! [`Observation`](observe::Observation), and grade a lumos stage's output against the
//! captured [`FrameTruth`](observe::FrameTruth) with [`metrics`]. A noiseless
//! [`Camera::ideal`](camera::Camera::ideal) collapses the render to its own ground truth.
//!
//! The test `the_forward_model_renders_a_scene_to_grade_against`, beside [`observe`], walks it end
//! to end: a scattered scene rendered through a realistic camera, its frame and truth read back.
//!
//! Ready-made fields live in [`fixtures`] (`star_field` / `cluster_field`), used by the
//! detection tests.
//!
//! # Modules
//!
//! Forward model: [`scene`] (true sky), [`camera`] (instrument/sensor + PSF), [`observe`]
//! (render + `FrameTruth`), [`noise`] (physical Poisson + read noise), [`fixtures`]
//! (ready-made field builders), [`metrics`] (graders).
//!
//! Building blocks: [`star_profiles`] (PSF kernels), [`bgs`] (background fields),
//! [`artifacts`] (cosmic rays, Bayer pattern), [`transforms`] (random star fields for
//! registration), [`distortion`] (radial lens fields for SIP), [`patterns`] (warp/interpolation
//! fixtures), [`bg_map`] (`BgEstimate` fixtures).

pub(crate) mod artifacts;
pub(crate) mod bg_map;
pub(crate) mod bgs;
pub(crate) mod camera;
pub(crate) mod distortion;
pub(crate) mod fixtures;
/// Eyeball-verification tool, not dead code: `#[ignore]`d generators that render every synthetic
/// combination to PNG. Nothing calls into it — that is the point, it is run by hand. See its
/// module docs for the invocation.
#[cfg(test)]
mod gallery;
pub(crate) mod metrics;
pub(crate) mod noise;
pub(crate) mod observe;
pub(crate) mod patterns;
pub(crate) mod scene;
pub(crate) mod sky_field;
pub(crate) mod star_profiles;
pub(crate) mod transforms;
