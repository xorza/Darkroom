//! Estimators of the white noise in an image, which exclude the signal that a spread measure such
//! as MAD includes.

pub(crate) mod background_split;
pub(crate) mod binned_samples;
pub(crate) mod ccd_noise;
pub(crate) mod mosaic_noise_measure;
pub(crate) mod mosaic_sampling;
pub(crate) mod mrs_noise;

/// The standard deviation of a uniform error one ADC step wide, in steps: `1/√12`, rounded to f32.
pub(crate) const QUANTIZATION_SIGMA_PER_STEP: f32 = 0.288_675_13;

#[cfg(feature = "internals")]
pub(crate) mod bench;

#[cfg(test)]
mod tests;
