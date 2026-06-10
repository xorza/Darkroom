//! Synthetic data generation for testing.
//!
//! Tools for generating synthetic astronomical images and star fields for testing the
//! lumos pipeline. See `SYNTHETIC_TESTING_PLAN.md` for the overall design and roadmap.
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
//! let scene = Scene::random_field(512, 512, 80, (5.0, 200.0),
//!     BackgroundField::Uniform { level: 0.05 }, 16.0, 42);
//! let frame = render(&scene, &Camera::realistic(3.5), &observe::Observation::reference(1));
//! // detect on `frame.image`, then: score_detection(&scene.positions(), &found, 2.0)
//! ```
//!
//! The flat per-frame generator [`star_field`] (`StarFieldConfig` + `generate_star_field`)
//! remains for detection tests that only need pixels + a `GroundTruthStar` list.
//!
//! # Modules
//!
//! Forward model: [`scene`] (true sky), [`camera`] (instrument/sensor + PSF), [`observe`]
//! (render + `FrameTruth`), [`noise`] (physical Poisson + read noise), [`metrics`] (graders).
//!
//! Building blocks: [`star_field`] (flat star-field generator + `*_config` presets),
//! [`star_profiles`] (PSF kernels), [`backgrounds`] (background fields), [`artifacts`]
//! (cosmic rays, Bayer pattern), [`transforms`] (star-position transforms for registration),
//! [`patterns`] (warp/interpolation fixtures), [`stamps`] (detection benchmark fields),
//! [`background_map`] (`BackgroundEstimate` fixtures).

pub mod artifacts;
pub mod background_map;
pub mod backgrounds;
pub mod camera;
pub mod gallery;
pub mod metrics;
pub mod noise;
pub mod observe;
pub mod patterns;
pub mod scene;
pub mod stamps;
pub mod star_field;
pub mod star_profiles;
pub mod transforms;
