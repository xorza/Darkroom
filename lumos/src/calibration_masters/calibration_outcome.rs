//! [`CalibrationOutcome`]: what calibrating one light checked, and what it could not.

/// What calibrating one light could not check, and what it scaled.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CalibrationOutcome {
    /// The light or the dark declares no exposure, so the two were not compared.
    pub unverified_exposure: bool,
    /// The light or the dark declares no sensor temperature, so the two were not compared. A DSLR
    /// declares none.
    pub unverified_temperature: bool,
    /// The factor the bias-removed dark was scaled by, the light's exposure over the dark's, when
    /// the two differ.
    pub dark_scale: Option<f64>,
}
