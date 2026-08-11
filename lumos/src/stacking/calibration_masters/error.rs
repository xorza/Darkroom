//! Why a calibration bundle does not fit the frame it was asked to calibrate.

use crate::io::image::cfa::CfaType;
use crate::math::size2us::Size2us;
use crate::stacking::calibration_masters::{CalibrationComponent, MasterRole};

/// A calibration master does not describe the same measurement as the light it is applied to.
///
/// Every variant is bad input rather than a broken invariant: masters are read from user-chosen
/// files, so a set that does not fit the light is reported with the offending role named, not
/// asserted on.
///
/// Not `Eq`: [`Self::SampleSpanMismatch`] reports the two spans, and a float has no total equality.
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
    /// A calibration master was decoded into a different sample domain than the light.
    ///
    /// Subtracting a master divided by one span from a light divided by another is not a small
    /// error, it is a no-op that reports success — a `[0, 1]` master against an unnormalized light
    /// removes ~0.01 from ~3000 — and `calibrate` then marks the light calibrated.
    #[error(
        "{component} master was decoded with sample span {master}, but the light used {light}; \
         a master and its light must come from the same domain"
    )]
    SampleSpanMismatch {
        component: MasterRole,
        light: f32,
        master: f32,
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
