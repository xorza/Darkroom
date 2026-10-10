//! Why a camera RAW file did not decode: what LibRaw refused, and what the file claims that lumos
//! refuses.

use std::fmt;

use libraw_sys as sys;

use crate::io::raw::demosaic::xtrans::xtrans_pattern::XTransPatternError;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

/// Why a camera RAW file did not decode.
///
/// Every variant describes the file or what LibRaw made of it: LibRaw hands the container's values
/// through, so a truncated, corrupt or unusual file can produce any of them.
#[derive(Debug, thiserror::Error)]
pub enum RawError {
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
    /// them: a damaged file whose pixels would enter the stack as measurements.
    #[error("the sensor data is corrupt: LibRaw decoded past values its format cannot hold")]
    CorruptData,
    /// The sensor geometry LibRaw reports leaves no visible area inside the raw one.
    #[error(
        "invalid sensor geometry: a {} x {} visible area at ({}, {}) in a {} x {} raw one",
        active.width, active.height, margin.x, margin.y, raw.width, raw.height
    )]
    Geometry {
        raw: Size2us,
        active: Size2us,
        margin: Vec2us,
    },
    /// The raw buffer's rows are not two bytes per sample: read as such, it would shear.
    #[error("the raw pitch of {pitch} bytes is not two per sample of a {width}-sample row")]
    RawPitch { pitch: usize, width: usize },
    /// LibRaw unpacked no raw buffer to read.
    #[error("LibRaw unpacked no raw image")]
    NoRawImage,
    /// A Fuji SuperCCD lays its photosites out 45° to the rows, which neither demosaic reads.
    #[error("a Fuji SuperCCD's 45° photosite layout is not supported")]
    SuperCcd,
    /// A Phase One IIQ: LibRaw leaves its black in the raw buffer and subtracts it, with its
    /// per-row and per-column corrections, only in its own processing.
    #[error(
        "a Phase One IIQ is not supported: LibRaw subtracts its black only in its own processing"
    )]
    PhaseOne,
    /// A floating-point DNG: LibRaw converts its samples to 16-bit integers on unpack, negatives
    /// clamped to 0 and the rest truncated.
    #[error("a floating-point DNG is not supported: LibRaw quantizes it to 16-bit integers")]
    FloatingPoint,
    /// A header read of a file whose decoder settles the mosaic only as it decodes — a Raspberry
    /// Pi or Nokia sensor dump, a Pentax 4-shot: the header's pattern is not the frame's. The file
    /// loads; its frame has to be decoded to be described.
    #[error("the decoder settles this file's mosaic as it decodes, which a header read cannot")]
    MosaicSetAtDecode,
    /// The file's black-level metadata is unusable.
    #[error(transparent)]
    BlackLevel(#[from] BlackLevelError),
    /// The file's X-Trans layout is not one.
    #[error(transparent)]
    XTrans(#[from] XTransPatternError),
    /// A CFA load of a sensor LibRaw delivers no mosaic for: a linear DNG or sRAW, or an exotic
    /// CFA. Such a file loads as a `LinearImage` through LibRaw's own processing.
    #[error("not a CFA frame: LibRaw delivers this sensor's image already processed")]
    NotACfaFrame,
    /// A sensor of four colours — Sony's RGBE, Nikon's CMYG — that lumos does not demosaic, and
    /// whose colours LibRaw's processing would either hand through as four planes or fold to RGB
    /// through a camera matrix lumos does not check.
    #[error("a four-colour sensor ({colours}) is not supported")]
    FourColourSensor { colours: String },
    /// LibRaw's own processing failed.
    #[error("LibRaw could not process the image: {0}")]
    Process(LibrawCode),
    /// LibRaw's processing produced an image of a shape a `LinearImage` cannot hold.
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

/// Why a file's black-level metadata could not be consolidated into a usable normalization range.
///
/// Every variant describes something the *file* claims, not something the code got wrong: libraw
/// hands these values through verbatim from the container, so a truncated or hand-edited RAW can
/// produce any of them.
#[derive(Debug, thiserror::Error)]
pub enum BlackLevelError {
    /// The spatial black pattern's dimensions do not multiply into a `usize`.
    #[error("invalid spatial black pattern dimensions: {width}x{height}")]
    SpatialPatternOverflow { width: u32, height: u32 },

    /// The spatial black pattern claims more entries than the fixed table libraw reports it in.
    #[error("spatial black pattern {width}x{height} exceeds {capacity} entries")]
    SpatialPatternTooLarge {
        width: u32,
        height: u32,
        capacity: usize,
    },

    /// Black is at or above maximum somewhere, leaving no range to normalize into.
    #[error("invalid black level: black {black} ADU reaches the maximum {maximum}")]
    BlackExceedsMaximum { black: f64, maximum: u32 },
}
