//! Why a calibration bundle does not fit the frame it was asked to calibrate, and why a bundle file
//! could not be read.

use std::io;

use crate::calibration_masters::calibration_component::CalibrationComponent;
use crate::calibration_masters::fits::BUNDLE_VERSION;
use crate::calibration_masters::master_role::MasterRole;
use crate::combine::error::StackError;
use crate::io::image::cfa::CfaType;
use crate::io::image::error::ImageError;
use crate::io::image::sample_domain::SampleDomain;
use crate::math::size2us::Size2us;

/// A calibration master does not describe the same measurement as the light it is applied to.
///
/// Every variant is bad input rather than a broken invariant: masters are read from user-chosen
/// files, so a set that does not fit the light is reported with the offending role named, not
/// asserted on.
///
/// Not `Eq`: [`Self::SampleDomainMismatch`] reports the two domains, and their spans are floats,
/// which have no total equality.
#[derive(Debug, thiserror::Error, Clone, PartialEq)]
pub enum CalibrationError {
    /// The light already lost a part to calibration: its file says so (`LUMCALB`, `LUMCALD`,
    /// `LUMCALF`), or this set calibrated it. A second pass would remove that part twice.
    #[error("the light frame is already calibrated")]
    AlreadyCalibrated,
    /// Building the bundle was cancelled before it finished.
    #[error("calibration-master construction was cancelled")]
    Cancelled,
    /// The flat, once its own bias or flat-dark is subtracted, has no positive mean to normalize
    /// by — in the given CFA colour channel, or over the whole frame (`None`). Swapped roles (a
    /// dark given as the flat) and a subtractor at the wrong level both end here.
    #[error("the subtracted flat has no positive mean{}", channel.map_or(String::new(), |c| format!(" in colour channel {c}")))]
    NonPositiveFlat { channel: Option<usize> },
    /// The flat still holds an additive offset — its pedestal is kept or unknown — and the set has
    /// no bias or flat-dark to remove it: divided by, it would add an inverse-vignetting pattern of
    /// the offset to every light.
    #[error("the flat holds an offset, and the set has no bias or flat-dark to subtract it")]
    FlatWithoutSubtractor,
    /// A master lost more to calibration than its role can have and still serve the set: a bias
    /// anything, a dark or a flat-dark more than its bias, a flat its own response. Subtracted or
    /// divided by, it would leave in the frame what it exists to remove.
    #[error("the {component} master lost more to calibration than a {component} master can")]
    OverCalibratedMaster { component: MasterRole },
    /// The flat-dark holds a part the flat already lost when it was stacked, beside one the flat
    /// still holds, and the set has no bias to separate the two.
    #[error(
        "the flat-dark would remove a part the flat already lost, and the set has no bias to separate it"
    )]
    UnusableFlatDark,
    /// The light still holds an additive offset and the set has a flat but nothing that removes
    /// the offset — no bias, and no dark that still holds it: `(S + b)/flat` puts the offset `b`
    /// under the flat's vignetting.
    #[error("the light holds an offset, and the set has a flat but no dark or bias to subtract it")]
    LightWithoutSubtractor,
    /// A master holding dark signal was taken under other conditions than the frame it calibrates:
    /// the dark against a light, or the flat-dark against the flat.
    #[error("the {component} master does not match the frame it calibrates: {source}")]
    DarkMismatch {
        component: MasterRole,
        source: DarkMismatch,
    },
    /// A calibration master was captured with a different sensor pattern than the rest of the
    /// bundle, when the set is assembled, or than the light, when one is calibrated.
    #[error("{component} master CFA pattern {master:?} does not match {expected:?}")]
    CfaPatternMismatch {
        component: MasterRole,
        expected: CfaType,
        master: CfaType,
    },
    /// A calibration master was taken with another camera than the light: both state one, and they
    /// differ. Its bias, dark current, defects and vignetting are another sensor's.
    #[error("{component} master was taken with {master}, the light with {light}")]
    InstrumentMismatch {
        component: MasterRole,
        light: String,
        master: String,
    },
    /// A calibration master's samples cannot be expressed in the domain of the frame it calibrates.
    ///
    /// Subtracting a master divided by one span from a frame divided by another is not a small
    /// error, it is a no-op that reports success — a `[0, 1]` master against an unnormalized light
    /// removes ~0.01 from ~3000. Two declared spans in one unit relate by their exact ratio and are
    /// converted; what is refused is a different unit, or a span the decoder had to assume.
    #[error(
        "{component} master was decoded into sample domain {master}, which cannot be converted to \
         the {frame} of the frame it calibrates"
    )]
    SampleDomainMismatch {
        component: MasterRole,
        frame: SampleDomain,
        master: SampleDomain,
    },
    /// A calibration master covers a different sensor area than the frame it has to line up with:
    /// the rest of the bundle when the set is assembled, or the light when one is calibrated.
    #[error("{component} master is {master}, expected {expected}")]
    DimensionMismatch {
        component: CalibrationComponent,
        expected: Size2us,
        master: Size2us,
    },
}

