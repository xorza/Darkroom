use std::fmt::{self, Display, Formatter};
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};

use fits_well::image::{Bitpix, Scaling};
use fits_well::io::ChecksumReport;
use imaginarium::FileFormat;
use thiserror::Error;

use crate::image::flat_gain::{self, GainGrid};
use crate::image::image_dimensions::ImageDimensions;
use crate::io::fits::cfa::{CFA_FITS_FORMAT, CFA_FITS_VERSION};
use crate::io::fits::lumos_extension::LumosExtension;
use crate::io::fits::metadata::domain_keywords;
use crate::io::raw::error::{LibrawCode, RawError};
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

/// Errors that can occur when loading an astronomical image from disk.
#[derive(Debug, Error)]
pub enum ImageError {
    #[error("Image load cancelled for '{path}'")]
    Cancelled { path: PathBuf },

    #[error("Failed to load FITS file '{path}': {source}")]
    Fits {
        path: PathBuf,
        source: fits_well::FitsError,
    },

    #[error("Unsupported FITS file '{path}': {reason}")]
    FitsUnsupported { path: PathBuf, reason: FitsRefusal },

    #[error("Failed to load image '{path}': {source}")]
    Image {
        path: PathBuf,
        source: imaginarium::Error,
    },

    #[error("Failed to load raw file '{path}': {source}")]
    Raw { path: PathBuf, source: RawError },

    #[error("Failed to read file '{path}': {source}")]
    Io { path: PathBuf, source: io::Error },

    #[error("No decoder reads the extension of '{path}'")]
    UnsupportedFormat { path: PathBuf },

    #[error("Scientific image input '{path}' was rejected: {reason}")]
    ScientificInputRejected {
        path: PathBuf,
        reason: ScientificRefusal,
    },

    #[error("Failed to save image: {source}")]
    Save { source: imaginarium::Error },
}

impl ImageError {
    /// LibRaw's or the decoder's `source` failure reading `path`: [`Self::Cancelled`] where LibRaw
    /// stopped because the load's cancel token asked it to.
    pub(crate) fn raw(path: &Path, source: RawError) -> Self {
        match source {
            RawError::Open(LibrawCode::Cancelled)
            | RawError::Unpack(LibrawCode::Cancelled)
            | RawError::Process(LibrawCode::Cancelled) => Self::cancelled(path),
            source => Self::Raw {
                path: path.to_path_buf(),
                source,
            },
        }
    }

    /// The load of `path` stopped by its cancel token.
    pub(crate) fn cancelled(path: &Path) -> Self {
        Self::Cancelled {
            path: path.to_path_buf(),
        }
    }

    /// fits-well's `source` failure reading `path`.
    pub(crate) fn fits(path: &Path, source: fits_well::FitsError) -> Self {
        Self::Fits {
            path: path.to_path_buf(),
            source,
        }
    }

    /// A FITS file at `path` this decoder refuses, for `reason`.
    pub(crate) fn fits_unsupported(path: &Path, reason: FitsRefusal) -> Self {
        Self::FitsUnsupported {
            path: path.to_path_buf(),
            reason,
        }
    }

    /// An image at `path` refused as scientific input, for `reason`.
    pub(crate) fn scientific_rejection(path: &Path, reason: ScientificRefusal) -> Self {
        Self::ScientificInputRejected {
            path: path.to_path_buf(),
            reason,
        }
    }
}

