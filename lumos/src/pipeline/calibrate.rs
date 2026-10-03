//! RAW calibration front end for registered stacking.

use std::path::Path;

use common::CancelToken;

use std::sync::atomic::{AtomicU64, Ordering};

use crate::calibration_masters::CalibrationMasters;
use crate::calibration_masters::calibration_outcome::CalibrationOutcome;
use crate::calibration_masters::cosmic_ray;
use crate::calibration_masters::cosmic_ray::config::CosmicRayConfig;
use crate::calibration_masters::cosmic_ray::reject_cosmic_rays;
use crate::combine::error::Error as StackError;
use crate::error::FrameDimensionMismatch;
use crate::frame_store::frame_stats::FrameStats;
use crate::ingest::ingest_run::IngestRun;
use crate::io::cancelled::Cancelled;
use crate::io::image::cfa::{CfaFrameInfo, CfaImage};
use crate::io::image::error::ImageError;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::linear::LinearImage;
use crate::io::image::load_context::LoadContext;
use crate::memory::{
    DECODE_TRANSIENT_FACTOR, DETECTION_WORKING_PLANES, MemoryPlan, PerFrameBytes, RunShape,
};
use crate::pipeline::align::{log_detection, register_warp_and_stack};
use crate::pipeline::config::AlignStackConfig;
use crate::pipeline::detector_pool::DetectorPool;
use crate::pipeline::frame::DetectedFrame;
use crate::pipeline::result::{AlignStackResult, Error};
use crate::pipeline::tier::StagePlan;
use crate::progress::stage_counter::StageCounter;
use crate::progress::{ProgressCallback, StackingStage};
use crate::run_report::RunReport;

/// Calibrate, align, and stack camera-RAW or mosaic-FITS light frames end to end.
///
/// For each raw light: load it as a `CfaImage`, apply `masters` (dark/flat/defect) in place,
/// demosaic to a `LinearImage`, and detect its stars — then hand the detected frames to the
/// shared register → warp → combine body. A frame that fails to **load** is a hard error (bad
/// input); a frame that fails to **register** is dropped and reported in
/// [`AlignmentSummary::dropped`](crate::pipeline::result::AlignmentSummary::dropped).
///
/// The sensor geometry is peeked from the first frame's header without a decode, so the memory
/// tier is chosen before any pixels are read. When the frame set plus its per-frame scratch
/// won't fit the budget, every calibrated and warped frame goes through the frame store's
/// memory maps and peak RAM stays flat in the frame count.
///
/// For frames that are already calibrated (e.g. pre-processed FITS), skip this and call
/// [`align_and_stack`](crate::pipeline::align::align_and_stack) directly.
pub fn calibrate_align_stack<P: AsRef<Path> + Sync>(
    light_paths: &[P],
    masters: &CalibrationMasters,
    config: &AlignStackConfig,
    progress: ProgressCallback,
    cancel: CancelToken,
) -> Result<AlignStackResult, Error> {
    if light_paths.is_empty() {
        return Err(Error::NoFrames);
    }
    config.validate(light_paths.len())?;
    let total = light_paths.len();
    // Sample the machine once, here, and hand the reading to every stage below, so the tier
    // decision, the chunk sizes and the decode ceiling are derived from the same figure.
    let IngestRun {
        memory,
        context: load_context,
    } = IngestRun::new(&config.stack.ingest, cancel.clone());

    // Peek the sensor dimensions (no decode) so the tier is decided before any frame is read.
    let frame_info =
        CfaFrameInfo::from_file(light_paths[0].as_ref(), &load_context).map_err(|source| {
            Error::Load {
                path: light_paths[0].as_ref().to_path_buf(),
                source: Box::new(source),
            }
        })?;
    let plane_bytes = frame_info.dimensions.pixel_count() * size_of::<f32>();
    let demosaic = frame_info.cfa_type.demosaic_memory(frame_info.dimensions);
    let output = ImageDimensions::new(
        frame_info.dimensions.size(),
        frame_info.cfa_type.num_colors(),
    );
    // One frame's pass peaks at the largest of: the demosaic, the cosmic-ray pass over the mosaic,
    // and the statistics — a copy of every channel beside the demosaiced frame.
    let cosmic_ray = config.cosmic_ray.as_ref().map_or(0, |_| {
        plane_bytes + cosmic_ray::heap_bytes(&frame_info.cfa_type, frame_info.dimensions.size())
    });
    let decode = demosaic
        .with_peak_at_least(cosmic_ray)
        .with_peak_at_least(DECODE_TRANSIENT_FACTOR * demosaic.output_bytes);
    let plan = MemoryPlan::plan(
        RunShape {
            frame_count: total,
            decode,
            detection_bytes: DETECTION_WORKING_PLANES * plane_bytes,
            warp: Some(PerFrameBytes::new(plane_bytes, demosaic.output_bytes)),
            output_bytes: config.stack.quality.resident_bytes(output),
        },
        rayon::current_num_threads(),
        memory.planning(),
    );
    let stage = StagePlan::new(&plan, &config.stack.ingest, memory)?;

    tracing::info!(
        frames = total,
        planning_mb = memory.planning() / (1024 * 1024),
        concurrency = plan.decode_concurrency,
        spilling = stage.tier.spills(),
        "Loading, calibrating and demosaicing raw lights (RAW decode — the slow phase)"
    );
    // Bound how many frames are in flight: the RAW decode (libraw) is the one uninterruptible
    // step, so capping it caps the work a cancel must drain and peak demosaic memory. The
    // demosaic itself polls `cancel` between stages (see `CfaImage::demosaic`), so the heavy
    // phase stays interruptible at full core utilization within a batch.
    let done = StageCounter::new(&progress, StackingStage::Preparing, total);
    let notes = CalibrationNotes::default();
    let detected: Vec<DetectedFrame> = {
        let mut detectors =
            DetectorPool::from_config(&config.detection, plan.decode_concurrency.min(total))
                .map_err(Error::DetectionConfig)?;
        detectors.try_map(light_paths, |detector, index, path| {
            // Skip launching the RAW decode (the slow uninterruptible step) once cancelled.
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let image = decode_calibrate_demosaic(
                path.as_ref(),
                masters,
                config.cosmic_ray.as_ref(),
                &load_context,
                &notes,
            )?;
            // Checked here, at decode, rather than after every frame has been detected: a frame
            // from another sensor fails the run before the rest are paid for.
            FrameDimensionMismatch::check(index, output, image.dimensions())
                .map_err(|mismatch| Error::from(StackError::from(mismatch)))?;
            // Detect while the decoded frame is still in hand, so the spilled tier reads it back
            // once (for the warp) rather than twice.
            let result = detector.detect(&image);
            // Measured here, before interpolation correlates neighbouring pixels and would
            // understate the frame's noise, and while the frame is in hand: a spilled frame would
            // otherwise be read back for it.
            let stats = FrameStats::measure(&image);
            let image = stage.tier.hold(&format!("calib_{index}"), image)?;
            let n = done.complete_one();
            log_detection(n, total, &result);
            Ok(DetectedFrame {
                image,
                stars: result.stars,
                diagnostics: result.diagnostics,
                stats,
            })
        })
    }?;

    let mut result = register_warp_and_stack(detected, config, stage, progress, cancel)?;
    notes.report_into(&mut result.product.report, masters);
    Ok(result)
}

