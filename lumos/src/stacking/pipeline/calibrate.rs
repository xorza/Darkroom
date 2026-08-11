//! RAW calibration front end for registered stacking.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::CancelToken;

use crate::io::image::cfa::{CfaFrameInfo, CfaImage};
use crate::io::image::error::ImageError;
use crate::io::image::linear::LinearImage;
use crate::io::image::load_context::LoadContext;
use crate::io::raw;
use crate::io::raw::demosaic::DemosaicError;
use crate::memory;
use crate::memory::MemoryPlan;
use crate::stacking::calibration_masters::CalibrationMasters;
use crate::stacking::calibration_masters::cosmic_ray::reject_cosmic_rays;
use crate::stacking::combine::error::Error as StackError;
use crate::stacking::pipeline::align::{log_detection, register_warp_and_stack};
use crate::stacking::pipeline::config::AlignStackConfig;
use crate::stacking::pipeline::detector_pool::DetectorPool;
use crate::stacking::pipeline::frame::DetectedFrame;
use crate::stacking::pipeline::result::{AlignStackResult, Error};
use crate::stacking::pipeline::tier::FrameTier;
use crate::stacking::progress::{ProgressCallback, StackingStage};

/// Calibrate, align, and stack camera-RAW or mosaic-FITS light frames end to end.
///
/// For each raw light: load it as a `CfaImage`, apply `masters` (dark/flat/defect) in place,
/// demosaic to a `LinearImage`, and detect its stars — then hand the detected frames to the
/// shared register → warp → combine body. A frame that fails to **load** is a hard error (bad
/// input); a frame that fails to **register** is dropped and reported in
/// [`AlignmentSummary::dropped`](crate::stacking::pipeline::result::AlignmentSummary::dropped).
///
/// The sensor geometry is peeked from the first frame's header without a decode, so the memory
/// tier is chosen before any pixels are read. When the frame set plus its per-frame scratch
/// won't fit the budget, every calibrated and warped frame goes through the frame store's
/// memory maps and peak RAM stays flat in the frame count.
///
/// For frames that are already calibrated (e.g. pre-processed FITS), skip this and call
/// [`align_and_stack`](crate::stacking::pipeline::align::align_and_stack) directly.
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
    config.validate()?;
    let total = light_paths.len();
    // Sample the machine once, here, and hand the resolved config to every stage below, so the
    // tier decision and the decode budget are derived from the same figure. An unresolved config
    // samples lazily and `LoadContext::default()` samples again, which can disagree.
    let system_available = memory::available_memory();
    let config = &config.with_resolved_memory(system_available);
    let available = config.stack.cache.planning_memory();
    // From the system reading, not `available`: the config's figure is a tier-planning override
    // and must not shrink what a single FITS decode may allocate.
    let load_context = LoadContext::new(cancel.clone(), memory::memory_budget(system_available));

    // Peek the sensor dimensions (no decode) so the tier is decided before any frame is read.
    let frame_info =
        CfaFrameInfo::from_file(light_paths[0].as_ref(), &load_context).map_err(|source| {
            Error::Load {
                path: light_paths[0].as_ref().to_path_buf(),
                source,
            }
        })?;
    let plan = MemoryPlan::plan(
        frame_info.dimensions.pixel_count() * size_of::<f32>(),
        frame_info.demosaic.memory(frame_info.dimensions),
        total,
        rayon::current_num_threads(),
        available,
    );
    let tier = FrameTier::for_plan(&plan, &config.stack.cache)?;

    tracing::info!(
        frames = total,
        available_mb = available / (1024 * 1024),
        concurrency = plan.decode_concurrency,
        spilling = tier.spills(),
        "Loading, calibrating and demosaicing raw lights (RAW decode — the slow phase)"
    );
    // Bound how many frames are in flight: the RAW decode (libraw) is the one uninterruptible
    // step, so capping it caps the work a cancel must drain and peak demosaic memory. The
    // demosaic itself polls `cancel` between stages (see `CfaImage::demosaic`), so the heavy
    // phase stays interruptible at full core utilization within a batch.
    let done = AtomicUsize::new(0);
    let detected: Vec<DetectedFrame> = {
        let mut detectors =
            DetectorPool::from_config(&config.detection, plan.decode_concurrency.min(total))
                .map_err(Error::DetectionConfig)?;
        detectors.try_map(light_paths, |detector, index, path| {
            // Skip launching the RAW decode (the slow uninterruptible step) once cancelled.
            if cancel.is_cancelled() {
                return Err(Error::Stack(StackError::Cancelled));
            }
            let image = decode_calibrate_demosaic(path.as_ref(), masters, config, &load_context)?;
            // Detect while the decoded frame is still in hand, so the spilled tier reads it back
            // once (for the warp) rather than twice.
            let result = detector.detect(&image);
            let image = tier.hold(&format!("calib_{index}"), image)?;
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            log_detection(n, total, &result);
            progress.report(n, total, StackingStage::Preparing);
            Ok(DetectedFrame {
                image,
                stars: result.stars,
                diagnostics: result.diagnostics,
            })
        })
    }?;

    register_warp_and_stack(
        detected,
        config,
        tier,
        plan.warp_concurrency,
        progress,
        cancel,
    )
}

/// Load one raw light, apply the calibration masters, optionally reject cosmic rays, and
/// demosaic to a `LinearImage`.
fn decode_calibrate_demosaic(
    path: &Path,
    masters: &CalibrationMasters,
    config: &AlignStackConfig,
    context: &LoadContext,
) -> Result<LinearImage, Error> {
    let mut cfa = match CfaImage::from_file(path, context) {
        Ok(image) => image,
        Err(ImageError::Cancelled { .. }) => {
            return Err(Error::Stack(StackError::Cancelled));
        }
        Err(source) => {
            return Err(Error::Load {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    masters.calibrate(&mut cfa)?;
    if let Some(cr) = &config.cosmic_ray {
        // Dispatched per CFA type inside `reject_cosmic_rays` (mono / Bayer-deinterleave /
        // X-Trans same-color). Only an unlabeled frame is skipped — its pattern is unknown, so any
        // same-color/Laplacian stencil could corrupt a mislabeled mosaic.
        match &cfa.metadata.cfa_type {
            Some(_) => {
                let removed = reject_cosmic_rays(&mut cfa, cr);
                tracing::info!(removed, "rejected cosmic rays");
            }
            None => tracing::warn!("frame has no CFA pattern; skipping cosmic-ray rejection"),
        }
    }
    // Demosaic is the other heavy step; it polls `cancel` internally and bails mid-pass.
    cfa.demosaic(&context.cancel)
        .map_err(|source| match source {
            DemosaicError::Cancelled => Error::Stack(StackError::Cancelled),
            DemosaicError::InvalidXTransPattern(source) => Error::Load {
                path: path.to_path_buf(),
                source: raw::raw_err(path, source.to_string()),
            },
        })
}
