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

/// A master holding dark signal does not match the frame it is taken from: dark current depends on
/// the exposure and, by about 12% per degree, on the sensor temperature.
#[derive(Debug, thiserror::Error, Clone, Copy, PartialEq)]
pub enum DarkMismatch {
    /// The dark was exposed for another time than the frame, and it cannot be scaled to the
    /// frame: it still holds the bias, which does not grow with exposure, or it was exposed for 0 s.
    #[error(
        "the dark's exposure {dark} s does not match the frame's {frame} s, and the dark cannot be scaled to it"
    )]
    Exposure { frame: f64, dark: f64 },
    /// The dark was taken at another sensor temperature than the frame.
    #[error("the dark's temperature {dark} °C does not match the frame's {frame} °C")]
    Temperature { frame: f64, dark: f64 },
}
