//! Synthetic data tests for registration.
//!
//! - `transform_types`: transform estimation from star position correspondences
//! - `image_registration`: end to end on synthetic images
//! - `warping`: image warping with every transform type and interpolation method
//! - `robustness`: outliers, partial overlap, subpixel accuracy, edge cases
//! - `input`: the star lists registration is handed — too few, degenerate, mismatched
//! - `recovery`: match recovery after an initial transform estimate
//! - `sip_distortion`: a known radial distortion recovered through `register()`
//! - `helpers`: shared fixtures (affine and homography application, the seeded `register`)

mod helpers;
mod image_registration;
mod input;
mod recovery;
mod robustness;
mod sip_distortion;
mod transform_types;
mod warping;
