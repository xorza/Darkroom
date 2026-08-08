//! Background estimation for star detection.
//!
//! Estimates the sky background using a tiled approach with sigma-clipped
//! statistics, then interpolates using natural bicubic spline to create a
//! C2-continuous background map (matching SExtractor/SEP).
//!
//! Uses SIMD acceleration when available for statistics computation.

pub(crate) mod background_estimate;
#[cfg(all(test, feature = "internals"))]
mod bench;
mod simd;
#[cfg(test)]
mod tests;
pub(crate) mod workspace;
