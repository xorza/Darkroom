//! Threshold mask creation, packed 64 pixels to a word.
//!
//! Creates binary masks marking the pixels of a residual above a sigma threshold: the detection
//! plane's candidates, and the sources the sky is refined around.
//!
//! Uses bit-packed storage (`BitBuffer2`) for memory efficiency - each pixel
//! uses 1 bit instead of 1 byte, reducing memory usage by 8x.

pub(super) mod simd;
pub(crate) mod threshold_params;

#[cfg(test)]
mod tests;
