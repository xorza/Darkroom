use super::*;
use crate::calibration::dark_scale::DarkScale;
use crate::image::dark_comparison::DarkComparison;
use crate::image::temperature_difference::TemperatureDifference;

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
        dark: Some(compared(false, sensor, None)),
        ..CalibrationOutcome::default()
    });
    notes.record(CalibrationOutcome {
        dark: Some(compared(true, None, None)),
        dark_scale: Some(DarkScale {
            factor: 2.5,
            source: DarkScaleSource::Exposure { fit_pixels: 40 },
        }),
    });
    notes.record(CalibrationOutcome {
        dark: Some(compared(true, Some(TemperatureSource::Camera), None)),
        ..CalibrationOutcome::default()
    });
    notes.record(CalibrationOutcome {
        dark: Some(compared(
            true,
            sensor,
            Some(TemperatureDifference {
                reading: TemperatureSource::Sensor,
                frame: -7.0,
                dark: -10.0,
            }),
        )),
        dark_scale: Some(DarkScale {
            factor: 1.31,
            source: DarkScaleSource::Fitted {
                pixels: 400,
                standard_error: 0.002,
            },
        }),
    });
    notes.record(CalibrationOutcome {
        dark: Some(compared(true, sensor, None)),
        dark_scale: Some(DarkScale {
            factor: 1.0,
            source: DarkScaleSource::Exposure { fit_pixels: 0 },
        }),
    });
    let mut report = RunReport::default();
    notes.report_into(&mut report, &CalibrationMasters::default());
    assert_eq!(
        (
            report.unverified_dark_exposures,
            report.unverified_dark_temperatures,
            report.dark_temperature_differences,
            report.scaled_darks,
            report.fitted_dark_scales,
            report.camera_temperature_darks,
            report.floored_flat_pixels,
            report.flat_dark
        ),
        (1, 1, 1, 1, 1, 1, 0, None)
    );
}
