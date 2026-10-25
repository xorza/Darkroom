//! What an image carries beside its samples: the header its source stated, the facts the
//! pipeline derived, and what they hold.

pub(crate) mod bg_split;
pub(crate) mod calibration_state;
pub(crate) mod dark_comparison;
pub(crate) mod derived_facts;
pub(crate) mod fits_checksum_provenance;
pub(crate) mod fits_transfer_provenance;
pub(crate) mod image_metadata;
pub(crate) mod image_provenance;
pub(crate) mod mosaic_noise;
pub(crate) mod observation_header;
pub(crate) mod sample_domain;
pub(crate) mod sample_type_record;
pub(crate) mod temperature_difference;
pub(crate) mod temperature_source;
pub(crate) mod unverified_conditions;
