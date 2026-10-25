//! Why a camera RAW file did not decode: what LibRaw refused, and what the file claims that lumos
//! refuses; and why a directory could not be scanned for RAW files.

use std::io;
use std::path::PathBuf;

use libraw::LibrawError;

use crate::error::Cancelled;
use crate::image::error::XTransPatternError;

/// Why a camera RAW file did not decode.
///
/// Every variant describes the file or what LibRaw made of it: LibRaw hands the container's values
/// through, so a truncated, corrupt or unusual file can produce any of them.
#[derive(Debug, thiserror::Error)]
pub enum RawError {
    /// LibRaw refused the file, failed to decode or process it, or reported it in a shape no
    /// reader can take.
    #[error(transparent)]
    Libraw(#[from] LibrawError),
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
    /// A sensor of four colors — Sony's RGBE, Nikon's CMYG — that lumos does not demosaic, and
    /// whose colors LibRaw's processing would either hand through as four planes or fold to RGB
    /// through a camera matrix lumos does not check.
    #[error("a four-color sensor ({colors}) is not supported")]
    FourColorSensor { colors: String },
    /// The load's cancel token stopped the decode.
    #[error("the decode was cancelled")]
    Cancelled,
    /// LibRaw processed the image into a number of colors a `LinearImage` cannot hold.
    #[error("LibRaw processed the image into {colors} colors, where a linear image holds 1 or 3")]
    ProcessedColors { colors: usize },
}

impl From<Cancelled> for RawError {
    fn from(Cancelled: Cancelled) -> Self {
        Self::Cancelled
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

/// Why [`raw_files`](crate::raw_files) could not list a directory's RAW files.
#[derive(Debug, thiserror::Error)]
pub enum RawScanError {
    /// The directory could not be opened.
    #[error("failed to read directory '{}': {source}", path.display())]
    Dir { path: PathBuf, source: io::Error },
    /// An entry of the directory could not be read.
    #[error("failed to read an entry of directory '{}': {source}", path.display())]
    Entry { path: PathBuf, source: io::Error },
    /// An entry named as a RAW file could not be examined: a dangling link, say.
    #[error("failed to read '{}': {source}", path.display())]
    File { path: PathBuf, source: io::Error },
}