/// Why a FITS file is refused: one case per refusal a caller can tell apart.
#[derive(Debug, Clone, PartialEq)]
pub enum FitsRefusal {
    /// [`FitsHduSelector::Index`](crate::FitsHduSelector::Index) past the file's `hdus`.
    HduOutOfRange {
        index: usize,
        hdus: usize,
    },
    /// No HDU holds an image for [`FitsHduSelector::Auto`](crate::FitsHduSelector::Auto).
    NoImageHdu,
    /// [`FitsHduSelector::Auto`](crate::FitsHduSelector::Auto) found more than one image HDU.
    SeveralImageHdus {
        images: usize,
    },
    NoMatchingHdu {
        extname: String,
        extver: Option<i64>,
    },
    SeveralMatchingHdus {
        extname: String,
        extver: Option<i64>,
        matches: usize,
    },
    NotAnImage {
        hdu: usize,
    },
    /// A checksum the file holds disagrees with the data.
    InvalidChecksum {
        hdu: usize,
        report: ChecksumReport,
    },
    /// [`FitsChecksumPolicy::RequireValid`](crate::FitsChecksumPolicy::RequireValid) met a
    /// checksum that is not valid.
    ChecksumRequired {
        hdu: usize,
        report: ChecksumReport,
    },
    ZeroAxis {
        shape: Vec<usize>,
    },
    /// A three-plane cube read under
    /// [`FitsCubeInterpretation::Reject`](crate::FitsCubeInterpretation::Reject).
    CubeNeedsRgb,
    /// A cube's `NAXIS3`, which is neither one plane nor three.
    PlaneCount {
        planes: usize,
    },
    AxisCount {
        axes: usize,
    },
    SideTooLong {
        width: usize,
        height: usize,
    },
    SampleCountOverflow {
        shape: Vec<usize>,
    },
    SizeOverflow {
        allocation: FitsAllocation,
    },
    /// `allocation` needs `required` bytes, past the load's memory limit of `limit`.
    OverBudget {
        allocation: FitsAllocation,
        required: u64,
        limit: u64,
    },
    /// The scale a Lumos writer recorded, which is not finite and positive.
    RecordedScale {
        scale: f64,
    },
    /// [`FitsFloatScale::FullScale`](crate::FitsFloatScale::FullScale) that is not finite and
    /// positive.
    DeclaredFullScale {
        scale: f32,
    },
    /// A zero or non-finite `BSCALE`.
    UnusableBscale {
        bscale: f64,
    },
    /// A `BSCALE` whose span of `bitpix` samples overflows.
    BscaleOverflow {
        bscale: f64,
        bitpix: Bitpix,
    },
    /// Floating-point samples reach `maximum`, past a normalized image's, with no scale declared.
    UndeclaredFloatScale {
        maximum: f32,
    },
    /// Nulls under [`FitsNullPolicy::Reject`](crate::FitsNullPolicy::Reject): `count` samples,
    /// the first at `first_index` of the channel-major sample space.
    NullSamples {
        count: usize,
        first_index: usize,
    },
    /// A section of `rows` of `channel` read as `samples`, not the `expected` count.
    SectionLength {
        channel: usize,
        rows: Range<usize>,
        samples: usize,
        expected: usize,
    },
    /// A CFA image of other than one plane.
    CfaPlanes {
        planes: usize,
    },
    MissingCfaPattern,
    /// A Lumos CFA file whose HDU `hdu` is not a Lumos CFA image.
    NotLumosCfa {
        hdu: usize,
    },
    /// A Lumos file of another format, `format`, read as a CFA image.
    NotStandaloneCfa {
        format: String,
    },
    CfaVersion {
        version: Option<i64>,
    },
    /// An extension of another `LUMOSFMT` or `LUMOSVER` than this reader's.
    ExtensionFormat {
        extension: LumosExtension,
        format: Option<String>,
        version: Option<i64>,
    },
    /// An extension that is not an uncompressed image.
    ExtensionNotImage {
        extension: LumosExtension,
    },
    /// An extension of another sample type or scaling than its format's.
    ExtensionSamples {
        extension: LumosExtension,
        bitpix: Bitpix,
        scaling: Scaling,
    },
    /// An extension whose `shape` does not fit its `image`.
    ExtensionShape {
        extension: LumosExtension,
        shape: Vec<usize>,
        image: Size2us,
    },
    /// An extension at HDU `hdu` that names no image it is for.
    ClaimsNoImage {
        extension: LumosExtension,
        hdu: usize,
    },
    /// An extension at HDU `hdu` for an `image` the file does not hold.
    ClaimsMissingImage {
        extension: LumosExtension,
        hdu: usize,
        image: String,
    },
    /// Two extensions of one kind for one `image`.
    DuplicateClaim {
        extension: LumosExtension,
        image: String,
    },
    /// A gain extension's node spacing, `step`, which is not this reader's.
    GainStep {
        step: Option<i64>,
    },
    /// A gain extension's `node` holding `value`, which is not a finite gain above zero.
    NotAGain {
        node: usize,
        value: f32,
    },
    /// A gain extension of `grids` grids for an image of `channels` colours or channels.
    GainChannels {
        grids: usize,
        channels: usize,
    },
    /// A flags byte at `pixel` holding a bit no flag has.
    UnknownFlagBits {
        pixel: Vec2us,
        byte: u8,
    },
    /// A flags `NO_DATA` at `pixel` that the image's NaN does not state, or a NaN it does not
    /// flag.
    NoDataDisagrees {
        pixel: Vec2us,
    },
}

