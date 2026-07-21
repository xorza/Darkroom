//! Detection, registration, warping, and combination of calibrated images.

use std::sync::atomic::{AtomicUsize, Ordering};

use common::CancelToken;
use rayon::prelude::*;

use crate::io::astro_image::LinearImage;
use crate::stacking::combine::error::Error as StackError;
use crate::stacking::combine::stack::{StackFrame, stack_images};
use crate::stacking::progress::ProgressCallback;
use crate::stacking::registration::register;
use crate::stacking::registration::resample::warp;
use crate::stacking::star_detection::star::Star;

use crate::stacking::pipeline::config::{AlignStackConfig, Reference};
use crate::stacking::pipeline::detector_pool::DetectorPool;
use crate::stacking::pipeline::result::{AlignStackResult, Error};

/// Detect → register → warp → stack a set of light frames into one aligned, combined image.
///
/// All frames are expected to share the same dimensions (same sensor). The reference frame is
/// added to the stack unwarped; every other frame is aligned to it. Frames that fail to
/// register (too few stars, RANSAC failure, accuracy gate) are dropped and listed in
/// [`AlignmentSummary::dropped`](crate::stacking::pipeline::result::AlignmentSummary::dropped);
/// the stack proceeds with whatever aligned. A single
/// input frame is returned as its own "stack".
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
    tracing::info!(frames = total, "Detecting stars");
    let detected = AtomicUsize::new(0);
    let detected_stars = {
        let mut detectors =
            DetectorPool::from_config(&config.detection, total.min(rayon::current_num_threads()))?;
        detectors.try_map(&lights, |detector, image| {
            // Cancelled: skip this frame's detection (cheap empty result); the
            // post-loop check below turns the run into `Cancelled`.
            if cancel.is_cancelled() {
                return Ok(Vec::new());
            }
            let result = detector.detect(image);
            let d = &result.diagnostics;
            let n = detected.fetch_add(1, Ordering::Relaxed) + 1;
            // The detection funnel — candidates → deblended → centroided → kept — shows how
            // confidently the frame resolved into usable stars.
            tracing::info!(
                frame = n,
                total,
                candidates = d.candidates_after_filtering,
                deblended = d.deblended_components,
                measured = d.stars_after_centroid,
                stars = result.stars.len(),
                "detected stars"
            );
            Ok::<_, Error>(result.stars)
        })
    }?;
    let mut detected_frames: Vec<DetectedFrame<LinearImage>> = lights
        .into_iter()
        .zip(detected_stars)
        .map(|(image, stars)| DetectedFrame { image, stars })
        .collect();
    let total_stars: usize = detected_frames.iter().map(|frame| frame.stars.len()).sum();
    tracing::info!(total_stars, "Star detection complete");
    if cancel.is_cancelled() {
        return Err(Error::Stack(StackError::Cancelled));
    }

    let star_counts: Vec<usize> = detected_frames
        .iter()
        .map(|frame| frame.stars.len())
        .collect();
    let reference = select_reference(
        &star_counts,
        config.reference,
        config
            .registration
            .matching
            .required_stars(config.registration.transform_type),
    )?;
    let ref_stars = std::mem::take(&mut detected_frames[reference].stars);
    // The master follows the alignment anchor rather than whichever frame reaches combine first.
    let ref_metadata = detected_frames[reference].image.metadata.clone();
    tracing::info!(
        reference,
        ref_stars = ref_stars.len(),
        "Reference frame selected"
    );

    // Consuming each detected record frees its input image as soon as its warped output is produced,
    // so this stage never holds the complete input and warped sets simultaneously.
    let n_lights = detected_frames.len();
    let reg_total = n_lights - 1;
    tracing::info!(frames = reg_total, "Registering frames to the reference");
    let registered_so_far = AtomicUsize::new(0);
    let outcomes: Vec<Result<StackFrame, usize>> = detected_frames
        .into_par_iter()
        .enumerate()
        .map(|(index, detected)| {
            if index == reference {
                // The unwarped reference has full support and unit interpolation confidence.
                return Ok(StackFrame::from(detected.image));
            }
            // Cancelled: drop this frame (skips the heavy register + warp); the
            // post-loop check below turns the run into `Cancelled`.
            if cancel.is_cancelled() {
                return Err(index);
            }
            let n = registered_so_far.fetch_add(1, Ordering::Relaxed) + 1;
            match register(&ref_stars, &detected.stars, &config.registration) {
                Ok(result) => {
                    tracing::info!(
                        frame = n,
                        total = reg_total,
                        inliers = result.num_inliers(),
                        rms = format!("{:.3}", result.rms_error()),
                        quality = format!("{:.3}", result.quality_score()),
                        transform = %result.transform(),
                        "registered"
                    );
                    let warped = warp(
                        &detected.image,
                        &result.warp_transform(),
                        &config.registration.warp,
                    );
                    Ok(StackFrame::registered(&detected.image, warped))
                }
                Err(error) => {
                    tracing::info!(frame = n, total = reg_total, %error, "registration failed");
                    Err(index)
                }
            }
        })
        .collect();
    if cancel.is_cancelled() {
        return Err(Error::Stack(StackError::Cancelled));
    }

    // Warped frames carry separate support and interpolation-confidence planes. The reference is
    // already in `outcomes` at its original index.
    let mut frames: Vec<StackFrame> = Vec::with_capacity(outcomes.len());
    let mut dropped = Vec::new();
    for outcome in outcomes {
        match outcome {
            Ok(frame) => frames.push(frame),
            Err(index) => dropped.push(index),
        }
    }
    dropped.sort_unstable();
    tracing::info!(
        aligned = frames.len(),
        dropped = dropped.len(),
        "Registration complete"
    );

    // Only the reference survived → every non-reference frame dropped. (A lone reference input is
    // fine; "nothing aligned" with more than one input is an error.)
    if frames.len() <= 1 && n_lights > 1 {
        return Err(Error::AllFramesDropped {
            count: n_lights - 1,
        });
    }

    let registered = frames.len();
    tracing::info!(frames = registered, "Stacking aligned frames");
    let mut stacked = stack_images(
        frames,
        config.stack.clone(),
        ProgressCallback::default(),
        cancel,
    )?;
    stacked.image.metadata = ref_metadata;
    tracing::info!("Stack complete");

    Ok(AlignStackResult::from_product(
        stacked, reference, registered, dropped,
    ))
}

/// Choose the reference (alignment anchor) index from per-frame star counts, validating it has
/// enough stars. Shared by the RAM and streaming paths.
pub(crate) fn select_reference(
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

/// One detected frame whose pixels and stars advance through the pipeline together.
#[derive(Debug)]
pub(crate) struct DetectedFrame<I> {
    pub(crate) image: I,
    pub(crate) stars: Vec<Star>,
}
