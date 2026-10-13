//! [`DarkMatch`]: how a master holding dark signal is taken from a frame.

use crate::calibration::error::DarkMismatch;
use crate::calibration::temperature_difference::TemperatureDifference;
use crate::calibration::temperature_source::TemperatureSource;
use crate::frame::capture_condition::CaptureCondition;
use crate::frame::capture_conditions::CaptureConditions;
use crate::frame::unverified_conditions::UnverifiedConditions;
use crate::image::calibration_state::CalibrationState;

/// How a master holding dark signal is taken from a frame — a dark from a light, a flat-dark from
/// a flat: the factor on its signal, and what could not be compared.
///
/// Exposures within tolerance count as one and leave the dark unscaled. A dark of another exposure
/// is scaled by the frame's exposure over its own only once its bias is gone, since the bias does
/// not grow with exposure. The temperatures compared are the sensor's where both state it, else the
/// camera body's where both state that; a difference past the tolerance is recorded, not refused.
/// A condition one side does not state is not compared.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DarkMatch {
    /// The frame's exposure over the dark's, when the two differ and the dark could be scaled.
    pub(crate) scale: Option<f64>,
    pub(crate) unverified: UnverifiedConditions,
    /// The temperature the two were compared on; `None` when they share none.
    pub(crate) temperature: Option<TemperatureSource>,
    /// The two temperatures, when they differ past the tolerance.
    pub(crate) temperature_difference: Option<TemperatureDifference>,
}

/// Whether a master holding dark signal still holds the bias, which does not grow with exposure,
/// so a dark that holds it cannot be scaled to another exposure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DarkBias {
    Held,
    Removed,
}

impl DarkBias {
    /// Whether `held`, the parts a dark still holds, names the bias.
    pub(crate) const fn held_in(held: CalibrationState) -> Self {
        if held.bias { Self::Held } else { Self::Removed }
    }
}

impl DarkMatch {
    /// Match a dark taken under `dark`, holding the bias as `bias` says, to a frame taken under
    /// `frame`.
    ///
    /// # Errors
    ///
    /// The exposures disagree and the dark still holds the bias or was exposed for 0 s.
    pub(crate) fn new(
        frame: CaptureConditions,
        dark: CaptureConditions,
        bias: DarkBias,
    ) -> Result<Self, DarkMismatch> {
        let compared = [
            (
                TemperatureSource::Sensor,
                CaptureCondition::Temperature,
                frame.ccd_temp.zip(dark.ccd_temp),
            ),
            (
                TemperatureSource::Camera,
                CaptureCondition::CameraTemperature,
                frame.camera_temp.zip(dark.camera_temp),
            ),
        ]
        .into_iter()
        .find_map(|(reading, condition, pair)| pair.map(|pair| (reading, condition, pair)));
        let temperature_difference = compared
            .filter(|(_, condition, (frame, dark))| {
                !condition.agree(frame.min(*dark), frame.max(*dark))
            })
            .map(|(reading, _, (frame, dark))| TemperatureDifference {
                reading,
                frame,
                dark,
            });
        let temperature = compared.map(|(reading, ..)| reading);
        let (Some(frame), Some(dark)) = (frame.exposure_time, dark.exposure_time) else {
            return Ok(Self {
                scale: None,
                unverified: UnverifiedConditions {
                    exposure: true,
                    temperature: temperature.is_none(),
                },
                temperature,
                temperature_difference,
            });
        };
        let scale = if CaptureCondition::Exposure.agree(frame.min(dark), frame.max(dark)) {
            None
        } else if bias == DarkBias::Held || dark == 0.0 {
            // A dark of 0 s holds no dark signal to scale.
            return Err(DarkMismatch::Exposure { frame, dark });
        } else {
            Some(frame / dark)
        };
        Ok(Self {
            scale,
            unverified: UnverifiedConditions {
                exposure: false,
                temperature: temperature.is_none(),
            },
            temperature,
            temperature_difference,
        })
    }

    /// The factor the dark's signal is subtracted with.
    pub(crate) const fn factor(self) -> f64 {
        match self.scale {
            Some(scale) => scale,
            None => 1.0,
        }
    }
}

#[cfg(test)]
mod tests;
