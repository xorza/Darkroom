//! Estimators of the white noise in an image, which exclude the signal that a spread measure such
//! as MAD includes.

pub(crate) mod background_split;
pub(crate) mod binned_samples;
pub(crate) mod ccd_noise;
pub(crate) mod difference_noise;
pub(crate) mod gain_bins;
pub(crate) mod mosaic_noise;
pub(crate) mod mosaic_sampling;
pub(crate) mod mrs_noise;

#[cfg(feature = "internals")]
pub(crate) mod bench;

#[cfg(test)]
mod tests;
