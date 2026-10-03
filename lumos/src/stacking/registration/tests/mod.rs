//! Synthetic data tests for registration.
//!
//! - `robustness`: every model on catalogs of known correspondence — noise, outliers, partial
//!   overlap, large rotations and scales, the smallest catalogs
//! - `auto_ladder`: the `Auto` model ladder
//! - `image_registration`: end to end on synthetic images
//! - `warping`: image warping with every transform type and interpolation method
//! - `input`: the star lists registration is handed — too few, degenerate, mismatched
//! - `recovery`: match recovery after an initial transform estimate
//! - `sip_distortion`: a known radial distortion recovered through `register()`
//! - `helpers`: shared fixtures (stars under a transform, a fit's deviation, the seeded `register`)

mod auto_ladder;
mod helpers;
mod image_registration;
mod input;
mod recovery;
mod robustness;
mod sip_distortion;
mod warping;
