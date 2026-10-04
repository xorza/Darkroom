//! Tests for registration.
//!
//! - `robustness`: every model on catalogs of known correspondence — noise, outliers, partial
//!   overlap, large rotations and scales, the smallest catalogs
//! - `auto_model`: the `Auto` model choice by GRIC
//! - `image_registration`: end to end on synthetic images
//! - `warping`: image warping with every transform type and interpolation method
//! - `input`: the star lists registration is handed — too few, degenerate, mismatched
//! - `real_data` (feature `real-data`): the bundled dataset's lights detected and registered
//! - `sip_distortion`: a known radial distortion recovered through `register()`
//! - `helpers`: shared fixtures (stars under a transform, a fit's deviation, the seeded `register`)

mod auto_model;
mod helpers;
mod image_registration;
mod input;
#[cfg(feature = "real-data")]
mod real_data;
mod robustness;
mod sip_distortion;
mod warping;
