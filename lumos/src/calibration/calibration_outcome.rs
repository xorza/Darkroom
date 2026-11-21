//! [`CalibrationOutcome`]: what calibrating one light checked, and what it could not.

use crate::calibration::dark_scale::DarkScale;
use crate::image::metadata::dark_comparison::DarkComparison;

/// What calibrating one light compared, and what it scaled.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CalibrationOutcome {
    /// What became of the dark.
    pub dark: DarkOutcome,
    /// Whether the sensor setting of every additive master — gain, ISO, offset and binning — was
    /// compared with the light's in full: false where one side states a setting the other does
    /// not, or neither states any. A setting both state and differ on refuses the master.
    pub settings_compared: bool,
    /// Whether the flat was taken through another filter than the light, both stating one.
    pub filter_differs: bool,
}

/// What became of a light's dark: what it was compared to the light on — the record the light's
/// metadata keeps too — and, for a dark that lost its bias, the factor it was scaled by.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum DarkOutcome {
    /// The set holds no dark.
    #[default]
    NoDark,
    /// A dark that still holds the bias, subtracted as it is, the exposures having matched.
    AsIs(DarkComparison),
    /// A dark that lost its bias, scaled to the light.
    Scaled {
        comparison: DarkComparison,
        scale: DarkScale,
    },
}

impl DarkOutcome {
    /// What the dark was compared to the light on; `None` without a dark.
    pub const fn comparison(&self) -> Option<DarkComparison> {
        match *self {
            Self::NoDark => None,
            Self::AsIs(comparison) | Self::Scaled { comparison, .. } => Some(comparison),
        }
    }

    /// The factor the dark was scaled by, and where it came from; `None` without a dark, or for
    /// one that holds the bias.
    pub const fn scale(&self) -> Option<DarkScale> {
        match *self {
            Self::NoDark | Self::AsIs(_) => None,
            Self::Scaled { scale, .. } => Some(scale),
        }
    }
}
