//! Synthetic data generation for testing.
//!
//! Tools for generating synthetic astronomical images and star fields for testing
//! star detection and registration algorithms.
//!
//! # Quick Start
//!
//! ```rust,ignore
//! use lumos::testing::synthetic::star_field::{
//!     StarFieldConfig, generate_star_field, sparse_field_config,
//! };
//!
//! // A preset config (sparse / dense / crowded / faint / elliptical) ...
//! let (pixels, stars) = generate_star_field(&sparse_field_config());
//!
//! // ... or a custom one, overriding only the fields you care about.
//! let config = StarFieldConfig {
//!     width: 1024,
//!     height: 1024,
//!     num_stars: 100,
//!     noise_sigma: 0.02,
//!     ..Default::default()
//! };
//! let (pixels, stars) = generate_star_field(&config);
//! ```
//!
//! # Modules
//!
//! - [`star_field`] - Main star field generator + `StarFieldConfig` and the `*_config` presets
//! - [`star_profiles`] - PSF rendering (Gaussian, Moffat, elliptical, saturated)
//! - [`backgrounds`] - Background generators (uniform, gradient, vignette, nebula)
//! - [`artifacts`] - Artifact generators (cosmic rays, Bayer pattern)
//! - [`transforms`] - Star position transforms for registration testing
//! - [`patterns`] - Simple test patterns (gradients, checkerboard, noise)
//! - [`stamps`] - Star-field stamps for centroid/detection benchmarks
//! - [`background_map`] - BackgroundEstimate generation for detection tests

pub mod artifacts;
pub mod background_map;
pub mod backgrounds;
pub mod patterns;
pub mod stamps;
pub mod star_field;
pub mod star_profiles;
pub mod transforms;
