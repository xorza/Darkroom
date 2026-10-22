//! FITS: decoding an image HDU into the [`image`](crate::image) types, the extensions lumos writes
//! beside it, and writing them back.

pub(crate) mod calibration_bundle;
pub(crate) mod cfa;
pub(crate) mod decode;
pub(crate) mod error;
pub(crate) mod extension_claims;
pub(crate) mod fits_load_config;
pub(crate) mod flags_extension;
pub(crate) mod gain_extension;
pub(crate) mod linear;
pub(crate) mod lumos_extension;
pub(crate) mod metadata;
pub(crate) mod product;
