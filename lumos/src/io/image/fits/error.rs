use std::io::{Error as IoError, ErrorKind};

/// fits-well's error as an `io::Error`, for the writers and the bundle reader, which report in
/// `io::Result`: its own I/O failure as it is, every other as invalid data.
pub(crate) fn fits_to_io(source: fits_well::FitsError) -> IoError {
    match source {
        fits_well::FitsError::Io(source) => source,
        source => IoError::new(ErrorKind::InvalidData, source),
    }
}
