//! Background estimation for star detection.
//!
//! Estimates the sky background using a tiled approach with sigma-clipped
//! statistics, then interpolates using natural bicubic spline to create a
//! C2-continuous background map (matching SExtractor/SEP).
//!
//! Uses SIMD acceleration when available for statistics computation.

pub(crate) mod background_estimate;
mod simd;
pub(crate) mod sky_noise;
pub(crate) mod workspace;

#[cfg(all(test, feature = "bench"))]
mod bench;
#[cfg(test)]
mod tests;
