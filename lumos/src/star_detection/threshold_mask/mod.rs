//! Threshold mask creation, packed 64 pixels to a word.
//!
//! Creates binary masks marking the pixels of a residual above a sigma threshold: the detection
//! plane's candidates, and the sources the sky is refined around.
//!
//! Bit-packed storage (`BitBuffer2`) takes 1 bit a pixel, 8x less than a byte.

pub(super) mod simd;
pub(crate) mod threshold_params;

#[cfg(test)]
mod tests;
