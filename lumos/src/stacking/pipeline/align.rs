//! Detection, registration, warping, and combination of calibrated images.

use std::sync::atomic::{AtomicUsize, Ordering};

use common::CancelToken;

use crate::concurrency;
use crate::io::image::linear::LinearImage;
use crate::io::raw::demosaic::DemosaicMemory;
use crate::stacking::combine::error::Error as StackError;
use crate::stacking::combine::stack::stack_stored_frames;
use crate::stacking::frame_store::{StoredFrame, compute_frame_stats, plan_memory};
use crate::stacking::progress::ProgressCallback;
use crate::stacking::registration::register;
use crate::stacking::registration::resample::warp;
use crate::stacking::star_detection::detector::DetectionResult;

use crate::stacking::pipeline::config::{AlignStackConfig, Reference};
use crate::stacking::pipeline::detector_pool::DetectorPool;
use crate::stacking::pipeline::frame::{DetectedFrame, PipelineFrame};
use crate::stacking::pipeline::result::{AlignStackResult, Error};
use crate::stacking::pipeline::tier::FrameTier;

/// Detect → register → warp → stack a set of light frames into one aligned, combined image.
///
/// All frames are expected to share the same dimensions (same sensor). The reference frame is
/// added to the stack unwarped; every other frame is aligned to it. Frames that fail to
/// register (too few stars, RANSAC failure, accuracy gate) are dropped and listed in
/// [`AlignmentSummary::dropped`](crate::stacking::pipeline::result::AlignmentSummary::dropped);
/// the stack proceeds with whatever aligned. A single input frame is returned as its own "stack".
///
/// The frames arrive decoded and resident, so the inputs are committed before this is called —
/// but the warped outputs are a second full set, and those spill to the frame store when they
/// would not fit alongside the inputs. For camera RAW, enter through
/// [`calibrate_align_stack`](crate::stacking::pipeline::calibrate::calibrate_align_stack)
/// instead, which tiers the decode as well.
pub fn align_and_stack(
    lights: Vec<LinearImage>,
    config: &AlignStackConfig,
    cancel: CancelToken,
) -> Result<AlignStackResult, Error> {
    if lights.is_empty() {
        return Err(Error::NoFrames);
    }
    config.detection.validate()?;

    let total = lights.len();
    // The inputs are already decoded and resident, so only the warped outputs and the per-frame
    // scratch are still in question; charge the decode as done by giving it no transient arena.
    let frame_bytes = lights[0].dimensions().sample_count() * size_of::<f32>();
    let plan = plan_memory(
        lights[0].dimensions().pixel_count() * size_of::<f32>(),
        DemosaicMemory {
            output_bytes: frame_bytes,
            peak_bytes: frame_bytes,
        },
        total,
        rayon::current_num_threads(),
        config.stack.cache.get_available_memory(),
    );
    let tier = FrameTier::for_plan(&plan, &config.stack.cache)?;

    tracing::info!(frames = total, spilling = tier.spills(), "Detecting stars");
    let detected_count = AtomicUsize::new(0);
    let stars = {
        let mut detectors =
            DetectorPool::from_config(&config.detection, total.min(rayon::current_num_threads()))?;
        detectors.try_map(&lights, |detector, image| {
            // Cancelled: abort the batch rather than spend the rest of the budget detecting
            // frames the run will discard.
            if cancel.is_cancelled() {
                return Err(Error::Stack(StackError::Cancelled));
            }
            let result = detector.detect(image);
            let n = detected_count.fetch_add(1, Ordering::Relaxed) + 1;
            log_detection(n, total, &result);
            Ok(result.stars)
        })
    }?;

    // Resident whatever the tier: these frames are already in RAM, and spilling them here would
    // be a write and a read-back for nothing. The tier governs the *warped* set below, which is
    // the one that would otherwise double the footprint.
    let detected: Vec<DetectedFrame> = lights
        .into_iter()
        .zip(stars)
        .map(|(image, stars)| DetectedFrame {
            image: PipelineFrame::Resident(image),
            stars,
        })
        .collect();

    register_warp_and_stack(detected, config, tier, plan.warp_concurrency, cancel)
}

/// The detection funnel — candidates → deblended → centroided → kept — shows how confidently
/// the frame resolved into usable stars. Shared so both front ends report the same numbers.
pub(crate) fn log_detection(frame: usize, total: usize, result: &DetectionResult) {
    let diagnostics = &result.diagnostics;
    tracing::info!(
        frame,
        total,
        candidates = diagnostics.candidates_after_filtering,
        deblended = diagnostics.deblended_components,
        measured = diagnostics.stars_after_centroid,
        stars = result.stars.len(),
        "detected stars"
    );
}

