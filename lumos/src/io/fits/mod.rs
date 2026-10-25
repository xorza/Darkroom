//! FITS: decoding an image HDU into the [`image`](crate::image) types, the extensions lumos writes
//! beside it, and writing them back.

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
pub(crate) mod plane_hdu;
pub(crate) mod tile_parallel;

/// The bytes of samples a FITS read or write holds at once beside the planes it reads or writes:
/// past it the I/O dominates, and below any frame worth chunking.
pub(crate) const CHUNK_BYTES: usize = 4 * 1024 * 1024;
