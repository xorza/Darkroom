//! Synthetic data tests for registration.
//!
//! - `transform_types`: Tests transform estimation from star position correspondences
//! - `image_registration`: End-to-end tests with actual synthetic images
//! - `warping`: Tests image warping with all transform types and interpolation methods
//! - `robustness`: Tests for outliers, partial overlap, subpixel accuracy, edge cases
//! - `helpers`: Shared test utilities (affine/homography application)

mod helpers;
mod image_registration;
mod robustness;
mod transform_types;
mod warping;
