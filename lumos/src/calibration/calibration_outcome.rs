//! [`CalibrationOutcome`]: what calibrating one light checked, and what it could not.

use crate::calibration::dark_scale::DarkScale;
use crate::image::dark_comparison::DarkComparison;

/// What calibrating one light compared, and what it scaled.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CalibrationOutcome {
    /// What the dark was compared to the light on, the record the light's metadata keeps too;
    /// `None` without a dark.
    pub dark: Option<DarkComparison>,
    /// The factor a bias-removed dark was scaled by, and where it came from; `None` for a dark
    /// that holds the bias, subtracted as it is.
    pub dark_scale: Option<DarkScale>,
}
