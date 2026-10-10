//! [`CalibrationOutcome`]: what calibrating one light checked, and what it could not.

use crate::io::image::unverified_conditions::UnverifiedConditions;

/// What calibrating one light could not check, and what it scaled.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CalibrationOutcome {
    /// The conditions the light or the dark does not state, so the two were not compared on them.
    /// The light's metadata records the same.
    pub unverified: UnverifiedConditions,
    /// The factor the bias-removed dark was scaled by, the light's exposure over the dark's, when
    /// the two differ.
    pub dark_scale: Option<f64>,
}