/// A master holding dark signal cannot be taken from the frame: dark current grows with the
/// exposure, and this dark cannot be scaled to the frame's.
#[derive(Debug, thiserror::Error, Clone, Copy, PartialEq)]
pub enum DarkMismatch {
    /// The dark was exposed for another time than the frame, and it cannot be scaled to the
    /// frame: it still holds the bias, which does not grow with exposure, or it was exposed for 0 s.
    #[error(
        "the dark's exposure {dark} s does not match the frame's {frame} s, and the dark cannot be scaled to it"
    )]
    Exposure { frame: f64, dark: f64 },
}

/// Why the master a calibration stack subtracts from every frame cannot be taken from them: the
/// master itself, or frame `index` it meets.
#[derive(Debug, thiserror::Error)]
pub enum SubtractionError {
    /// The master a calibration stack subtracts from every frame is of another sensor shape or
    /// pattern than frame `index`.
    #[error(
        "frame {index} is {frame} on its sensor, but the master it subtracts is {subtractor}, or of another pattern"
    )]
    SubtractorShape {
        index: usize,
        frame: Size2us,
        subtractor: Size2us,
    },

    /// The master a calibration stack subtracts cannot be expressed in frame `index`'s domain.
    #[error(
        "frame {index} was decoded into sample domain {frame}, which the subtracted master's {subtractor} cannot be converted to"
    )]
    SubtractorDomain {
        index: usize,
        frame: Box<SampleDomain>,
        subtractor: Box<SampleDomain>,
    },

    /// The master a calibration stack would subtract from every frame lost, when it was itself
    /// stacked, everything its role holds, or more than its role may lose: a bias anything, a dark
    /// or a flat-dark more than its bias.
    #[error("the {subtractor} master is calibrated past anything it could subtract")]
    OverCalibratedSubtractor { subtractor: MasterRole },

    /// The master a calibration stack would subtract holds more than a frame of the stacked role
    /// may lose before it is combined: a dark or a flat-dark only its bias, a bias nothing, and no
    /// frame its flat response.
    #[error(
        "the {subtractor} master holds more than a {target} frame may lose before it is combined"
    )]
    SubtractorForRole {
        target: MasterRole,
        subtractor: MasterRole,
    },

    /// Frame `index` already lost a part of what the subtracted master holds: subtracted again,
    /// that part would be removed twice.
    #[error("frame {index} already lost part of what the {subtractor} master holds")]
    SubtractedTwice {
        index: usize,
        subtractor: MasterRole,
    },

    /// The master holding dark signal that a calibration stack subtracts was taken under other
    /// conditions than frame `index`.
    #[error("frame {index} cannot take the subtracted master: {source}")]
    SubtractorConditions { index: usize, source: DarkMismatch },
}

impl SubtractionError {
    /// The error with each frame index it holds, a position in the list it was raised over, read
    /// through `inputs` into the caller's own numbering of those frames.
    pub(crate) fn renumbered(self, inputs: &[usize]) -> Self {
        match self {
            Self::SubtractorShape {
                index,
                frame,
                subtractor,
            } => Self::SubtractorShape {
                index: inputs[index],
                frame,
                subtractor,
            },
            Self::SubtractorDomain {
                index,
                frame,
                subtractor,
            } => Self::SubtractorDomain {
                index: inputs[index],
                frame,
                subtractor,
            },
            Self::SubtractedTwice { index, subtractor } => Self::SubtractedTwice {
                index: inputs[index],
                subtractor,
            },
            Self::SubtractorConditions { index, source } => Self::SubtractorConditions {
                index: inputs[index],
                source,
            },
            error @ (Self::OverCalibratedSubtractor { .. } | Self::SubtractorForRole { .. }) => {
                error
            }
        }
    }
}

/// Why [`CalibrationMasters::build`](crate::CalibrationMasters::build) could not build the masters.
#[derive(Debug, thiserror::Error)]
pub enum MasterBuildError<E> {
    /// A role's master could not be stacked.
    #[error(transparent)]
    Stack(StackError),
    /// The stacked masters do not make a set.
    #[error(transparent)]
    Calibration(CalibrationError),
    /// The cache could not be read or written.
    #[error("calibration master cache: {0}")]
    Cache(E),
    /// The build was cancelled: between roles, inside a role's stack, or while the set was
    /// assembled.
    #[error("calibration master build cancelled")]
    Cancelled,
}

