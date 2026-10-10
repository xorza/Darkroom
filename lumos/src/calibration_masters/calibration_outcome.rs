//! [`CalibrationOutcome`]: what calibrating one light checked, and what it could not.

use crate::calibration_masters::temperature_source::TemperatureSource;
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
    /// The temperature the dark was matched on: the sensor's, or the camera body's where the two
    /// do not both state the sensor's; `None` when they share neither, which `unverified` records.
    pub dark_temperature: Option<TemperatureSource>,
}
