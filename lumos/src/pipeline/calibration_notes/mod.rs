//! [`CalibrationNotes`]: what calibrating a run's lights could not check.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::calibration::calibration_masters::CalibrationMasters;
use crate::calibration::calibration_outcome::CalibrationOutcome;
use crate::calibration::dark_scale::DarkScaleSource;
use crate::image::metadata::temperature_source::TemperatureSource;
use crate::stack_product::run_report::RunReport;

/// What calibrating the lights could not check, counted across the workers.
#[derive(Debug, Default)]
pub(super) struct CalibrationNotes {
    unverified_exposures: AtomicU64,
    unverified_temperatures: AtomicU64,
    temperature_differences: AtomicU64,
    scaled_darks: AtomicU64,
    fitted_dark_scales: AtomicU64,
    camera_temperature_darks: AtomicU64,
}

impl CalibrationNotes {
    pub(super) fn record(&self, outcome: CalibrationOutcome) {
        let count = |counter: &AtomicU64, happened: bool| {
            counter.fetch_add(u64::from(happened), Ordering::Relaxed);
        };
        let dark = outcome.dark;
        count(
            &self.unverified_exposures,
            dark.is_some_and(|dark| !dark.exposure_compared),
        );
        count(
            &self.unverified_temperatures,
            dark.is_some_and(|dark| dark.temperature.is_none()),
        );
        count(
            &self.temperature_differences,
            dark.is_some_and(|dark| dark.temperature_difference.is_some()),
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
            dark.is_some_and(|dark| dark.temperature == Some(TemperatureSource::Camera)),
        );
    }

    /// The counts, and what `masters` could not check, into `report`.
    pub(super) fn report_into(&self, report: &mut RunReport, masters: &CalibrationMasters) {
        report.unverified_dark_exposures = self.unverified_exposures.load(Ordering::Relaxed);
        report.unverified_dark_temperatures = self.unverified_temperatures.load(Ordering::Relaxed);
        report.dark_temperature_differences = self.temperature_differences.load(Ordering::Relaxed);
        report.scaled_darks = self.scaled_darks.load(Ordering::Relaxed);
        report.fitted_dark_scales = self.fitted_dark_scales.load(Ordering::Relaxed);
        report.camera_temperature_darks = self.camera_temperature_darks.load(Ordering::Relaxed);
        report.flat_dark = masters.flat_dark();
        report.floored_flat_pixels = masters.floored_flat_pixels() as u64;
    }
}

#[cfg(test)]
mod tests;