/// What calibrating the lights could not check, counted across the workers.
#[derive(Debug, Default)]
struct CalibrationNotes {
    unverified_exposures: AtomicU64,
    unverified_temperatures: AtomicU64,
    scaled_darks: AtomicU64,
}

impl CalibrationNotes {
    fn record(&self, outcome: CalibrationOutcome) {
        let count = |counter: &AtomicU64, happened: bool| {
            counter.fetch_add(u64::from(happened), Ordering::Relaxed);
        };
        count(&self.unverified_exposures, outcome.unverified_exposure);
        count(
            &self.unverified_temperatures,
            outcome.unverified_temperature,
        );
        count(&self.scaled_darks, outcome.dark_scale.is_some());
    }

    fn report_into(&self, report: &mut RunReport, masters: &CalibrationMasters) {
        report.unverified_dark_exposures = self.unverified_exposures.load(Ordering::Relaxed);
        report.unverified_dark_temperatures = self.unverified_temperatures.load(Ordering::Relaxed);
        report.scaled_darks = self.scaled_darks.load(Ordering::Relaxed);
        report.floored_flat_pixels = masters.floored_flat_pixels() as u64;
    }
}

/// Load one raw light, apply the calibration masters, optionally reject cosmic rays, and
/// demosaic to a `LinearImage`.
fn decode_calibrate_demosaic(
    path: &Path,
    masters: &CalibrationMasters,
    cosmic_ray: Option<&CosmicRayConfig>,
    context: &LoadContext,
    notes: &CalibrationNotes,
) -> Result<LinearImage, Error> {
    let mut cfa = match CfaImage::from_file(path, context) {
        Ok(image) => image,
        Err(ImageError::Cancelled { .. }) => {
            return Err(Error::Cancelled);
        }
        Err(source) => {
            return Err(Error::Load {
                path: path.to_path_buf(),
                source: Box::new(source),
            });
        }
    };
    notes.record(masters.calibrate(&mut cfa)?);
    if let Some(cosmic_ray) = cosmic_ray {
        // Dispatched per CFA type inside `reject_cosmic_rays` (mono / Bayer-deinterleave /
        // X-Trans same-color).
        let removed =
            reject_cosmic_rays(&mut cfa, cosmic_ray).map_err(|source| Error::CosmicRay {
                path: path.to_path_buf(),
                source,
            })?;
        tracing::info!(removed, "rejected cosmic rays");
    }
    // Demosaic is the other heavy step; it polls `cancel` internally and bails mid-pass.
    cfa.demosaic(&context.cancel)
        .map_err(|Cancelled| Error::Cancelled)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Three lights: one unverified in exposure, one in temperature and scaled, one clean. The
    /// report counts each fact once per light that met it, and carries the bundle's floored flat
    /// pixels, here none.
    #[test]
    fn the_notes_count_what_calibration_could_not_check() {
        let notes = CalibrationNotes::default();
        notes.record(CalibrationOutcome {
            unverified_exposure: true,
            ..CalibrationOutcome::default()
        });
        notes.record(CalibrationOutcome {
            unverified_temperature: true,
            dark_scale: Some(2.5),
            ..CalibrationOutcome::default()
        });
        notes.record(CalibrationOutcome::default());
        let mut report = RunReport::default();
        notes.report_into(&mut report, &CalibrationMasters::default());
        assert_eq!(
            (
                report.unverified_dark_exposures,
                report.unverified_dark_temperatures,
                report.scaled_darks,
                report.floored_flat_pixels
            ),
            (1, 1, 1, 0)
        );
    }
}
