//! RAW calibration front end for registered stacking.

use std::path::Path;

use common::CancelToken;

use crate::io::cancelled::Cancelled;
use crate::io::image::cfa::{CfaFrameInfo, CfaImage};
use crate::io::image::error::ImageError;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::linear::LinearImage;
use crate::io::image::load_context::LoadContext;
use crate::memory::run_memory::RunMemory;
use crate::memory::{MemoryPlan, PerFrameBytes, RunShape};
use crate::stacking::calibration_masters::CalibrationMasters;
use crate::stacking::calibration_masters::cosmic_ray::reject_cosmic_rays;
use crate::stacking::combine::error::Error as StackError;
use crate::stacking::pipeline::align::{log_detection, register_warp_and_stack};
use crate::stacking::pipeline::config::AlignStackConfig;
use crate::stacking::pipeline::detector_pool::DetectorPool;
use crate::stacking::pipeline::frame::DetectedFrame;
use crate::stacking::pipeline::result::{AlignStackResult, Error};
use crate::stacking::pipeline::tier::FrameTier;
use crate::stacking::progress::stage_counter::StageCounter;
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
    // Sample the machine once, here, and hand the reading to every stage below, so the tier
    // decision, the chunk sizes and the decode ceiling are derived from the same figure.
    let memory = RunMemory::read(config.stack.cache.memory_override);
    let load_context = memory.load_context(cancel.clone());

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
    let plan = MemoryPlan::plan(
        RunShape {
            frame_count: total,
            decode: demosaic,
            warp: Some(PerFrameBytes::new(plane_bytes, demosaic)),
            output_bytes: config.stack.quality.resident_bytes(output),
        },
        rayon::current_num_threads(),
        memory.planning(),
    );
    let tier = FrameTier::for_plan(&plan, &config.stack.cache, memory)?;

    tracing::info!(
        frames = total,
        planning_mb = memory.planning() / (1024 * 1024),
        concurrency = plan.decode_concurrency,
        spilling = tier.spills(),
        "Loading, calibrating and demosaicing raw lights (RAW decode — the slow phase)"
    );
    // Bound how many frames are in flight: the RAW decode (libraw) is the one uninterruptible
    // step, so capping it caps the work a cancel must drain and peak demosaic memory. The
    // demosaic itself polls `cancel` between stages (see `CfaImage::demosaic`), so the heavy
    // phase stays interruptible at full core utilization within a batch.
    let done = StageCounter::new(&progress, StackingStage::Preparing, total);
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
            let n = done.complete_one();
            log_detection(n, total, &result);
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
                source: Box::new(source),
            });
        }
    };
    masters.calibrate(&mut cfa)?;
    if let Some(cr) = &config.cosmic_ray {
        // Dispatched per CFA type inside `reject_cosmic_rays` (mono / Bayer-deinterleave /
        // X-Trans same-color).
        let removed = reject_cosmic_rays(&mut cfa, cr);
        tracing::info!(removed, "rejected cosmic rays");
    }
    // Demosaic is the other heavy step; it polls `cancel` internally and bails mid-pass.
    cfa.demosaic(&context.cancel)
        .map_err(|Cancelled| Error::Stack(StackError::Cancelled))
}
