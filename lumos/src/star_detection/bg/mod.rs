//! Background estimation for star detection.
//!
//! Estimates the sky background using a tiled approach with sigma-clipped statistics, then
//! interpolates using the mesh's natural bicubic spline to create a C²-continuous background map
//! (matching SExtractor/SEP).

pub(crate) mod bg_estimate;
pub(crate) mod sky_noise;
pub(crate) mod workspace;

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
mod tests;
