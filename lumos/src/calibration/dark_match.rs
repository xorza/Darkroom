//! [`DarkMatch`]: how a master holding dark signal is taken from a frame.

use crate::calibration::error::DarkMismatch;
use crate::calibration::temperature_difference::TemperatureDifference;
use crate::calibration::temperature_source::TemperatureSource;
use crate::frame::capture_conditions::{CaptureCondition, CaptureConditions};
use crate::frame::unverified_conditions::UnverifiedConditions;

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

impl DarkMatch {
    /// Match a dark taken under `dark` to a frame taken under `frame`; `holds_bias` says whether
    /// the dark still holds the bias.
    ///
    /// # Errors
    ///
    /// The exposures disagree and the dark still holds the bias or was exposed for 0 s.
    pub(crate) fn new(
        frame: CaptureConditions,
        dark: CaptureConditions,
        holds_bias: bool,
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
        } else if holds_bias || dark == 0.0 {
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
mod tests {
    use super::*;

    fn conditions(exposure_time: Option<f64>, ccd_temp: Option<f64>) -> CaptureConditions {
        CaptureConditions {
            exposure_time,
            ccd_temp,
            camera_temp: None,
        }
    }

    #[test]
    fn a_bias_free_dark_of_another_exposure_scales_by_the_ratio() {
        // 300 s light, 120 s dark: 300 / 120 = 2.5.
        let matched = DarkMatch::new(
            conditions(Some(300.0), Some(-10.0)),
            conditions(Some(120.0), Some(-10.5)),
            false,
        )
        .unwrap();
        assert_eq!(
            matched,
            DarkMatch {
                scale: Some(2.5),
                unverified: UnverifiedConditions::NONE,
                temperature: Some(TemperatureSource::Sensor),
                temperature_difference: None,
            }
        );
        assert_eq!(matched.factor(), 2.5);
        // The same pair with the bias still in the dark cannot be scaled, nor can a dark of 0 s,
        // whose ratio 300 / 0 has no finite value.
        for (dark, holds_bias) in [(120.0, true), (0.0, false)] {
            assert_eq!(
                DarkMatch::new(
                    conditions(Some(300.0), None),
                    conditions(Some(dark), None),
                    holds_bias,
                ),
                Err(DarkMismatch::Exposure { frame: 300.0, dark })
            );
        }
    }

    #[test]
    fn agreeing_or_unstated_conditions_leave_the_dark_unscaled() {
        // 300 and 301 s are inside 1% of 301 s: one exposure, whatever the bias.
        let agreeing = DarkMatch::new(
            conditions(Some(301.0), None),
            conditions(Some(300.0), None),
            true,
        )
        .unwrap();
        assert_eq!(
            agreeing,
            DarkMatch {
                scale: None,
                unverified: UnverifiedConditions {
                    exposure: false,
                    temperature: true,
                },
                temperature: None,
                temperature_difference: None,
            }
        );
        assert_eq!(agreeing.factor(), 1.0);
        let silent = DarkMatch::new(
            conditions(None, Some(0.0)),
            conditions(Some(60.0), Some(0.5)),
            true,
        )
        .unwrap();
        assert_eq!(
            silent,
            DarkMatch {
                scale: None,
                unverified: UnverifiedConditions {
                    exposure: true,
                    temperature: false,
                },
                temperature: Some(TemperatureSource::Sensor),
                temperature_difference: None,
            }
        );
    }

    /// A dark of another temperature is taken, with the difference recorded: −7 against −10 °C,
    /// 3 °C apart, past the 1 °C tolerance. At −9 °C, 1 °C apart, the bound itself, nothing is.
    #[test]
    fn a_dark_of_another_temperature_is_recorded_not_refused() {
        let matched = DarkMatch::new(
            conditions(Some(300.0), Some(-7.0)),
            conditions(Some(120.0), Some(-10.0)),
            false,
        )
        .unwrap();
        assert_eq!(
            matched,
            DarkMatch {
                scale: Some(2.5),
                unverified: UnverifiedConditions::NONE,
                temperature: Some(TemperatureSource::Sensor),
                temperature_difference: Some(TemperatureDifference {
                    reading: TemperatureSource::Sensor,
                    frame: -7.0,
                    dark: -10.0,
                }),
            }
        );
        let within = DarkMatch::new(
            conditions(Some(300.0), Some(-9.0)),
            conditions(Some(300.0), Some(-10.0)),
            true,
        )
        .unwrap();
        assert_eq!(within.temperature_difference, None);
    }

    /// The camera body's temperature stands in only where the two do not both state the sensor's:
    /// two bodies at 21 and 21.5 °C match on it, at 21 and 23 °C differ on it, and two sensors
    /// that agree decide alone though their bodies are 5 °C apart. A sensor reading on one side
    /// only is no pair, so the bodies are compared.
    #[test]
    fn the_camera_temperature_matches_only_where_the_sensors_do_not() {
        let with_body = |ccd_temp, camera_temp| CaptureConditions {
            camera_temp,
            ..conditions(Some(300.0), ccd_temp)
        };
        let matched = |frame, dark| DarkMatch::new(frame, dark, true);
        assert_eq!(
            matched(with_body(None, Some(21.0)), with_body(None, Some(21.5)))
                .unwrap()
                .temperature,
            Some(TemperatureSource::Camera)
        );
        assert_eq!(
            matched(with_body(None, Some(21.0)), with_body(None, Some(23.0)))
                .unwrap()
                .temperature_difference,
            Some(TemperatureDifference {
                reading: TemperatureSource::Camera,
                frame: 21.0,
                dark: 23.0,
            })
        );
        let sensors = matched(
            with_body(Some(-10.0), Some(20.0)),
            with_body(Some(-10.5), Some(25.0)),
        )
        .unwrap();
        assert_eq!(sensors.temperature, Some(TemperatureSource::Sensor));
        assert!(!sensors.unverified.temperature);
        let one_sensor = matched(
            with_body(Some(-10.0), Some(21.0)),
            with_body(None, Some(21.5)),
        )
        .unwrap();
        assert_eq!(one_sensor.temperature, Some(TemperatureSource::Camera));
    }
}
