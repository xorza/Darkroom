//! Detection, registration, warping, and combination of calibrated images.

use std::sync::atomic::{AtomicUsize, Ordering};

use common::CancelToken;
use rayon::prelude::*;

use crate::io::astro_image::AstroImage;
use crate::stacking::combine::error::Error as StackError;
use crate::stacking::combine::progress::ProgressCallback;
use crate::stacking::combine::stack::{StackFrame, stack_images};
use crate::stacking::registration::{register, warp};
use crate::stacking::star_detection::detector::StarDetector;
use crate::stacking::star_detection::error::StarDetectionConfigError;
use crate::stacking::star_detection::star::Star;

use crate::stacking::pipeline::config::{AlignStackConfig, Reference};
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
    lights: Vec<AstroImage>,
    config: &AlignStackConfig,
    cancel: CancelToken,
) -> Result<AlignStackResult, Error> {
    if lights.is_empty() {
        return Err(Error::NoFrames);
    }
    config.detection.validate()?;

    // Detect stars on every frame. Each rayon task owns its detector — `detect` is `&mut`.
    let total = lights.len();
    tracing::info!(frames = total, "Detecting stars");
    let detected = AtomicUsize::new(0);
    let detected_frames: Result<Vec<DetectedFrame<AstroImage>>, StarDetectionConfigError> = lights
        .into_par_iter()
        .map(|image| {
            // Cancelled: skip this frame's detection (cheap empty result); the
            // post-loop check below turns the run into `Cancelled`.
            if cancel.is_cancelled() {
                return Ok(DetectedFrame {
                    image,
                    stars: Vec::new(),
                });
            }
            let result = StarDetector::from_config(config.detection.clone())?.detect(&image);
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
            Ok(DetectedFrame {
                image,
                stars: result.stars,
            })
        })
        .collect();
    let mut detected_frames = detected_frames?;
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
        config.registration.required_stars(),
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
                // Reference goes in unwarped → fully covered; `coverage: None` weights it 1
                // everywhere (no throwaway full-coverage map to allocate).
                return Ok(StackFrame {
                    image: detected.image,
                    coverage: None,
                });
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
                        inliers = result.num_inliers,
                        rms = format!("{:.3}", result.rms_error),
                        quality = format!("{:.3}", result.quality_score),
                        transform = %result.transform,
                        "registered"
                    );
                    let warped = warp(
                        &detected.image,
                        &result.warp_transform(),
                        &config.registration,
                    );
                    Ok(StackFrame {
                        image: warped.image,
                        coverage: Some(warped.coverage),
                    })
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

    // Each warped frame carries its coverage (how much of each output pixel landed on real
    // source); the stack uses it so warped-frame borders don't drag the edges dark. The reference
    // is already in `outcomes` (in its index position, unwarped).
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
