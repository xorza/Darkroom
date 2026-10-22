//! The errors of FITS: why a file is refused, what a decode sizes, and why a load or a write
//! failed. A load's error names no path; the entry point a caller reached attaches it.

use std::fmt::{self, Display, Formatter};
use std::io;
use std::ops::Range;

use fits_well::image::{Bitpix, Scaling};
use fits_well::io::ChecksumReport;
use thiserror::Error;

use crate::error::Cancelled;
use crate::image::error::XTransPatternError;
use crate::image::flat_gain::{self, GainGrid};
use crate::image::image_dimensions::ImageDimensions;
use crate::io::error::OverBudget;
use crate::io::fits::cfa::{CFA_FITS_FORMAT, CFA_FITS_VERSION};
use crate::io::fits::linear::LINEAR_FITS_VERSION;
use crate::io::fits::lumos_extension::LumosExtension;
use crate::io::fits::metadata::domain_keywords::{PEDESTAL, PEDESTAL_LEVEL, SCALE_ORIGIN};
use crate::io::fits::product::{INVERSE_VARIANCE, PRODUCT_VERSION};
use crate::io::null_policy::NullSamples;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

/// Why a FITS load failed, before the entry point names the file.
#[derive(Debug, Error)]
pub(crate) enum FitsLoadError {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("{0}")]
    Fits(#[from] fits_well::FitsError),
    #[error("{0}")]
    Refused(FitsRefusal),
    #[error("{0}")]
    NullSamples(#[from] NullSamples),
    #[error("cancelled")]
    Cancelled,
}

impl From<FitsRefusal> for FitsLoadError {
    fn from(reason: FitsRefusal) -> Self {
        Self::Refused(reason)
    }
}

impl From<Cancelled> for FitsLoadError {
    fn from(Cancelled: Cancelled) -> Self {
        Self::Cancelled
    }
}

/// Why a FITS file is refused: one case per refusal a caller can tell apart.
#[derive(Debug, Clone, PartialEq)]
pub enum FitsRefusal {
    /// [`FitsHduSelector::Idx`](crate::FitsHduSelector::Idx) past the file's `hdus`.
    HduOutOfRange {
        idx: usize,
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
    /// `allocation` is past the load's memory limit.
    OverBudget {
        allocation: FitsAllocation,
        budget: OverBudget,
    },
    /// The `BSCALE` of a lumos-written file, which is not finite and positive, or comes with a
    /// `BZERO` other than 0.
    RecordedScale {
        bscale: f64,
        bzero: f64,
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
    /// A section of `rows` of `channel` read as `samples`, not the `expected` count.
    SectionLen {
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
    /// A Lumos file of another format, `format`, read as an image of its own.
    NotStandaloneImage {
        format: String,
    },
    CfaVersion {
        version: Option<i64>,
    },
    /// A Lumos linear image of another version than this reader's.
    LinearVersion {
        version: Option<i64>,
    },
    /// A file read as a stack product whose primary HDU is not a Lumos linear image that states a
    /// product version.
    NotAProduct,
    /// A stack product of another version than this reader's.
    ProductVersion {
        version: i64,
    },
    /// A keyword a Lumos format requires, absent.
    MissingKeyword {
        keyword: String,
    },
    /// An `UNCERT` extension whose `UTYPE` is not `InverseVariance`.
    UncertaintyType {
        utype: Option<String>,
    },
    /// A `MASK` sample, at `idx` of its channel-major space, that is not the image's `NO_DATA`
    /// at its pixel: set where the pixel holds data, clear where it holds none, or other than 0
    /// and 1.
    MaskDisagrees {
        idx: usize,
    },
    /// A stack product that states a uniform coverage and holds a `COVERAGE` extension too.
    CoverageTwice,
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
    /// A gain extension's stated key, `text`, which is no 64 hex digits.
    GainKeyText {
        text: String,
    },
    /// A gain extension whose nodes are not the gain its stated key names.
    GainKeyMismatch,
    /// A gain extension's `node` holding `value`, which is not a finite gain above zero.
    NotAGain {
        node: usize,
        value: f32,
    },
    /// A gain extension of `grids` grids for an image of `channels` colors or channels.
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
    /// A keyword whose value lies outside the range its meaning allows.
    KeywordOutOfRange {
        keyword: String,
    },
    /// A `CFATYPE` that names no pattern: neither `MONO`, `BAYER` nor `XTRANS`.
    CfaTypeKeyword {
        value: String,
    },
    /// `CFATYPE = 'BAYER'` with no `BAYERPAT` to give the phase.
    MissingBayerPattern,
    /// `BAYERPAT = 'TRUE'`, a Bayer mosaic of no stated phase, and no
    /// [`FitsLoadConfig::unstated_bayer_pattern`](crate::FitsLoadConfig::unstated_bayer_pattern)
    /// to give it.
    UnstatedBayerPhase,
    /// A `BAYERPAT` that names no Bayer phase.
    BayerPattern {
        value: String,
    },
    /// `XTRNROW{row}`, absent, or other than six digits of 0, 1 or 2.
    XTransRow {
        row: usize,
    },
    /// Six rows that make no X-Trans layout the demosaic works on.
    XTransPattern(XTransPatternError),
    /// Some of the four white-balance multipliers `LUMWB*`, but not all.
    PartialWhiteBalance,
    /// A white-balance multiplier `LUMWB*` that is not finite and above zero.
    WhiteBalanceRange,
    /// A `LUMSORIG` that names no scale origin.
    ScaleOrigin {
        value: String,
    },
    /// A `LUMPED` that names no pedestal state.
    Pedestal {
        value: String,
    },
    /// `LUMPED = 'KEPT'` with no finite `LUMPEDLV` level.
    PedestalLevel,
}

impl Display for FitsRefusal {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::HduOutOfRange { idx, hdus } => {
                write!(f, "HDU index {idx} is out of range for {hdus} HDUs")
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
                write!(f, "unsupported channel count (NAXIS3): {planes}")
            }
            Self::AxisCount { axes } => write!(f, "unsupported number of dimensions: {axes}"),
            Self::SideTooLong { width, height } => write!(
                f,
                "FITS image {width}x{height} has a side past {} px",
                ImageDimensions::MAX_SIDE
            ),
            Self::SampleCountOverflow { shape } => {
                write!(f, "FITS sample count overflows: {shape:?}")
            }
            Self::SizeOverflow { allocation } => write!(f, "FITS {allocation} size overflows"),
            Self::OverBudget { allocation, budget } => write!(f, "{allocation} {budget}"),
            Self::RecordedScale { bscale, bzero } => write!(
                f,
                "a lumos-written BSCALE {bscale} must be finite and positive, with BZERO 0 \
                 (got {bzero})"
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
            Self::SectionLen {
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
            Self::NotStandaloneImage { format } => {
                write!(f, "Lumos {format} FITS is not a standalone image")
            }
            Self::CfaVersion { version } => write!(
                f,
                "unsupported Lumos CFA FITS version {}; expected {CFA_FITS_VERSION}",
                Stated(version.as_ref())
            ),
            Self::LinearVersion { version } => write!(
                f,
                "unsupported Lumos linear FITS version {}; expected {LINEAR_FITS_VERSION}",
                Stated(version.as_ref())
            ),
            Self::NotAProduct => {
                write!(f, "the primary HDU is not a Lumos stack product")
            }
            Self::ProductVersion { version } => write!(
                f,
                "unsupported Lumos stack product version {version}; expected {PRODUCT_VERSION}"
            ),
            Self::MissingKeyword { keyword } => write!(f, "the keyword {keyword} is missing"),
            Self::UncertaintyType { utype } => write!(
                f,
                "{}: uncertainty type {}; expected {INVERSE_VARIANCE}",
                LumosExtension::Uncertainty,
                Stated(utype.as_ref())
            ),
            Self::MaskDisagrees { idx } => write!(
                f,
                "{} sample {idx} is not the image's NO_DATA at its pixel",
                LumosExtension::Mask
            ),
            Self::CoverageTwice => write!(
                f,
                "the product states a uniform coverage and holds a {} extension",
                LumosExtension::Coverage
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
                    LumosExtension::Flags | LumosExtension::Mask => "BITPIX = 8 bytes",
                    LumosExtension::Gain
                    | LumosExtension::Uncertainty
                    | LumosExtension::Coverage
                    | LumosExtension::Weight
                    | LumosExtension::Dispersion => "BITPIX = -32 floats",
                };
                write!(f, "{extension}: not {samples} at its format's scaling")
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
                extension:
                    extension @ (LumosExtension::Mask
                    | LumosExtension::Uncertainty
                    | LumosExtension::Coverage
                    | LumosExtension::Weight
                    | LumosExtension::Dispersion),
                shape,
                image,
            } => write!(
                f,
                "{extension}: shape {shape:?} is not one or three planes of the image's {}x{}",
                image.width, image.height
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
                    grid.cols,
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
            Self::GainKeyText { text } => write!(
                f,
                "{} states the key {text:?}, which is no 64 hex digits",
                LumosExtension::Gain
            ),
            Self::GainKeyMismatch => write!(
                f,
                "{}: the nodes are not the gain its key states",
                LumosExtension::Gain
            ),
            Self::NotAGain { node, value } => write!(
                f,
                "{} node {node} holds {value}, not a gain",
                LumosExtension::Gain
            ),
            Self::GainChannels { grids, channels } => write!(
                f,
                "{} holds {grids} grids for an image of {channels} colors or channels",
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
            Self::KeywordOutOfRange { keyword } => {
                write!(f, "{keyword} lies outside the range its meaning allows")
            }
            Self::CfaTypeKeyword { value } => {
                write!(f, "CFATYPE {value:?} is none of MONO, BAYER or XTRANS")
            }
            Self::MissingBayerPattern => write!(f, "CFATYPE = 'BAYER' states no BAYERPAT"),
            Self::UnstatedBayerPhase => write!(
                f,
                "BAYERPAT = 'TRUE' states no Bayer phase; FitsLoadOptions::unstated_bayer_pattern \
                 must give it"
            ),
            Self::BayerPattern { value } => {
                write!(f, "BAYERPAT {value:?} is none of RGGB, BGGR, GRBG or GBRG")
            }
            Self::XTransRow { row } => {
                write!(
                    f,
                    "XTRNROW{row} does not hold six X-Trans color digits 0, 1 or 2"
                )
            }
            Self::XTransPattern(error) => write!(f, "XTRNROW0..XTRNROW5: {error}"),
            Self::PartialWhiteBalance => {
                write!(
                    f,
                    "LUMWB* states some of the four white-balance multipliers"
                )
            }
            Self::WhiteBalanceRange => {
                write!(
                    f,
                    "a LUMWB* white-balance multiplier is not finite and positive"
                )
            }
            Self::ScaleOrigin { value } => {
                write!(
                    f,
                    "{SCALE_ORIGIN} {value:?} is neither DECLARED nor ASSUMED"
                )
            }
            Self::Pedestal { value } => {
                write!(f, "{PEDESTAL} {value:?} is neither REMOVED nor KEPT")
            }
            Self::PedestalLevel => {
                write!(
                    f,
                    "{PEDESTAL} = 'KEPT' states no finite {PEDESTAL_LEVEL} level"
                )
            }
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
    /// A stack product's image beside the quality planes read after it.
    QualityPlanes,
    /// A mosaic's decode, then its demosaic, which a preview makes.
    Demosaic,
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
            Self::QualityPlanes => "image with its quality planes",
            Self::Demosaic => "mosaic with its demosaic",
        })
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
