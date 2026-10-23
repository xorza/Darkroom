//! What an image is: the linear image the pipeline works on, the sensor mosaic it starts from, the
//! display preview, and what each carries beside its samples.

pub(crate) mod bg_split;
pub(crate) mod calibration_state;
pub(crate) mod cfa;
pub(crate) mod channel_layout;
pub(crate) mod dark_comparison;
pub(crate) mod derived_facts;
pub(crate) mod error;
pub(crate) mod fits_checksum_provenance;
pub(crate) mod fits_transfer_provenance;
pub(crate) mod flag_counts;
pub(crate) mod flat_gain;
pub(crate) mod image_dimensions;
pub(crate) mod image_metadata;
pub(crate) mod image_provenance;
pub(crate) mod intensity;
pub(crate) mod linear_image;
pub(crate) mod linear_pixels;
pub(crate) mod masked_plane;
pub(crate) mod mosaic_noise;
pub(crate) mod observation_header;
pub(crate) mod pixel_flags;
pub(crate) mod preview_image;
pub(crate) mod quality_flags;
pub(crate) mod quality_map;
pub(crate) mod rgb;
pub(crate) mod sample_domain;
pub(crate) mod sample_type_record;
pub(crate) mod stackable_image;
pub(crate) mod temperature_difference;
pub(crate) mod temperature_source;
pub(crate) mod unverified_conditions;

#[cfg(feature = "internals")]
pub(crate) mod bench;
