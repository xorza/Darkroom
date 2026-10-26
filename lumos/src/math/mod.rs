//! Math utilities.
//!
//! # Modules
//!
//! - [`sum`]: Sum, accumulate, and scale operations
//! - [`statistics`]: Median, MAD, and sigma-clipped statistics
//! - [`wavelet`]: The à trous (starlet) B3-spline transform
//! - [`lanczos`]: The windowed-sinc resampling kernel
//! - [`fwhm`]: FWHM/sigma conversion for Gaussian profiles
//! - [`error_function`]: `erf` and `erfc`, after fdlibm
//! - [`floor_i32`]: `floor` to an `i32`
//! - [`lm_controller`]: Levenberg–Marquardt for small dense least-squares problems
//! - [`lstsq`]: Linear least squares by the SVD, with one rank rule
//! - [`pixel_quadrature`]: Gauss–Legendre quadrature over one pixel
//! - [`pixel_gaussian`]: A Gaussian's mean over a pixel, in closed form
//! - [`plane_moments`]: The least-squares plane through samples at integer offsets, exact
//! - [`dmat3`]: A 3×3 `f64` matrix
//! - [`grid`]: The nodes of a sampling grid over a frame's pixels

pub(crate) mod dmat3;
pub(crate) mod error_function;
pub(crate) mod floor_i32;
pub(crate) mod fwhm;
pub(crate) mod grid;
pub(crate) mod wavelet;

pub(crate) mod lanczos;
pub(crate) mod lm_controller;
pub(crate) mod lstsq;
pub(crate) mod pixel_gaussian;
pub(crate) mod pixel_quadrature;
pub(crate) mod plane_moments;
pub(crate) mod statistics;
pub(crate) mod sum;

#[cfg(feature = "internals")]
pub(crate) mod bench;