impl Display for FitsRefusal {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::HduOutOfRange { index, hdus } => {
                write!(f, "HDU index {index} is out of range for {hdus} HDUs")
            }
            Self::NoImageHdu => write!(f, "no image HDU found"),
            Self::SeveralImageHdus { images } => write!(
                f,
                "FITS file contains {images} image HDUs; select one explicitly by index or \
                 EXTNAME/EXTVER"
            ),
            Self::NoMatchingHdu {
                extname,
                extver: Some(extver),
            } => write!(f, "no HDU matches EXTNAME={extname:?}, EXTVER={extver}"),
            Self::NoMatchingHdu {
                extname,
                extver: None,
            } => write!(f, "no HDU matches EXTNAME={extname:?}"),
            Self::SeveralMatchingHdus {
                extname,
                extver: Some(extver),
                matches,
            } => write!(
                f,
                "{matches} HDUs match EXTNAME={extname:?}, EXTVER={extver}"
            ),
            Self::SeveralMatchingHdus {
                extname,
                extver: None,
                matches,
            } => write!(
                f,
                "{matches} HDUs match EXTNAME={extname:?}; specify EXTVER"
            ),
            Self::NotAnImage { hdu } => write!(f, "selected HDU {hdu} is not an image"),
            Self::InvalidChecksum { hdu, report } => write!(
                f,
                "HDU {hdu} has an invalid FITS checksum: DATASUM={:?}, CHECKSUM={:?}",
                report.datasum, report.checksum
            ),
            Self::ChecksumRequired { hdu, report } => write!(
                f,
                "HDU {hdu} requires valid DATASUM and CHECKSUM: DATASUM={:?}, CHECKSUM={:?}",
                report.datasum, report.checksum
            ),
            Self::ZeroAxis { shape } => {
                write!(f, "FITS image axes must be nonzero, got {shape:?}")
            }
            Self::CubeNeedsRgb => write!(
                f,
                "three-plane FITS cube requires FitsCubeInterpretation::Rgb"
            ),
            Self::PlaneCount { planes } => {
                write!(f, "Unsupported channel count (NAXIS3): {planes}")
            }
            Self::AxisCount { axes } => write!(f, "Unsupported number of dimensions: {axes}"),
            Self::SideTooLong { width, height } => write!(
                f,
                "FITS image {width}x{height} has a side past {} px",
                ImageDimensions::MAX_SIDE
            ),
            Self::SampleCountOverflow { shape } => {
                write!(f, "FITS sample count overflows: {shape:?}")
            }
            Self::SizeOverflow { allocation } => write!(f, "FITS {allocation} size overflows"),
            Self::OverBudget {
                allocation,
                required,
                limit,
            } => write!(
                f,
                "{allocation} requires {required} bytes, exceeding the FITS load budget of \
                 {limit} bytes"
            ),
            Self::RecordedScale { scale } => write!(
                f,
                "{} {scale} must be finite and positive",
                domain_keywords::SAMPLE_SCALE
            ),
            Self::DeclaredFullScale { scale } => write!(
                f,
                "declared floating-point full scale {scale} must be positive"
            ),
            Self::UnusableBscale { bscale } => write!(
                f,
                "BSCALE {bscale} leaves no scale to normalize integer samples by"
            ),
            Self::BscaleOverflow { bscale, bitpix } => write!(
                f,
                "BSCALE {bscale} overflows the normalization scale for {bitpix:?} samples"
            ),
            Self::UndeclaredFloatScale { maximum } => write!(
                f,
                "floating-point samples reach {maximum}, but nothing declares their scale; give \
                 FitsFloatScale::FullScale, or a DATAMAX"
            ),
            Self::NullSamples { count, first_index } => write!(
                f,
                "image contains {count} null/non-finite samples; first at linear index \
                 {first_index}"
            ),
            Self::SectionLength {
                channel,
                rows,
                samples,
                expected,
            } => write!(
                f,
                "channel {channel} rows {}..{} contain {samples} pixels; expected {expected}",
                rows.start, rows.end
            ),
            Self::CfaPlanes { .. } => {
                write!(f, "scientific CFA input must have exactly one image plane")
            }
            Self::MissingCfaPattern => write!(
                f,
                "scientific CFA FITS input is missing validated CFA pattern metadata"
            ),
            Self::NotLumosCfa { hdu } => {
                write!(f, "HDU {hdu} is not a Lumos {CFA_FITS_FORMAT} image")
            }
            Self::NotStandaloneCfa { format } => write!(
                f,
                "Lumos {format} FITS is not a standalone {CFA_FITS_FORMAT} image"
            ),
            Self::CfaVersion { version } => write!(
                f,
                "unsupported Lumos CFA FITS version {}; expected {CFA_FITS_VERSION}",
                Stated(version.as_ref())
            ),
            Self::ExtensionFormat {
                extension,
                format,
                version,
            } => write!(
                f,
                "{extension}: format {} version {}; expected {} version {}",
                Stated(format.as_ref()),
                Stated(version.as_ref()),
                extension.format(),
                extension.version()
            ),
            Self::ExtensionNotImage { extension } => {
                write!(f, "{extension}: not an uncompressed image extension")
            }
            Self::ExtensionSamples { extension, .. } => {
                let samples = match extension {
                    LumosExtension::Flags => "BITPIX = 8 bytes",
                    LumosExtension::Gain => "BITPIX = -32 floats",
                };
                write!(f, "{extension}: not unscaled {samples}")
            }
            Self::ExtensionShape {
                extension: LumosExtension::Flags,
                shape,
                image,
            } => write!(
                f,
                "{}: shape {shape:?} is not the image's {}x{}",
                LumosExtension::Flags,
                image.width,
                image.height
            ),
            Self::ExtensionShape {
                extension: LumosExtension::Gain,
                shape,
                image,
            } => {
                let grid = GainGrid::of(*image);
                write!(
                    f,
                    "{}: shape {shape:?} is not one or three {}x{} grids over the image's {}x{}",
                    LumosExtension::Gain,
                    grid.columns,
                    grid.rows,
                    image.width,
                    image.height
                )
            }
            Self::ClaimsNoImage { extension, hdu } => {
                write!(f, "{extension} HDU {hdu} names no image")
            }
            Self::ClaimsMissingImage {
                extension,
                hdu,
                image,
            } => write!(
                f,
                "{extension} HDU {hdu} is for {image:?}, which is no image of the file"
            ),
            Self::DuplicateClaim { extension, image } => {
                write!(f, "two {extension} HDUs claim image {image:?}")
            }
            Self::GainStep { step: Some(step) } => write!(
                f,
                "{}: a node every {step} pixels; this reader takes {}",
                LumosExtension::Gain,
                flat_gain::STEP
            ),
            Self::GainStep { step: None } => write!(
                f,
                "{}: no node spacing stated; this reader takes {}",
                LumosExtension::Gain,
                flat_gain::STEP
            ),
            Self::NotAGain { node, value } => write!(
                f,
                "{} node {node} holds {value}, not a gain",
                LumosExtension::Gain
            ),
            Self::GainChannels { grids, channels } => write!(
                f,
                "{} holds {grids} grids for an image of {channels} colours or channels",
                LumosExtension::Gain
            ),
            Self::UnknownFlagBits { pixel, byte } => write!(
                f,
                "{} at ({}, {}): byte {byte:#04x} holds a bit no flag has",
                LumosExtension::Flags,
                pixel.x,
                pixel.y
            ),
            Self::NoDataDisagrees { pixel } => write!(
                f,
                "{} at ({}, {}): NO_DATA disagrees with the image's NaN",
                LumosExtension::Flags,
                pixel.x,
                pixel.y
            ),
        }
    }
}

