//! What a per-pixel reducer reads and writes: the samples gathered at a pixel, with their weights
//! and noise, and the one combined sample it reduces them to. Below both the gather engine, which
//! fills and stores them, and the rejection kernels, which read them.

pub(crate) mod combined_sample;
pub(crate) mod pixel_samples;
pub(crate) mod sample_quality;