/// A cancel inside a role's stack is the build's cancel.
impl<E> From<StackError> for MasterBuildError<E> {
    fn from(error: StackError) -> Self {
        match error {
            StackError::Cancelled => Self::Cancelled,
            error => Self::Stack(error),
        }
    }
}

/// A cancel while the set is assembled is the build's cancel.
impl<E> From<CalibrationError> for MasterBuildError<E> {
    fn from(error: CalibrationError) -> Self {
        match error {
            CalibrationError::Cancelled => Self::Cancelled,
            error => Self::Calibration(error),
        }
    }
}

/// Why [`CalibrationMasters::load`](crate::CalibrationMasters::load) could not read a bundle.
#[derive(Debug, thiserror::Error)]
pub enum BundleError {
    /// The file could not be opened or read.
    #[error("calibration-master bundle: {0}")]
    Io(#[from] io::Error),
    /// The file is not well-formed FITS, or a header value has the wrong type.
    #[error("calibration-master bundle: {0}")]
    Fits(#[from] fits_well::FitsError),
    /// The file does not start with the dataless primary HDU that names a Lumos bundle.
    #[error("not a Lumos calibration-master bundle")]
    NotABundle,
    /// The primary HDU names a bundle but no version of it.
    #[error("the calibration-master bundle states no version")]
    MissingVersion,
    /// The bundle is of another version than this build reads.
    #[error("calibration-master bundle version {found}; this build reads version {BUNDLE_VERSION}")]
    Version { found: i64 },
    /// An HDU's data or header does not match its checksum.
    #[error("HDU {hdu} of the calibration-master bundle fails its checksum")]
    Checksum { hdu: usize },
    /// An extension has no `EXTNAME` to say what it holds.
    #[error("HDU {hdu} of the calibration-master bundle has no EXTNAME")]
    MissingExtname { hdu: usize },
    /// An extension's `EXTNAME` names nothing a bundle holds.
    #[error("unknown calibration-master bundle extension {extname:?}")]
    UnknownExtension { extname: String },
    /// Two extensions name the same component.
    #[error("calibration-master bundle extension {extname:?} appears twice")]
    DuplicateExtension { extname: String },
    /// A bundle holds no flat-dark: the flat is stored prepared, the flat-dark spent on it.
    #[error("the calibration-master bundle holds a flat-dark")]
    FlatDark,
    /// A defect map without the dark or the flat it was detected on.
    #[error("the calibration-master bundle holds a defect map without a dark or a flat")]
    DefectsWithoutMaster,
    /// A flags extension names no image of the file, or is not this format's.
    #[error(transparent)]
    Flags(ImageError),
    /// A master's HDU is not an uncompressed 32-bit float image.
    #[error("the {role} master of the bundle is not an uncompressed BITPIX=-32 image")]
    MasterEncoding { role: MasterRole },
    /// A master's HDU states another role, or another prepared state, than its `EXTNAME`'s.
    #[error("the {role} master of the bundle states another role or prepared state")]
    MasterRecord { role: MasterRole },
    /// A master's HDU could not be decoded.
    #[error("the {role} master of the bundle: {source}")]
    Master {
        role: MasterRole,
        source: ImageError,
    },
    /// The defect map's table is not what a bundle writes.
    #[error("the bundle's defect map: {0}")]
    DefectTable(DefectTableError),
    /// The masters read do not make a set.
    #[error(transparent)]
    Calibration(#[from] CalibrationError),
    /// The load was cancelled while a master was decoded.
    #[error("calibration-master bundle load cancelled")]
    Cancelled,
}

/// What is wrong with a bundle's defect-map table.
#[derive(Debug, thiserror::Error, Clone, Copy, PartialEq, Eq)]
pub enum DefectTableError {
    /// Not a binary table of this format and version.
    #[error("not a Lumos defect table of this version")]
    Format,
    /// The sensor extent is missing, not positive, or overflows.
    #[error("no valid sensor extent")]
    Extent,
    /// A column is missing or of another type.
    #[error("column {column} is missing or of another type")]
    Column { column: &'static str },
    /// A column's length is not the table's row count.
    #[error("the column lengths do not match the row count")]
    ColumnLength,
    /// A row's kind is neither hot (0) nor cold (1).
    #[error("row {row} has kind {kind}, neither hot (0) nor cold (1)")]
    Kind { row: usize, kind: u8 },
    /// A row's index is negative or lies outside the sensor.
    #[error("row {row} indexes no pixel of the sensor")]
    Index { row: usize },
    /// A hot row's excess is not a positive finite number, or a cold row's is not NaN.
    #[error("row {row} has an invalid excess")]
    Excess { row: usize },
}