/// What a FITS decode sizes against the load's memory limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FitsAllocation {
    /// The HDU's data unit as stored, padded to whole FITS blocks.
    StoredDataUnit,
    DecodedOutput,
    /// The decoded output beside a flags extension as read and the decode's own flag plane.
    DecodedOutputWithFlags,
    /// The most the decode holds at once.
    PeakMemory,
    FlagsExtension,
    /// The byte per pixel the decode flags nulls and saturation in.
    FlagPlane,
    /// One row of the decoded output.
    OutputRow,
    /// The rows of samples the decode converts at once.
    DecodeChunk,
    /// One chunk of samples as stored.
    NativeDecodeChunk,
}

impl Display for FitsAllocation {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::StoredDataUnit => "stored data unit",
            Self::DecodedOutput => "decoded output",
            Self::DecodedOutputWithFlags => "decoded output with its flags",
            Self::PeakMemory => "estimated peak memory",
            Self::FlagsExtension => "flags extension",
            Self::FlagPlane => "flag plane",
            Self::OutputRow => "output row",
            Self::DecodeChunk => "decode chunk",
            Self::NativeDecodeChunk => "native decode chunk",
        })
    }
}

/// Why an image is refused as scientific input: one case per refusal a caller can tell apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScientificRefusal {
    /// Camera RAW read as a [`LinearImage`](crate::LinearImage): it is a mosaic until calibrated.
    CameraRaw,
    /// A FITS image whose header declares a mosaic, read as a [`LinearImage`](crate::LinearImage).
    MosaicFits,
    /// A raster read as a [`CfaImage`](crate::CfaImage): no raster decoder states a mosaic.
    RasterCfa,
    /// A raster whose transfer and colour transforms are not decoded.
    PreviewOnlyRaster {
        format: FileFormat,
    },
    RasterSideTooLong {
        width: usize,
        height: usize,
    },
    /// A TIFF of integer samples.
    IntegerRaster,
    AlphaChannel,
}

