//! Detection, registration, warping, and combination of calibrated images.

use std::mem;
use std::path::Path;

use common::CancelToken;

use crate::concurrency;
use crate::ingest::ingest_run::IngestRun;
use crate::io::image::linear::LinearImage;
use crate::progress::ProgressCallback;
use crate::registration::resample::WarpBuffers;
use crate::star_detection::detector::DetectionResult;
use crate::star_detection::detector::Diagnostics;

use crate::pipeline::config::{AlignStackConfig, Reference};
use crate::pipeline::frame::DetectedFrame;
use crate::pipeline::frame_registrar::{FrameRegistrar, FrameToPark};
use crate::pipeline::light_source::LightSource;
use crate::pipeline::registered_set::RegisteredSet;
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
    let metadata = detected[reference].image.metadata().clone();
    let dimensions = detected[reference].image.dimensions();
    let registrar = FrameRegistrar::new(
        reference,
        mem::take(&mut detected[reference].stars),
        config,
        &tier,
        total,
        &progress,
        &cancel,
    );
    // One reusable set of warp output planes per in-flight worker.
    let mut warp_buffers: Vec<Option<WarpBuffers>> = (0..warp_concurrency).map(|_| None).collect();
    // Taking each detected record by value frees its input image as soon as the warped output
    // exists, so this stage never holds the complete input and warped sets simultaneously.
    let outcomes = concurrency::try_par_map_bounded_owned(
        detected,
        &mut warp_buffers,
        |buffers, index, detected| {
            registrar.park(
                buffers,
                FrameToPark {
                    index,
                    image: detected.image.into_image(),
                    stars: &detected.stars,
                    stats: detected.stats,
                },
            )
        },
    )?;
    drop(registrar);

    RegisteredSet {
        outcomes,
        reference,
        metadata,
        dimensions,
        detection,
    }
    .combine(tier, config, progress, cancel)
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
