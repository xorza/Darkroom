//! RAW calibration and memory-tiered registered stacking.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::CancelToken;
use rayon::prelude::*;

use crate::concurrency;
use crate::io::image::LoadContext;
use crate::io::image::cfa::{CfaFrameInfo, CfaImage};
use crate::io::image::error::ImageError;
use crate::io::image::linear::LinearImage;
use crate::io::raw;
use crate::io::raw::demosaic::DemosaicError;
use crate::stacking::calibration_masters::CalibrationMasters;
use crate::stacking::calibration_masters::cosmic_ray::reject_cosmic_rays;
use crate::stacking::combine::error::Error as StackError;
use crate::stacking::combine::stack::stack_stored_frames;
use crate::stacking::frame_store::{
    MemoryPlan, SpillDirectory, StoredImage, StoredLightFrame, compute_frame_stats, plan_memory,
    store_image, store_light_frame,
};
use crate::stacking::pipeline::align::{DetectedFrame, align_and_stack, select_reference};
use crate::stacking::pipeline::config::AlignStackConfig;
use crate::stacking::pipeline::detector_pool::DetectorPool;
use crate::stacking::pipeline::result::{AlignStackResult, Error};
use crate::stacking::progress::ProgressCallback;
use crate::stacking::registration::register;
use crate::stacking::registration::resample::warp;

/// Calibrate, align, and stack camera-RAW or mosaic-FITS light frames end to end.
///
/// For each raw light (in parallel): load it as a `CfaImage`, apply `masters`
/// (dark/flat/defect) in place, demosaic to a `LinearImage`, then hand the calibrated frames
/// to [`align_and_stack`]. A frame that fails to **load** is a hard error (bad input); a frame
/// that fails to **register** is dropped and reported in
/// [`AlignmentSummary::dropped`](crate::stacking::pipeline::result::AlignmentSummary::dropped).
///
/// For frames that are already calibrated (e.g. pre-processed FITS), skip this and call
/// [`align_and_stack`] directly.
pub fn calibrate_align_stack<P: AsRef<Path> + Sync>(
    light_paths: &[P],
    masters: &CalibrationMasters,
    config: &AlignStackConfig,
    cancel: CancelToken,
) -> Result<AlignStackResult, Error> {
    if light_paths.is_empty() {
        return Err(Error::NoFrames);
    }
    config.detection.validate()?;
    let total = light_paths.len();
    let available = config.stack.cache.get_available_memory();
    let load_context = LoadContext {
        cancel: cancel.clone(),
        ..LoadContext::default()
    };

    // Tier decision: peek the sensor dimensions (no decode) and plan the memory tier. If the warped
    // frames plus the RAM path's per-frame scratch won't fit ~75% RAM, stream through a disk cache so
    // peak RAM stays flat in the frame count.
    let frame_info =
        CfaFrameInfo::from_file(light_paths[0].as_ref(), &load_context).map_err(|source| {
            Error::Load {
                path: light_paths[0].as_ref().to_path_buf(),
                source,
            }
        })?;
    let plane_bytes = frame_info.dimensions.pixel_count() * std::mem::size_of::<f32>();
    let demosaic_memory = frame_info.demosaic.memory(frame_info.dimensions);
    let plan = plan_memory(
        plane_bytes,
        demosaic_memory,
        total,
        rayon::current_num_threads(),
        available,
    );
    if !plan.fits_in_ram {
        tracing::info!(
            frames = total,
            available_mb = available / (1024 * 1024),
            "Frame set + scratch exceeds the RAM budget — streaming via the disk cache (memory-bounded)"
        );
        return calibrate_align_stack_streaming(light_paths, masters, config, cancel, plan);
    }

    tracing::info!(
        frames = total,
        concurrency = plan.decode_concurrency,
        "Loading, calibrating and demosaicing raw lights (RAW decode — the slow phase)"
    );
    let done = AtomicUsize::new(0);
    // Bound how many frames are in flight: the RAW decode (libraw) is the one
    // uninterruptible step, so capping it caps the work a cancel must drain and
    // peak demosaic memory. The demosaic itself polls `cancel` between stages
    // (see `CfaImage::demosaic`), so the heavy phase stays interruptible at full
    // core utilization within a batch.
    let calibrated: Vec<LinearImage> =
        concurrency::try_par_map_limited(light_paths, plan.decode_concurrency, |path| {
            // Skip launching the RAW decode (the slow uninterruptible step) once cancelled.
            if cancel.is_cancelled() {
                return Err(Error::Stack(StackError::Cancelled));
            }
            let image = decode_calibrate_demosaic(path.as_ref(), masters, config, &load_context)?;
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            tracing::info!(frame = n, total, "calibrated light");
            Ok(image)
        })?;

    align_and_stack(calibrated, config, cancel)
}

/// Load one raw light, apply the calibration masters, optionally reject cosmic rays, and demosaic to
/// a `LinearImage`. The per-frame core shared by the RAM and streaming calibrate paths.
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

