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
//! ```rust,ignore
//! use lumos::testing::synthetic::{camera::Camera, observe::render,
//!     scene::{BackgroundField, Scene}, metrics::score_detection};
//! use glam::DVec2;
//!
//! let scene = Scene::random_field(Size2us::new(512, 512), 80, (5.0, 200.0), //!     BackgroundField::Uniform { level: 0.05 }, 16.0, 42);
//! let frame = render(&scene, &Camera::realistic(3.5), &observe::Observation::reference(1));
//! // detect on `frame.image`, then: score_detection(&scene.positions(), &found, 2.0)
//! ```
//!
//! Ready-made fields live in [`fixtures`] (`star_field` / `cluster_field`), used by the
//! benches and detection tests.
//!
//! # Modules
//!
//! Forward model: [`scene`] (true sky), [`camera`] (instrument/sensor + PSF), [`observe`]
//! (render + `FrameTruth`), [`noise`] (physical Poisson + read noise), [`fixtures`]
//! (ready-made field builders), [`metrics`] (graders).
//!
//! Building blocks: [`star_profiles`] (PSF kernels), [`backgrounds`] (background fields),
//! [`artifacts`] (cosmic rays, Bayer pattern), [`transforms`] (star-position transforms for
//! registration), [`patterns`] (warp/interpolation fixtures), [`background_map`]
//! (`BackgroundEstimate` fixtures).

pub(crate) mod artifacts;
pub(crate) mod background_map;
pub(crate) mod backgrounds;
pub(crate) mod camera;
pub(crate) mod fixtures;
/// Eyeball-verification tool, not dead code: `#[ignore]`d generators that render every synthetic
/// combination to PNG. Nothing calls into it — that is the point, it is run by hand. See its
/// module docs for the invocation.
mod gallery;
pub(crate) mod metrics;
pub(crate) mod noise;
pub(crate) mod observe;
pub(crate) mod patterns;
pub(crate) mod scene;
pub(crate) mod star_profiles;
pub(crate) mod transforms;
