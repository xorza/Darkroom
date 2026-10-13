use super::*;
use crate::calibration::dark_scale::DarkScale;
use crate::calibration::temperature_difference::TemperatureDifference;
use crate::frame::unverified_conditions::UnverifiedConditions;

/// Five lights: one unverified in exposure, one in temperature and scaled by its exposure, one
/// matched on its camera's temperature, one matched on its sensor's 3 °C off and scaled by a
/// fit, and one whose dark was scaled by 1, which is no scaling. The report counts each fact
/// once per light that met it, and carries the bundle's floored flat pixels and unverified
/// flat-dark, here none.
#[test]
fn the_notes_count_what_calibration_could_not_check() {
    let notes = CalibrationNotes::default();
    notes.record(CalibrationOutcome {
        unverified: UnverifiedConditions {
            exposure: true,
            temperature: false,
        },
        ..CalibrationOutcome::default()
    });
    notes.record(CalibrationOutcome {
        unverified: UnverifiedConditions {
            exposure: false,
            temperature: true,
        },
        dark_scale: Some(DarkScale {
            factor: 2.5,
            source: DarkScaleSource::Exposure { fit_pixels: 40 },
        }),
        dark_temperature: None,
        dark_temperature_difference: None,
    });
    notes.record(CalibrationOutcome {
        dark_temperature: Some(TemperatureSource::Camera),
        ..CalibrationOutcome::default()
    });
    notes.record(CalibrationOutcome {
        dark_scale: Some(DarkScale {
            factor: 1.31,
            source: DarkScaleSource::Fitted {
                pixels: 400,
                standard_error: 0.002,
            },
        }),
        dark_temperature: Some(TemperatureSource::Sensor),
        dark_temperature_difference: Some(TemperatureDifference {
            reading: TemperatureSource::Sensor,
            frame: -7.0,
            dark: -10.0,
        }),
        ..CalibrationOutcome::default()
    });
    notes.record(CalibrationOutcome {
        dark_scale: Some(DarkScale {
            factor: 1.0,
            source: DarkScaleSource::Exposure { fit_pixels: 0 },
        }),
        ..CalibrationOutcome::default()
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
            report.unverified_flat_dark
        ),
        (1, 1, 1, 1, 1, 1, 0, UnverifiedConditions::NONE)
    );
}