/// Memory-bounded `calibrate → align → stack` for sets that don't fit ~75% RAM. Spills calibrated
/// and warped frames to the shared frame store (mmap), so peak RAM is
/// `concurrency × one-frame-working-set` plus the combine's chunk window — flat in the frame count.
fn calibrate_align_stack_streaming<P: AsRef<Path> + Sync>(
    light_paths: &[P],
    masters: &CalibrationMasters,
    config: &AlignStackConfig,
    cancel: CancelToken,
    plan: MemoryPlan,
) -> Result<AlignStackResult, Error> {
    let total = light_paths.len();
    let load_context = LoadContext {
        cancel: cancel.clone(),
        ..LoadContext::default()
    };
    let spill_directory = SpillDirectory::create(
        config.stack.cache.cache_dir.clone(),
        config.stack.cache.keep_cache,
    )
    .map_err(StackError::from)?;
    let cache_dir = &spill_directory.path;
    // Fan-out for the two streaming steps was sized in `plan_memory` (decode holds the extra
    // demosaic arena, so it fans out less than the warp step). Both stream to disk, so peak RAM is
    // `concurrency × one-frame-working-set` plus the combine's chunk window — flat in the frame count.
    let MemoryPlan {
        decode_concurrency,
        warp_concurrency,
        ..
    } = plan;

    tracing::info!(
        frames = total,
        concurrency = decode_concurrency,
        "Streaming: calibrating + detecting (spilling to disk)"
    );
    let done = AtomicUsize::new(0);
    let indexed: Vec<(usize, &P)> = light_paths.iter().enumerate().collect();
    let mut detected: Vec<DetectedFrame<StoredImage>> = {
        let mut detectors =
            DetectorPool::from_config(&config.detection, decode_concurrency.min(total))?;
        detectors.try_map(&indexed, |detector, &(idx, ref path)| {
            if cancel.is_cancelled() {
                return Err(Error::Stack(StackError::Cancelled));
            }
            let image = decode_calibrate_demosaic(path.as_ref(), masters, config, &load_context)?;
            let stars = detector.detect(&image).stars;
            let stored = store_image(cache_dir, &format!("calib_{idx}"), &image)
                .map_err(StackError::from)?;
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            tracing::info!(
                frame = n,
                total,
                stars = stars.len(),
                "calibrated + detected"
            );
            Ok(DetectedFrame {
                image: stored,
                stars,
            })
        })
    }?;
    if cancel.is_cancelled() {
        return Err(Error::Stack(StackError::Cancelled));
    }

    let dimensions = detected[0].image.dimensions;
    let star_counts: Vec<usize> = detected.iter().map(|d| d.stars.len()).collect();
    let reference = select_reference(
        &star_counts,
        config.reference,
        config
            .registration
            .matching
            .required_stars(config.registration.transform_type),
    )?;
    let metadata = detected[reference].image.metadata.clone();
    let ref_stars = std::mem::take(&mut detected[reference].stars);
    tracing::info!(
        reference,
        ref_stars = ref_stars.len(),
        "Reference selected (streaming)"
    );

    tracing::info!(
        frames = total - 1,
        "Streaming: registering + warping (spilling to disk)"
    );
    let registered_so_far = AtomicUsize::new(0);
    let mut outcomes: Vec<Option<StoredLightFrame>> = Vec::with_capacity(total);
    let mut pending = detected.into_iter().enumerate();
    loop {
        let batch: Vec<_> = pending.by_ref().take(warp_concurrency).collect();
        if batch.is_empty() {
            break;
        }
        let batch_outcomes: Result<Vec<Option<StoredLightFrame>>, Error> = batch
            .into_par_iter()
            .map(|(idx, detected)| {
                if cancel.is_cancelled() {
                    return Ok(None);
                }
                let calibrated = detected.image.load();
                let source_stats = compute_frame_stats(&calibrated);
                let name = format!("warped_{idx}");
                if idx == reference {
                    return store_light_frame(
                        cache_dir,
                        &name,
                        calibrated,
                        None,
                        None,
                        source_stats,
                    )
                    .map(Some)
                    .map_err(StackError::from)
                    .map_err(Error::Stack);
                }

                let n = registered_so_far.fetch_add(1, Ordering::Relaxed) + 1;
                let registration = match register(&ref_stars, &detected.stars, &config.registration)
                {
                    Ok(registration) => registration,
                    Err(error) => {
                        tracing::info!(frame = n, total = total - 1, %error, "registration failed");
                        return Ok(None);
                    }
                };
                let warped = warp(
                    &calibrated,
                    &registration.warp_transform(),
                    &config.registration.warp,
                );
                tracing::info!(
                    frame = n,
                    total = total - 1,
                    inliers = registration.num_inliers(),
                    "registered (streaming)"
                );
                store_light_frame(
                    cache_dir,
                    &name,
                    warped.image,
                    Some(warped.coverage),
                    Some(warped.confidence),
                    source_stats,
                )
                .map(Some)
                .map_err(StackError::from)
                .map_err(Error::Stack)
            })
            .collect();
        outcomes.extend(batch_outcomes?);
    }
    if cancel.is_cancelled() {
        return Err(Error::Stack(StackError::Cancelled));
    }

    // `try_par_map_limited` preserves input order, so the position is the frame index.
    let mut frames = Vec::with_capacity(outcomes.len());
    let mut dropped = Vec::new();
    for (idx, outcome) in outcomes.into_iter().enumerate() {
        match outcome {
            Some(frame) => frames.push(frame),
            None => dropped.push(idx),
        }
    }
    dropped.sort_unstable();
    if frames.len() <= 1 && total > 1 {
        return Err(Error::AllFramesDropped { count: total - 1 });
    }

    let registered = frames.len();
    tracing::info!(
        frames = registered,
        dropped = dropped.len(),
        "Streaming: combining (mmap)"
    );
    let stacked = stack_stored_frames(
        frames,
        Some(spill_directory),
        dimensions,
        metadata,
        config.stack.clone(),
        ProgressCallback::default(),
        cancel,
    )
    .map_err(Error::Stack)?;

    Ok(AlignStackResult::from_product(
        stacked, reference, registered, dropped,
    ))
}
