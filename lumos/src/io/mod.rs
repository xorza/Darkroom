//! The file formats: FITS, camera RAW and the formats imaginarium reads, decoded into the
//! [`image`](crate::image) types and written back from them.

pub(crate) mod decoder;
pub(crate) mod error;
pub(crate) mod fits;
pub(crate) mod frame_info;
pub(crate) mod input_format;
pub(crate) mod load_ctx;
pub(crate) mod null_policy;
pub(crate) mod raster;
pub(crate) mod raw;

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
mod tests;
