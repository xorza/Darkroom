//! Detection, registration, warping, and combination of calibrated images.

use std::mem;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::CancelToken;

use crate::combine::stack::stack_stored_frames;
use crate::concurrency;
use crate::frame_store::stored_frame::StoredFrame;
use crate::ingest::ingest_run::IngestRun;
use crate::io::image::linear::LinearImage;
use crate::progress::stage_counter::StageCounter;
use crate::progress::{ProgressCallback, StackingStage};
use crate::registration::register;
use crate::registration::resample::WarpBuffers;
use crate::registration::result::RegistrationError;
use crate::star_detection::detector::DetectionResult;
use crate::star_detection::detector::Diagnostics;

use crate::pipeline::config::{AlignStackConfig, Reference};
use crate::pipeline::frame::DetectedFrame;
use crate::pipeline::light_source::LightSource;
use crate::pipeline::result::{AlignStackResult, Error};
use crate::pipeline::tier::StagePlan;

/// Detect → register → warp → stack a set of light frames into one aligned, combined image.
///
/// All frames are expected to share the same dimensions (same sensor). The reference frame is
/// added to the stack unwarped; every other frame is aligned to it. Frames that fail to
/// register (too few stars, RANSAC failure, accuracy gate) are dropped and listed in
/// [`AlignmentSummary::dropped`](crate::pipeline::result::AlignmentSummary::dropped);
/// the stack proceeds with whatever aligned. A single input frame is returned as its own "stack".
///
/// Every light must share the first one's dimensions and hold only finite samples; each is checked
/// before its stars are detected, so a bad input is reported as one instead of surfacing from the
/// combine after a warp has spread it.
///
/// The frames arrive decoded and resident, so the inputs are committed before this is called —
/// but the warped outputs are a second full set, and those spill to the frame store when they
/// would not fit alongside the inputs. For camera RAW, enter through
/// [`calibrate_align_stack`](crate::pipeline::calibrate::calibrate_align_stack)
/// instead, which tiers the decode as well.
pub fn align_and_stack(
    lights: Vec<LinearImage>,
    config: &AlignStackConfig,
    progress: ProgressCallback,
    cancel: CancelToken,
) -> Result<AlignStackResult, Error> {
    if lights.is_empty() {
        return Err(Error::NoFrames);
    }
    config.validate(lights.len())?;
    let run = IngestRun::new(&config.stack.ingest, cancel.clone());
    let detected = LightSource::<&Path>::Held(lights).detect(config, &run, &progress)?;
    register_warp_and_stack(detected.frames, config, detected.stage, progress, cancel)
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
/// a [`DetectedFrame`] — already decoded, or decoded and calibrated from a path — and `stage`
/// decides whether a warped output stays resident or goes to the frame store. Everything from
/// reference selection onward is the same work either way.
///
/// Both entries checked every frame's dimensions against the first before detection: `warp`
/// reprojects into the *source* frame's grid, so a mismatch here would reach the combine as
/// differently-sized planes.
pub(crate) fn register_warp_and_stack(
    mut detected: Vec<DetectedFrame>,
    config: &AlignStackConfig,
    stage: StagePlan,
    progress: ProgressCallback,
    cancel: CancelToken,
) -> Result<AlignStackResult, Error> {
    let StagePlan {
        tier,
        warp_concurrency,
    } = stage;
    let total = detected.len();
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    debug_assert!(
        detected
            .iter()
            .all(|frame| frame.image.dimensions() == detected[0].image.dimensions()),
        "both entries check the dimensions before detection"
    );

    // Taken before the frames are consumed below, so the funnel survives into the result in input
    // order — including for frames registration goes on to drop.
    let detection: Vec<Diagnostics> = detected
        .iter_mut()
        .map(|frame| mem::take(&mut frame.diagnostics))
        .collect();

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
    let ref_stars = mem::take(&mut detected[reference].stars);
    tracing::info!(
        reference,
        ref_stars = ref_stars.len(),
        "Reference frame selected"
    );

    tracing::info!(frames = total - 1, "Registering frames to the reference");
    let registered_so_far = AtomicUsize::new(0);
    // Counted where the work ends rather than where it starts. `registered` and `dropped`
    // frames both count — the bar tracks attempts resolved, not survivors.
    let resolved = StageCounter::new(&progress, StackingStage::Registering, total - 1);
    let report_resolved = || {
        resolved.complete_one();
    };
    // One reusable set of warp output planes per in-flight worker. The spill tier hands its
    // buffers back once the frame is on disk, so a worker warps into pages it has already faulted
    // in; the RAM tier keeps them, and the slot simply refills from a fresh allocation it would
    // have made anyway.
    let mut warp_buffers: Vec<Option<WarpBuffers>> = (0..warp_concurrency).map(|_| None).collect();
    // Taking each detected record by value frees its input image as soon as the warped output
    // exists, so this stage never holds the complete input and warped sets simultaneously.
    let outcomes = concurrency::try_par_map_bounded_owned(
        detected,
        &mut warp_buffers,
        |buffers, index, detected| -> Result<Option<StoredFrame>, Error> {
            // Cancelled: drop this frame (skips the heavy register + warp); the post-loop check
            // below turns the run into `Cancelled`.
            if cancel.is_cancelled() {
                return Ok(None);
            }
            let name = format!("warped_{index}");
            let source_stats = detected.stats;
            if index == reference {
                // The unwarped reference has full support and unit interpolation confidence.
                let image = detected.image.into_image();
                return tier.store_reference(&name, image, source_stats).map(Some);
            }

            let n = registered_so_far.fetch_add(1, Ordering::Relaxed) + 1;
            let source = detected.image.into_image();
            let registration = match register(&ref_stars, &detected.stars, &config.registration) {
                Ok(registration) => registration,
                // A pair that did not match is a frame to drop. An invalid config is not: it
                // fails identically for every pair, so dropping it would spend the whole run to
                // report `AllFramesDropped` and blame the data.
                Err(RegistrationError::InvalidConfig(invalid)) => {
                    return Err(Error::RegistrationConfig(invalid));
                }
                Err(error) => {
                    tracing::info!(frame = n, total = total - 1, %error, "registration failed");
                    report_resolved();
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
            let mut warped = buffers
                .take()
                .unwrap_or_else(|| WarpBuffers::new(source.dimensions()));
            warped.warp_into(
                &source,
                &registration.warp_transform(),
                config.registration.warp,
            );
            let metadata = source.metadata.clone();
            drop(source);
            report_resolved();
            let stored = tier.store(&name, metadata, warped, source_stats)?;
            *buffers = stored.reusable;
            Ok(Some(stored.frame))
        },
    )?;
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
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
        tier.into_cache_tier(),
        dimensions,
        metadata,
        &config.stack.for_survivors(&dropped),
        progress,
        cancel,
    )?;
    tracing::info!("Stack complete");

    Ok(AlignStackResult::from_product(
        stacked, reference, registered, dropped, detection,
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
