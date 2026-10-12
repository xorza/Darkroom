//! What an image is: the linear image the pipeline works on, the sensor mosaic it starts from, the
//! display preview, and what each carries beside its samples.

pub(crate) mod calibration_state;
pub(crate) mod cfa;
pub(crate) mod error;
pub(crate) mod flag_counts;
pub(crate) mod flat_gain;
pub(crate) mod image_dimensions;
pub(crate) mod image_metadata;
pub(crate) mod image_provenance;
pub(crate) mod linear_image;
pub(crate) mod linear_pixels;
pub(crate) mod pixel_flags;
pub(crate) mod preview_image;
pub(crate) mod quality_flags;
pub(crate) mod sample_domain;
