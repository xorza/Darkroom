//! RAW calibration front end for registered stacking.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use common::CancelToken;

use crate::calibration_masters::CalibrationMasters;
use crate::calibration_masters::calibration_outcome::CalibrationOutcome;
use crate::ingest::ingest_run::IngestRun;
use crate::pipeline::align::register_warp_and_stack;
use crate::pipeline::config::{AlignStackConfig, Reference};
use crate::pipeline::error::AlignStackError;
use crate::pipeline::light_source::{LightSource, RawLights};
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
/// With [`Reference::Index`] the reference is prepared first, and each other light then goes
/// through its preparation, its registration and its warp in one pass, written once. With
/// [`Reference::Auto`] every light is prepared and detected first, since the reference is the
/// sharpest one, and parked until it registers.
///
/// The sensor geometry is peeked from the first frame's header without a decode, so the memory
/// tier is chosen before any pixels are read. When the frame set plus its per-frame scratch
/// won't fit the budget, every parked and warped frame goes through the frame store's memory maps
/// and peak RAM stays flat in the frame count.
///
/// For frames that are already calibrated (e.g. pre-processed FITS), skip this and call
/// [`align_and_stack`](crate::pipeline::align::align_and_stack) directly.
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
    let run = IngestRun::new(&config.stack.ingest, cancel.clone());
    let notes = CalibrationNotes::default();
    let lights = RawLights {
        paths: light_paths,
        masters,
        cosmic_ray: config.cosmic_ray.as_ref(),
        notes: &notes,
    };
    let mut result = match config.reference {
        Reference::Index(reference) => {
            lights.stack_in_one_pass(reference, config, &run, progress)?
        }
        Reference::Auto => {
            let detected = LightSource::Raw(lights).detect(config, &run, &progress)?;
            register_warp_and_stack(detected.frames, config, detected.stage, progress, cancel)?
        }
    };
    notes.report_into(&mut result.product.report, masters);
    Ok(result)
}

/// What calibrating the lights could not check, counted across the workers.
#[derive(Debug, Default)]
pub(crate) struct CalibrationNotes {
    unverified_exposures: AtomicU64,
    unverified_temperatures: AtomicU64,
    scaled_darks: AtomicU64,
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
        count(&self.scaled_darks, outcome.dark_scale.is_some());
    }

    fn report_into(&self, report: &mut RunReport, masters: &CalibrationMasters) {
        report.unverified_dark_exposures = self.unverified_exposures.load(Ordering::Relaxed);
        report.unverified_dark_temperatures = self.unverified_temperatures.load(Ordering::Relaxed);
        report.scaled_darks = self.scaled_darks.load(Ordering::Relaxed);
        report.unverified_flat_dark = masters.unverified_flat_dark();
        report.floored_flat_pixels = masters.floored_flat_pixels() as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::image::unverified_conditions::UnverifiedConditions;

    /// Three lights: one unverified in exposure, one in temperature and scaled, one clean. The
    /// report counts each fact once per light that met it, and carries the bundle's floored flat
    /// pixels and unverified flat-dark, here none.
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
            dark_scale: Some(2.5),
        });
        notes.record(CalibrationOutcome::default());
        let mut report = RunReport::default();
        notes.report_into(&mut report, &CalibrationMasters::default());
        assert_eq!(
            (
                report.unverified_dark_exposures,
                report.unverified_dark_temperatures,
                report.scaled_darks,
                report.floored_flat_pixels,
                report.unverified_flat_dark
            ),
            (1, 1, 1, 0, UnverifiedConditions::NONE)
        );
    }
}
