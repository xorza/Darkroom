//! The color filter array: the patterns over a sensor's photosites, the single-channel mosaic
//! they leave before demosaicing, which calibration works on, and the demosaic itself.

pub(crate) mod bayer_phase;
pub(crate) mod cfa_color;
pub(crate) mod cfa_image;
pub(crate) mod cfa_lattice;
pub(crate) mod cfa_pattern;
pub(crate) mod cfa_type;
pub(crate) mod color_raster;
pub(crate) mod demosaic;
pub(crate) mod xtrans_pattern;

#[cfg(test)]
mod tests;
