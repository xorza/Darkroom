//! [`DarkMatch`]: how a master holding dark signal is taken from a frame.

use crate::calibration_masters::error::DarkMismatch;
use crate::frame_store::capture_conditions::{CaptureCondition, CaptureConditions};
use crate::io::image::unverified_conditions::UnverifiedConditions;

/// How a master holding dark signal is taken from a frame — a dark from a light, a flat-dark from
/// a flat: the factor on its signal, and what could not be compared.
///
/// Exposures within tolerance count as one and leave the dark unscaled. A dark of another exposure
/// is scaled by the frame's exposure over its own only once its bias is gone, since the bias does
/// not grow with exposure. A condition one side does not state is not compared.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DarkMatch {
    /// The frame's exposure over the dark's, when the two differ and the dark could be scaled.
    pub(crate) scale: Option<f64>,
    pub(crate) unverified: UnverifiedConditions,
}

impl DarkMatch {
    /// Match a dark taken under `dark` to a frame taken under `frame`; `holds_bias` says whether
    /// the dark still holds the bias.
    ///
    /// # Errors
    ///
    /// The temperatures disagree, or the exposures do and the dark still holds the bias or was
    /// exposed for 0 s.
    pub(crate) fn new(
        frame: CaptureConditions,
        dark: CaptureConditions,
        holds_bias: bool,
    ) -> Result<Self, DarkMismatch> {
        let temperature = match (frame.ccd_temp, dark.ccd_temp) {
            (Some(frame), Some(dark)) => {
                if !CaptureCondition::Temperature.agree(frame.min(dark), frame.max(dark)) {
                    return Err(DarkMismatch::Temperature { frame, dark });
                }
                false
            }
            _ => true,
        };
        let (Some(frame), Some(dark)) = (frame.exposure_time, dark.exposure_time) else {
            return Ok(Self {
                scale: None,
                unverified: UnverifiedConditions {
                    exposure: true,
                    temperature,
                },
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
                temperature,
            },
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
            }
        );
    }

    #[test]
    fn a_dark_of_another_temperature_is_refused_before_its_exposure() {
        // 1.5 °C apart, past the 1 °C tolerance; the exposures differ too, but the temperature is
        // compared first.
        assert_eq!(
            DarkMatch::new(
                conditions(Some(300.0), Some(-8.5)),
                conditions(Some(120.0), Some(-10.0)),
                true,
            ),
            Err(DarkMismatch::Temperature {
                frame: -8.5,
                dark: -10.0,
            })
        );
    }
}
