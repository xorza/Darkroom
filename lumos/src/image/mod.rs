//! What an image is: the linear image the pipeline works on, the sensor mosaic it starts from, the
//! display preview, and what each carries beside its samples.

pub(crate) mod cfa;
pub(crate) mod error;
pub(crate) mod flags;
pub(crate) mod flat_gain;
pub mod frame_desc;
pub(crate) mod intensity;
pub(crate) mod linear_image;
pub(crate) mod masked_plane;
pub(crate) mod metadata;
pub(crate) mod preview_image;
pub(crate) mod rgb;
pub(crate) mod stackable_image;

/// Samples per parallel work item over a plane. Parallelizing per sample drowns a cheap per-pixel
/// op in rayon's recursive split/join overhead (it dominated SCNR); a coarse block amortizes that
/// while staying load-balanced and letting the inner loop auto-vectorize.
pub(crate) const SAMPLES_PER_BLOCK: usize = 8192;

#[cfg(feature = "internals")]
pub(crate) mod bench;
