//! RAW calibration front end for registered stacking.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use common::CancelToken;

use crate::calibration_masters::CalibrationMasters;
use crate::calibration_masters::calibration_outcome::CalibrationOutcome;
use crate::calibration_masters::dark_scale::DarkScaleSource;
use crate::calibration_masters::temperature_source::TemperatureSource;
use crate::ingest::ingest_run::IngestRun;
use crate::pipeline::config::AlignStackConfig;
use crate::pipeline::error::AlignStackError;
use crate::pipeline::light_source::{LightFiles, Preparation};
use crate::pipeline::result::AlignStackResult;
use crate::progress::progress_callback::ProgressCallback;
use crate::run_report::RunReport;

/// Calibrate, align, and stack camera-RAW or mosaic-FITS light frames end to end.
///
/// For each raw light: load it as a `CfaImage`, apply `masters` (dark/flat/defect) in place,
/// demosaic to a `LinearImage`, and detect its stars, then register, warp and combine. A frame
/// that fails to **load** is a hard error (bad input); a frame that fails to **register** is
/// dropped and reported in
/// [`AlignmentSummary::dropped`](crate::pipeline::result::AlignmentSummary::dropped).
///
/// With [`Reference::Index`](crate::Reference::Index) the reference is prepared first, and each other light then goes
/// through its preparation, its registration and its warp in one pass, written once. With
/// [`Reference::Auto`](crate::Reference::Auto) every light is prepared and detected first, since the reference is the
/// sharpest one, and parked until it registers.
///
/// The sensor geometry is peeked from the first frame's header without a decode, so the memory
/// tier is chosen before any pixels are read. When the frame set plus its per-frame scratch
/// won't fit the budget, every parked and warped frame goes through the frame store's memory maps
/// and peak RAM stays flat in the frame count.
///
/// For lights already calibrated (e.g. pre-processed FITS), skip this and call
/// [`align_and_stack_paths`](crate::pipeline::align::align_and_stack_paths), or
/// [`align_and_stack`](crate::pipeline::align::align_and_stack) for frames already decoded.
pub fn calibrate_align_stack<P: AsRef<Path> + Sync>(
    light_paths: &[P],
    masters: &CalibrationMasters,
    config: &AlignStackConfig,
    progress: ProgressCallback,
    cancel: CancelToken,
) -> Result<AlignStackResult, AlignStackError> {
    if light_paths.is_empty() {
        return Err(AlignStackError::NoFrames);
    }
    config.validate(light_paths.len())?;
    let run = IngestRun::new(&config.ingest, cancel.clone());
    let notes = CalibrationNotes::default();
    let mut result = LightFiles {
        paths: light_paths,
        preparation: Preparation::Calibrate {
            masters,
            cosmic_ray: config.cosmic_ray.as_ref(),
            notes: &notes,
        },
    }
    .stack(config, &run, progress, cancel)?;
    notes.report_into(&mut result.product.report, masters);
    Ok(result)
}

/// What calibrating the lights could not check, counted across the workers.
#[derive(Debug, Default)]
pub(crate) struct CalibrationNotes {
    unverified_exposures: AtomicU64,
    unverified_temperatures: AtomicU64,
    temperature_differences: AtomicU64,
    scaled_darks: AtomicU64,
    fitted_dark_scales: AtomicU64,
    camera_temperature_darks: AtomicU64,
}

impl CalibrationNotes {
    pub(crate) fn record(&self, outcome: CalibrationOutcome) {
        let count = |counter: &AtomicU64, happened: bool| {
            counter.fetch_add(u64::from(happened), Ordering::Relaxed);
        };
        count(&self.unverified_exposures, outcome.unverified.exposure);
        count(
            &self.unverified_temperatures,
            outcome.unverified.temperature,
        );
        count(
            &self.temperature_differences,
            outcome.dark_temperature_difference.is_some(),
        );
        count(
            &self.scaled_darks,
            outcome.dark_scale.is_some_and(|scale| {
                matches!(scale.source, DarkScaleSource::Exposure { .. }) && scale.factor != 1.0
            }),
        );
        count(
            &self.fitted_dark_scales,
            outcome
                .dark_scale
                .is_some_and(|scale| matches!(scale.source, DarkScaleSource::Fitted { .. })),
        );
        count(
            &self.camera_temperature_darks,
            outcome.dark_temperature == Some(TemperatureSource::Camera),
        );
    }

    fn report_into(&self, report: &mut RunReport, masters: &CalibrationMasters) {
        report.unverified_dark_exposures = self.unverified_exposures.load(Ordering::Relaxed);
        report.unverified_dark_temperatures = self.unverified_temperatures.load(Ordering::Relaxed);
        report.dark_temperature_differences = self.temperature_differences.load(Ordering::Relaxed);
        report.scaled_darks = self.scaled_darks.load(Ordering::Relaxed);
        report.fitted_dark_scales = self.fitted_dark_scales.load(Ordering::Relaxed);
        report.camera_temperature_darks = self.camera_temperature_darks.load(Ordering::Relaxed);
        report.unverified_flat_dark = masters.unverified_flat_dark();
        report.floored_flat_pixels = masters.floored_flat_pixels() as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calibration_masters::dark_scale::DarkScale;
    use crate::calibration_masters::temperature_difference::TemperatureDifference;
    use crate::io::image::unverified_conditions::UnverifiedConditions;

    /// Five lights: one unverified in exposure, one in temperature and scaled by its exposure, one
    /// matched on its camera's temperature, one matched on its sensor's 3 °C off and scaled by a fit,
    /// and one whose dark was scaled by 1, which is no scaling. The report counts each fact once per
    /// light that met it, and carries the bundle's floored flat pixels and unverified flat-dark,
    /// here none.
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
}
