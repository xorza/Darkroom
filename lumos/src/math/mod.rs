//! Math utilities.
//!
//! # Modules
//!
//! - [`sum`]: Sum, accumulate, and scale operations
//! - [`statistics`]: Median, MAD, and sigma-clipped statistics
//! - [`noise`]: White-noise estimators that exclude the signal
//! - [`wavelet`]: The à trous (starlet) B3-spline transform
//! - [`lanczos`]: The windowed-sinc resampling kernel
//! - [`fwhm`]: FWHM/sigma conversion for Gaussian profiles
//! - [`linear_system`]: Dense `A·x = b` by Gaussian elimination with partial pivoting
//! - [`dmat3`]: A 3×3 `f64` matrix
//! - [`size2us`], [`vec2us`], [`urect`]: Integer sizes, positions and rectangles

pub(crate) mod dmat3;
pub(crate) mod fwhm;
pub(crate) mod size2us;
pub(crate) mod urect;
pub(crate) mod vec2us;
pub(crate) mod wavelet;

pub(crate) mod lanczos;
pub(crate) mod linear_system;
pub(crate) mod noise;
pub(crate) mod statistics;
pub(crate) mod sum;
