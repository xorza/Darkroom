//! Math utilities.
//!
//! # Modules
//!
//! - [`sum`]: Sum, accumulate, and scale operations
//! - [`statistics`]: Median, MAD, and sigma-clipped statistics
//! - [`fwhm`]: FWHM/sigma conversion for Gaussian profiles

pub(crate) mod dmat3;
pub(crate) mod fwhm;
pub(crate) mod rect;
pub(crate) mod size2us;
pub(crate) mod vec2us;

pub(crate) mod statistics;
pub(crate) mod sum;