impl Display for ScientificRefusal {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::CameraRaw => write!(
                f,
                "camera RAW must be loaded as CfaImage and calibrated before demosaicing"
            ),
            Self::MosaicFits => write!(
                f,
                "mosaic FITS must be loaded as CfaImage and calibrated before demosaicing"
            ),
            Self::RasterCfa => write!(f, "scientific CFA input must be camera RAW or FITS"),
            Self::PreviewOnlyRaster { .. } => write!(
                f,
                "PNG and JPEG are preview-only because their transfer and color transforms are \
                 not decoded"
            ),
            Self::RasterSideTooLong { width, height } => write!(
                f,
                "{width}x{height} has a side past {} px",
                ImageDimensions::MAX_SIDE
            ),
            Self::IntegerRaster => {
                write!(f, "scientific raster input must be a floating-point TIFF")
            }
            Self::AlphaChannel => write!(
                f,
                "scientific raster input must not contain an alpha channel"
            ),
        }
    }
}

/// A keyword's value as a refusal states it: the value, or `none` for a keyword the header leaves
/// out.
#[derive(Debug)]
struct Stated<'a, T>(Option<&'a T>);

impl<T: Display> Display for Stated<'_, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(value) => value.fmt(f),
            None => f.write_str("none"),
        }
    }
}
