//! The colour filter array: the patterns over a sensor's photosites, the single-channel mosaic
//! they leave before demosaicing, which calibration works on, and the demosaic itself.

pub(crate) mod cfa_colour;
pub(crate) mod cfa_frame_info;
pub(crate) mod cfa_image;
pub(crate) mod cfa_lattice;
pub(crate) mod cfa_pattern;
pub(crate) mod cfa_type;
pub(crate) mod colour_raster;
pub(crate) mod demosaic;
pub(crate) mod xtrans_pattern;

/// Standard deviation of uniform error spanning one ADC step: `1 / √12`.
pub(crate) const QUANTIZATION_SIGMA_PER_STEP: f32 = 0.288_675_13;

#[cfg(test)]
mod tests;
