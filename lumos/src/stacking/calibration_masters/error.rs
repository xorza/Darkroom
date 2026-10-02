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
    /// The light frame does not identify its sensor pattern.
    #[error("light frame is missing CFA pattern metadata")]
    MissingLightCfaPattern,
    /// A calibration master does not identify its sensor pattern.
    #[error("{component} master is missing CFA pattern metadata")]
    MissingMasterCfaPattern { component: MasterRole },
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
