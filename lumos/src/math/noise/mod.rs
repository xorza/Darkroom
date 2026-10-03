//! Estimators of the white noise in an image, which exclude the signal that a spread measure such
//! as MAD includes.

pub(crate) mod difference_noise;
pub(crate) mod mrs_noise;

#[cfg(test)]
mod tests;
