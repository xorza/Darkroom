use super::*;
use crate::calibration::calibration_outcome::DarkOutcome;
use crate::calibration::dark_scale::DarkScale;
use crate::image::metadata::dark_comparison::DarkComparison;
use crate::image::metadata::temperature_difference::TemperatureDifference;

/// A dark compared on the exposure as `exposure_compared` says, on the `temperature` reading, and
/// found apart by `difference`.
fn compared(
    exposure_compared: bool,
    temperature: Option<TemperatureSource>,
    difference: Option<TemperatureDifference>,
) -> DarkComparison {
    DarkComparison {
        exposure_compared,
        temperature,
        temperature_difference: difference,
    }
}

/// Five lights: one unverified in exposure, one in temperature and scaled by its exposure, one
/// matched on its camera's temperature, one matched on its sensor's 3 °C off and scaled by a
/// fit, and one whose dark was scaled by 1, which is no scaling. The report counts each fact
/// once per light that met it, and carries the bundle's floored flat pixels and flat-dark
/// comparison, here none.
#[test]
fn the_notes_count_what_calibration_could_not_check() {
    let sensor = Some(TemperatureSource::Sensor);
    let notes = CalibrationNotes::default();
    notes.record(CalibrationOutcome {
        dark: DarkOutcome::AsIs(compared(false, sensor, None)),
        ..CalibrationOutcome::default()
    });
    notes.record(CalibrationOutcome {
        dark: DarkOutcome::Scaled {
            comparison: compared(true, None, None),
            scale: DarkScale {
                factor: 2.5,
                source: DarkScaleSource::Exposure { fit_pixels: 40 },
            },
        },
        settings_compared: true,
        filter_differs: false,
    });
    notes.record(CalibrationOutcome {
        dark: DarkOutcome::AsIs(compared(true, Some(TemperatureSource::Camera), None)),
        settings_compared: true,
        filter_differs: true,
    });
    notes.record(CalibrationOutcome {
        dark: DarkOutcome::Scaled {
            comparison: compared(
                true,
                sensor,
                Some(TemperatureDifference {
                    reading: TemperatureSource::Sensor,
                    frame: -7.0,
                    dark: -10.0,
                }),
            ),
            scale: DarkScale {
                factor: 1.31,
                source: DarkScaleSource::Fitted {
                    pixels: 400,
                    standard_error: 0.002,
                },
            },
        },
        settings_compared: true,
        filter_differs: false,
    });
    notes.record(CalibrationOutcome {
        dark: DarkOutcome::Scaled {
            comparison: compared(true, sensor, None),
            scale: DarkScale {
                factor: 1.0,
                source: DarkScaleSource::Exposure { fit_pixels: 0 },
            },
        },
        settings_compared: true,
        filter_differs: false,
    });
    assert_eq!(
        notes.report(&CalibrationMasters::default()),
        CalibrationReport {
            unverified_dark_exposures: 1,
            unverified_dark_temperatures: 1,
            unverified_settings: 1,
            filter_differences: 1,
            camera_temperature_darks: 1,
            dark_temperature_differences: 1,
            scaled_darks: 1,
            fitted_dark_scales: 1,
            flat_dark: None,
            floored_flat_pixels: 0,
        }
    );
}
