//! Why a calibration bundle does not fit the frame it was asked to calibrate.

use crate::io::image::cfa::CfaType;
use crate::io::image::sample_domain::SampleDomain;
use crate::math::size2us::Size2us;
use crate::stacking::calibration_masters::{CalibrationComponent, MasterRole};

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
    /// The light is already calibrated: its file says so (`LUMCAL`), or this set calibrated it.
    /// A second pass would subtract the dark and divide the flat twice.
    #[error("the light frame is already calibrated")]
    AlreadyCalibrated,
    /// The flat, once its own bias or flat-dark is subtracted, has no positive mean to normalize
    /// by — in the given CFA colour channel, or over the whole frame (`None`). Swapped roles (a
    /// dark given as the flat) and a subtractor at the wrong level both end here.
    #[error("the subtracted flat has no positive mean{}", channel.map_or(String::new(), |c| format!(" in colour channel {c}")))]
    NonPositiveFlat { channel: Option<usize> },
    /// A calibration master was captured with a different sensor pattern.
    #[error(
        "{component} master CFA pattern {master:?} does not match light frame pattern {light:?}"
    )]
    CfaPatternMismatch {
        component: MasterRole,
        light: CfaType,
        master: CfaType,
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
