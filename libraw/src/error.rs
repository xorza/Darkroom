//! [`LibrawError`]: what LibRaw refused, or reported in a shape no reader can take.

use std::fmt;

use common::USize2;
use common::glam::USizeVec2;

use crate::sys;

/// Why LibRaw did not open, unpack or process a file.
///
/// Every variant describes the file or what LibRaw made of it: LibRaw hands the container's values
/// through, so a truncated, corrupt or unusual file can produce any of them.
#[derive(Debug, thiserror::Error)]
pub enum LibrawError {
    /// LibRaw could not allocate its state.
    #[error("LibRaw could not initialize")]
    Init,
    /// LibRaw refused to open the file: an unsupported format or camera, or a damaged header.
    #[error("LibRaw could not open the file: {0}")]
    Open(LibrawCode),
    /// LibRaw failed to decode the file's sensor data.
    #[error("LibRaw could not unpack the sensor data: {0}")]
    Unpack(LibrawCode),
    /// LibRaw decoded the sensor data but met values its format cannot hold, and carried on with
    /// them: a damaged file whose pixels would enter a reader as measurements.
    #[error("the sensor data is corrupt: LibRaw decoded past values its format cannot hold")]
    CorruptData,
    /// The sensor geometry LibRaw reports leaves no visible area inside the raw one.
    #[error(
        "invalid sensor geometry: a {active} visible area at ({}, {}) in a {raw} raw one",
        margin.x, margin.y
    )]
    Geometry {
        raw: USize2,
        active: USize2,
        margin: USizeVec2,
    },
    /// LibRaw's own processing failed.
    #[error("LibRaw could not process the image: {0}")]
    Process(LibrawCode),
    /// LibRaw's processing produced an image of no pixels, or not of 16-bit samples.
    #[error("LibRaw processed the image into {width} x {height} x {colors} {bits}-bit samples")]
    ProcessedShape {
        width: usize,
        height: usize,
        colors: usize,
        bits: usize,
    },
    /// LibRaw's processed image holds fewer bytes than its shape needs.
    #[error("LibRaw processed the image into {actual} bytes, not {expected}")]
    ProcessedSize { actual: usize, expected: usize },
}

impl LibrawError {
    /// Whether the load's cancel token stopped LibRaw, at whichever stage it was.
    pub const fn is_cancelled(&self) -> bool {
        matches!(
            self,
            Self::Open(LibrawCode::Cancelled)
                | Self::Unpack(LibrawCode::Cancelled)
                | Self::Process(LibrawCode::Cancelled)
        )
    }
}

/// A LibRaw return code, as its `LibRaw_errors` names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibrawCode {
    Unspecified,
    FileUnsupported,
    NoImage,
    OutOfOrderCall,
    InputClosed,
    NotImplemented,
    OutOfMemory,
    DataError,
    IoError,
    Cancelled,
    BadCrop,
    TooBig,
    MemoryPoolOverflow,
    /// A code `LibRaw_errors` does not name.
    Other(i32),
}

impl LibrawCode {
    /// The case of LibRaw's return code `code`, or `None` for success.
    pub(crate) const fn of(code: i32) -> Option<Self> {
        Some(match code {
            sys::LibRaw_errors_LIBRAW_SUCCESS => return None,
            sys::LibRaw_errors_LIBRAW_UNSPECIFIED_ERROR => Self::Unspecified,
            sys::LibRaw_errors_LIBRAW_FILE_UNSUPPORTED => Self::FileUnsupported,
            sys::LibRaw_errors_LIBRAW_REQUEST_FOR_NONEXISTENT_IMAGE => Self::NoImage,
            sys::LibRaw_errors_LIBRAW_OUT_OF_ORDER_CALL => Self::OutOfOrderCall,
            sys::LibRaw_errors_LIBRAW_INPUT_CLOSED => Self::InputClosed,
            sys::LibRaw_errors_LIBRAW_NOT_IMPLEMENTED => Self::NotImplemented,
            sys::LibRaw_errors_LIBRAW_UNSUFFICIENT_MEMORY => Self::OutOfMemory,
            sys::LibRaw_errors_LIBRAW_DATA_ERROR => Self::DataError,
            sys::LibRaw_errors_LIBRAW_IO_ERROR => Self::IoError,
            sys::LibRaw_errors_LIBRAW_CANCELLED_BY_CALLBACK => Self::Cancelled,
            sys::LibRaw_errors_LIBRAW_BAD_CROP => Self::BadCrop,
            sys::LibRaw_errors_LIBRAW_TOO_BIG => Self::TooBig,
            sys::LibRaw_errors_LIBRAW_MEMPOOL_OVERFLOW => Self::MemoryPoolOverflow,
            other => Self::Other(other),
        })
    }
}

impl fmt::Display for LibrawCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Unspecified => "unspecified error",
            Self::FileUnsupported => "unsupported file format",
            Self::NoImage => "no such image in the file",
            Self::OutOfOrderCall => "call out of order",
            Self::InputClosed => "input closed",
            Self::NotImplemented => "not implemented",
            Self::OutOfMemory => "out of memory",
            Self::DataError => "data error",
            Self::IoError => "read error",
            Self::Cancelled => "cancelled",
            Self::BadCrop => "bad crop",
            Self::TooBig => "image too big",
            Self::MemoryPoolOverflow => "memory pool overflow",
            Self::Other(code) => return write!(f, "LibRaw error {code}"),
        };
        f.write_str(text)
    }
}
