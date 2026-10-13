//! FITS: decoding an image HDU into the [`image`](crate::image) types, the extensions lumos writes
//! beside it, and writing them back.

pub(crate) mod cfa;
pub(crate) mod decode;
pub(crate) mod extension_claims;
pub(crate) mod fits_checksum_provenance;
pub(crate) mod fits_load_options;
pub(crate) mod fits_transfer_provenance;
pub(crate) mod flags_extension;
pub(crate) mod gain_extension;
pub(crate) mod lumos_extension;
pub(crate) mod metadata;
pub(crate) mod sample_type_record;

use std::io::{Error as IoError, ErrorKind};

/// fits-well's error as an `io::Error`, for the writers and the bundle reader, which report in
/// `io::Result`: its own I/O failure as it is, every other as invalid data.
pub(crate) fn to_io(source: fits_well::FitsError) -> IoError {
    match source {
        fits_well::FitsError::Io(source) => source,
        source => IoError::new(ErrorKind::InvalidData, source),
    }
}
