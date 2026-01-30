//! SIMD-optimized threshold mask creation.
//!
//! Creates binary masks marking pixels above a sigma threshold relative to
//! background and noise estimates. Used by both background estimation
//! (to mask bright objects) and detection (to find star candidates).
//!
//! Uses bit-packed storage (`BitBuffer2`) for memory efficiency - each pixel
//! uses 1 bit instead of 1 byte, reducing memory usage by 8x.

pub mod packed;

#[cfg(test)]
mod bench;

#[cfg(test)]
mod tests;