/// Register every frame to the chosen reference, warp it, and combine the survivors.
///
/// The single body behind both entry points. The front ends differ only in how a frame becomes
/// a [`DetectedFrame`] — already decoded, or decoded and calibrated from a path — and `tier`
/// decides whether a warped output stays resident or goes to the frame store. Everything from
/// reference selection onward is the same work either way.
pub(crate) fn register_warp_and_stack(
    mut detected: Vec<DetectedFrame>,
    config: &AlignStackConfig,
    tier: FrameTier,
    warp_concurrency: usize,
    cancel: CancelToken,
) -> Result<AlignStackResult, Error> {
    let total = detected.len();
    if cancel.is_cancelled() {
        return Err(Error::Stack(StackError::Cancelled));
    }

    // Fail before spending any registration work on a set that can't combine. `warp` reprojects
    // into the *source* frame's grid, not the reference's, so mismatched inputs would otherwise
    // reach the combine as differently-sized planes rather than as an error.
    let expected = detected[0].image.dimensions();
    if let Some((index, frame)) = detected
        .iter()
        .enumerate()
        .find(|(_, frame)| frame.image.dimensions() != expected)
    {
        return Err(Error::Stack(StackError::DimensionMismatch {
            index,
            expected,
            actual: frame.image.dimensions(),
        }));
    }

    let star_counts: Vec<usize> = detected.iter().map(|frame| frame.stars.len()).collect();
    let reference = select_reference(
        &star_counts,
        config.reference,
        config
            .registration
            .matching
            .required_stars(config.registration.transform_type),
    )?;
    // The master follows the alignment anchor rather than whichever frame reaches combine first.
    let metadata = detected[reference].image.metadata().clone();
    let dimensions = detected[reference].image.dimensions();
    let ref_stars = std::mem::take(&mut detected[reference].stars);
    tracing::info!(
        reference,
        ref_stars = ref_stars.len(),
        "Reference frame selected"
    );

    tracing::info!(frames = total - 1, "Registering frames to the reference");
    let registered_so_far = AtomicUsize::new(0);
    // Taking each detected record by value frees its input image as soon as the warped output
    // exists, so this stage never holds the complete input and warped sets simultaneously.
    let outcomes = concurrency::try_par_map_limited_owned(
        detected,
        warp_concurrency,
        |index, detected| -> Result<Option<StoredFrame>, Error> {
            // Cancelled: drop this frame (skips the heavy register + warp); the post-loop check
            // below turns the run into `Cancelled`.
            if cancel.is_cancelled() {
                return Ok(None);
            }
            let name = format!("warped_{index}");
            if index == reference {
                // The unwarped reference has full support and unit interpolation confidence.
                let image = detected.image.into_image();
                let source_stats = compute_frame_stats(&image);
                return tier.store(&name, image, None, None, source_stats).map(Some);
            }

            let n = registered_so_far.fetch_add(1, Ordering::Relaxed) + 1;
            let source = detected.image.into_image();
            // Measured before interpolation, which correlates neighbouring pixels and would
            // otherwise understate the frame's noise.
            let source_stats = compute_frame_stats(&source);
            let registration = match register(&ref_stars, &detected.stars, &config.registration) {
                Ok(registration) => registration,
                Err(error) => {
                    tracing::info!(frame = n, total = total - 1, %error, "registration failed");
                    return Ok(None);
                }
            };
            tracing::info!(
                frame = n,
                total = total - 1,
                inliers = registration.num_inliers(),
                rms = format!("{:.3}", registration.rms_error()),
                quality = format!("{:.3}", registration.quality_score()),
                transform = %registration.transform(),
                "registered"
            );
            let warped = warp(
                &source,
                &registration.warp_transform(),
                &config.registration.warp,
            );
            drop(source);
            tier.store(
                &name,
                warped.image,
                Some(warped.coverage),
                Some(warped.confidence),
                source_stats,
            )
            .map(Some)
        },
    )?;
    if cancel.is_cancelled() {
        return Err(Error::Stack(StackError::Cancelled));
    }

    let mut frames = Vec::with_capacity(outcomes.len());
    let mut dropped = Vec::new();
    // Ascending without a sort: the bounded map preserves input order, so this visits outcomes
    // by frame index — the ordering `AlignmentSummary::dropped` documents.
    for (index, outcome) in outcomes.into_iter().enumerate() {
        match outcome {
            Some(frame) => frames.push(frame),
            None => dropped.push(index),
        }
    }
    tracing::info!(
        aligned = frames.len(),
        dropped = dropped.len(),
        "Registration complete"
    );

    // Only the reference survived → every non-reference frame dropped. (A lone reference input
    // is fine; "nothing aligned" with more than one input is an error.)
    if frames.len() <= 1 && total > 1 {
        return Err(Error::AllFramesDropped { count: total - 1 });
    }

    let registered = frames.len();
    tracing::info!(frames = registered, "Stacking aligned frames");
    let stacked = stack_stored_frames(
        frames,
        tier.into_spill_directory(),
        dimensions,
        metadata,
        config.stack.clone(),
        ProgressCallback::default(),
        cancel,
    )?;
    tracing::info!("Stack complete");

    Ok(AlignStackResult::from_product(
        stacked, reference, registered, dropped,
    ))
}

/// Choose the reference (alignment anchor) index from per-frame star counts, validating it has
/// enough stars.
fn select_reference(
    star_counts: &[usize],
    reference: Reference,
    required: usize,
) -> Result<usize, Error> {
    let index = match reference {
        Reference::Index(index) => {
            if index >= star_counts.len() {
                return Err(Error::ReferenceOutOfRange {
                    index,
                    count: star_counts.len(),
                });
            }
            index
        }
        // Most stars → most anchors for the other frames to match against.
        Reference::Auto => (0..star_counts.len())
            .max_by_key(|&i| star_counts[i])
            .expect("star_counts is non-empty"),
    };
    if star_counts[index] < required {
        return Err(Error::ReferenceInsufficientStars {
            index,
            found: star_counts[index],
            required,
        });
    }
    Ok(index)
}
