//! The calibration stage: the masters a session's darks, flats and biases stack into, and what
//! they take from each light — the dark signal, the vignetting, the defects and the cosmic rays.

pub(crate) mod calibration_component;
pub(crate) mod calibration_masters;
pub(crate) mod calibration_outcome;
pub(crate) mod calibration_pass;
pub(crate) mod calibration_set;
pub(crate) mod cosmic_ray;
pub(crate) mod dark_match;
pub(crate) mod dark_scale;
pub(crate) mod defect_map;
pub(crate) mod error;
pub(crate) mod master_cache;
pub(crate) mod master_recipe;
pub(crate) mod master_role;
pub(crate) mod master_stack;
pub(crate) mod master_subtraction;
pub(crate) mod prepared_flat;
pub(crate) mod sensor;
pub(crate) mod subtraction;
pub(crate) mod subtractor;

#[cfg(feature = "internals")]
pub(crate) mod bench;
