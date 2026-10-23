//! Why a calibration bundle does not fit the frame it was asked to calibrate.

use std::convert::Infallible;

use crate::calibration::calibration_component::CalibrationComponent;
use crate::calibration::master_role::MasterRole;
use crate::combine::error::StackError;
use crate::error::Cancelled;
use crate::frame::error::ConditionMismatch;
use crate::image::cfa::cfa_color::CfaColor;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::sample_domain::SampleDomain;
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
    /// by — in the given color of its mosaic, or over the whole frame of a mono sensor (`None`).
    /// Swapped roles (a dark given as the flat) and a subtractor at the wrong level both end here.
    #[error("the subtracted flat has no positive mean{}", color.map_or(String::new(), |color| format!(" in its {color} photosites")))]
    NonPositiveFlat { color: Option<CfaColor> },
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
/// master itself, or frame `idx` it meets.
#[derive(Debug, thiserror::Error)]
pub enum SubtractionError {
    /// The master a calibration stack subtracts from every frame covers another sensor area than
    /// frame `idx`.
    #[error("frame {idx} is {frame} on its sensor, but the master it subtracts is {subtractor}")]
    SubtractorShape {
        idx: usize,
        frame: Size2us,
        subtractor: Size2us,
    },

    /// The master a calibration stack subtracts from every frame is of another sensor pattern than
    /// frame `idx`.
    #[error(
        "frame {idx} has CFA pattern {frame:?}, but the master it subtracts has {subtractor:?}"
    )]
    SubtractorPattern {
        idx: usize,
        frame: CfaType,
        subtractor: CfaType,
    },

    /// The master a calibration stack subtracts cannot be expressed in frame `idx`'s domain.
    #[error(
        "frame {idx} was decoded into sample domain {frame}, which the subtracted master's {subtractor} cannot be converted to"
    )]
    SubtractorDomain {
        idx: usize,
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

    /// Frame `idx` already lost a part of what the subtracted master holds: subtracted again,
    /// that part would be removed twice.
    #[error("frame {idx} already lost part of what the {subtractor} master holds")]
    SubtractedTwice { idx: usize, subtractor: MasterRole },

    /// The master holding dark signal that a calibration stack subtracts was taken under other
    /// conditions than frame `idx`.
    #[error("frame {idx} cannot take the subtracted master: {source}")]
    SubtractorConditions { idx: usize, source: DarkMismatch },
}

/// Why one calibration role's master could not be stacked: the stack, the master its frames
/// have subtracted, or the conditions its frames had to share.
#[derive(Debug, thiserror::Error)]
pub enum MasterStackError {
    #[error(transparent)]
    Stack(#[from] StackError),
    /// The master a calibration stack subtracts from every frame cannot be taken from them.
    #[error(transparent)]
    Subtraction(#[from] SubtractionError),
    /// The frames of a master whose dark signal is one exposure's at one temperature — a dark or a
    /// flat-dark — were not all taken under one: the master would state one frame's conditions and
    /// hold the majority's signal.
    #[error("the {role} frames must share one exposure and one temperature: {source}")]
    Conditions {
        role: MasterRole,
        source: ConditionMismatch,
    },
}

/// A cancelled stack.
impl From<Cancelled> for MasterStackError {
    fn from(cancelled: Cancelled) -> Self {
        Self::Stack(cancelled.into())
    }
}

/// Why [`CalibrationMasters::build`](crate::CalibrationMasters::build) could not build the masters.
#[derive(Debug, thiserror::Error)]
pub enum MasterBuildError<E> {
    /// A role's master could not be stacked.
    #[error(transparent)]
    Stack(MasterStackError),
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

impl MasterBuildError<Infallible> {
    /// The same failure as a build under a cache that fails with `E`: a build with no cache fails
    /// every way but by its cache.
    pub fn with_cache_error<E>(self) -> MasterBuildError<E> {
        match self {
            Self::Stack(error) => MasterBuildError::Stack(error),
            Self::Calibration(error) => MasterBuildError::Calibration(error),
            Self::Cache(never) => match never {},
            Self::Cancelled => MasterBuildError::Cancelled,
        }
    }
}

/// A cancel inside a role's stack is the build's cancel.
impl<E> From<MasterStackError> for MasterBuildError<E> {
    fn from(error: MasterStackError) -> Self {
        match error {
            MasterStackError::Stack(StackError::Cancelled) => Self::Cancelled,
            error => Self::Stack(error),
        }
    }
}

/// A role's stack that fails before its master's own checks.
impl<E> From<StackError> for MasterBuildError<E> {
    fn from(error: StackError) -> Self {
        MasterStackError::Stack(error).into()
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

/// Why an additive master cannot be taken from a frame, as each caller's error then names it: a
/// light's calibration, or a frame of a calibration stack.
#[derive(Debug)]
pub(crate) enum SubtractionFault {
    /// The master holds dark signal and was taken under other conditions than the frame.
    Conditions {
        role: MasterRole,
        source: DarkMismatch,
    },
    /// The master's domain cannot be expressed in the frame's, as the frame stands when the master
    /// applies.
    Domain {
        role: MasterRole,
        frame: SampleDomain,
        master: SampleDomain,
    },
}

impl From<SubtractionFault> for CalibrationError {
    fn from(fault: SubtractionFault) -> Self {
        match fault {
            SubtractionFault::Conditions { role, source } => Self::DarkMismatch {
                component: role,
                source,
            },
            SubtractionFault::Domain {
                role,
                frame,
                master,
            } => Self::SampleDomainMismatch {
                component: role,
                frame,
                master,
            },
        }
    }
}

impl SubtractionError {
    /// The `fault` of taking a master from frame `idx` of a calibration stack.
    pub(super) fn of_frame(idx: usize, fault: SubtractionFault) -> Self {
        match fault {
            SubtractionFault::Conditions { source, .. } => {
                Self::SubtractorConditions { idx, source }
            }
            SubtractionFault::Domain { frame, master, .. } => Self::SubtractorDomain {
                idx,
                frame: Box::new(frame),
                subtractor: Box::new(master),
            },
        }
    }
}
