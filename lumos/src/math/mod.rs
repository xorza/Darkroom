//! Math utilities.
//!
//! # Modules
//!
//! - [`sum`]: Sum, accumulate, and scale operations
//! - [`statistics`]: Median, MAD, and sigma-clipped statistics
//! - [`lanczos`]: The windowed-sinc resampling kernel
//! - [`fwhm`]: FWHM/sigma conversion for Gaussian profiles
//! - [`linear_system`]: Dense `A·x = b` by Gaussian elimination with partial pivoting

pub(crate) mod dmat3;
pub(crate) mod fwhm;
pub(crate) mod size2us;
pub(crate) mod urect;
pub(crate) mod vec2us;

pub(crate) mod lanczos;
pub(crate) mod linear_system;
pub(crate) mod statistics;
pub(crate) mod sum;
