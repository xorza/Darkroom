//! Why a calibration bundle does not fit the frame it was asked to calibrate.

use crate::calibration_masters::calibration_component::CalibrationComponent;
use crate::calibration_masters::master_role::MasterRole;
use crate::io::image::cfa::CfaType;
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
    /// The light is already calibrated: its file says so (`LUMCAL`), or this set calibrated it.
    /// A second pass would subtract the dark and divide the flat twice.
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
    /// The light still holds an additive offset and the set has a flat but no dark or bias:
    /// `(S + b)/flat` puts the offset `b` under the flat's vignetting.
    #[error("the light holds an offset, and the set has a flat but no dark or bias to subtract it")]
    LightWithoutSubtractor,
    /// The dark was exposed for another time than the light, and it still holds the bias, so it
    /// cannot be scaled to the light: its thermal signal is wrong by the ratio.
    #[error(
        "the dark's exposure {dark} s does not match the light's {light} s, and with no bias to separate its thermal signal it cannot be scaled"
    )]
    DarkExposureMismatch { light: f64, dark: f64 },
    /// The dark was taken at another sensor temperature than the light: dark current changes by
    /// about 12% per degree.
    #[error("the dark's temperature {dark} °C does not match the light's {light} °C")]
    DarkTemperatureMismatch { light: f64, dark: f64 },
    /// A calibration master was captured with a different sensor pattern than the rest of the
    /// bundle, when the set is assembled, or than the light, when one is calibrated.
    #[error("{component} master CFA pattern {master:?} does not match {expected:?}")]
    CfaPatternMismatch {
        component: MasterRole,
        expected: CfaType,
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
