//! Detection, registration, warping, and combination of calibrated images.

use std::cmp::Reverse;
use std::mem;
use std::path::Path;

use common::CancelToken;

use crate::concurrency;
use crate::ingest::ingest_run::IngestRun;
use crate::io::image::linear::LinearImage;
use crate::progress::progress_callback::ProgressCallback;
use crate::registration::resample::WarpBuffers;
use crate::star_detection::detector::DetectionResult;
use crate::star_detection::detector::Diagnostics;

use crate::pipeline::config::{AlignStackConfig, Reference};
use crate::pipeline::detected_frame::DetectedFrame;
use crate::pipeline::error::AlignStackError;
use crate::pipeline::frame_registrar::{FrameRegistrar, FrameToPark};
use crate::pipeline::frame_tier::StagePlan;
use crate::pipeline::light_source::LightSource;
use crate::pipeline::registered_set::RegisteredSet;
use crate::pipeline::result::AlignStackResult;

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
) -> Result<AlignStackResult, AlignStackError> {
    if lights.is_empty() {
        return Err(AlignStackError::NoFrames);
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
) -> Result<AlignStackResult, AlignStackError> {
    let StagePlan {
        tier,
        warp_concurrency,
    } = stage;
    let total = detected.len();
    if cancel.is_cancelled() {
        return Err(AlignStackError::Cancelled);
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
    let median_fwhms: Vec<Option<f32>> =
        detection.iter().map(|funnel| funnel.median_fwhm).collect();
    let reference = select_reference(
        &star_counts,
        &median_fwhms,
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
                    image: detected.image,
                    stars: &detected.stars,
                    stats: detected.stats,
                },
            )
        },
    )?;
    drop(registrar);
    // A worker's buffers are a warped frame's worth each, and the combine sizes its chunks
    // against the memory without them.
    drop(warp_buffers);

    RegisteredSet {
        outcomes,
        reference,
        metadata,
        dimensions,
        detection,
    }
    .combine(&tier, config, progress, cancel)
}

/// Choose the reference (alignment anchor) index, validating it has enough stars.
///
/// `Auto` takes the sharpest frame among those with `required` stars: the lowest median FWHM, as
/// Siril chooses, ties to the lowest index. Every other frame is warped onto it, so its seeing is
/// what the stack is resampled to; the star count only has to clear the gate registration needs.
/// When no frame clears it, the one with the most stars is reported.
fn select_reference(
    star_counts: &[usize],
    median_fwhms: &[Option<f32>],
    reference: Reference,
    required: usize,
) -> Result<usize, AlignStackError> {
    let index = match reference {
        Reference::Index(index) => {
            if index >= star_counts.len() {
                return Err(AlignStackError::ReferenceOutOfRange {
                    index,
                    count: star_counts.len(),
                });
            }
            index
        }
        Reference::Auto => (0..star_counts.len())
            .filter(|&i| star_counts[i] >= required)
            .filter_map(|i| median_fwhms[i].map(|fwhm| (i, fwhm)))
            .reduce(|best, candidate| {
                if candidate.1 < best.1 {
                    candidate
                } else {
                    best
                }
            })
            .map_or_else(
                || {
                    (0..star_counts.len())
                        .max_by_key(|&i| (star_counts[i], Reverse(i)))
                        .expect("star_counts is non-empty")
                },
                |(i, _)| i,
            ),
    };
    if star_counts[index] < required {
        return Err(AlignStackError::ReferenceInsufficientStars {
            index,
            found: star_counts[index],
            required,
        });
    }
    Ok(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Auto` takes the lowest median FWHM among the frames with the stars registration needs:
    /// frame 2 at 2.5 of 2.5, 3.0 and 3.5; a tie goes to the lowest index; a frame below the gate
    /// is passed over however sharp, frame 1 at 2.0 with 30 of 40 stars; and when none clears the
    /// gate, the one with the most stars is the one reported. An index must lie in the input.
    #[test]
    fn auto_takes_the_sharpest_frame_with_enough_stars() {
        let auto = |counts: &[usize], fwhms: &[f32]| {
            let fwhms: Vec<Option<f32>> = fwhms.iter().map(|&fwhm| Some(fwhm)).collect();
            select_reference(counts, &fwhms, Reference::Auto, 40)
        };
        assert_eq!(auto(&[50, 80, 60], &[3.0, 3.5, 2.5]).unwrap(), 2);
        assert_eq!(auto(&[50, 80, 60], &[2.5, 3.0, 2.5]).unwrap(), 0);
        assert_eq!(auto(&[50, 30, 60], &[3.0, 2.0, 3.5]).unwrap(), 0);
        assert!(matches!(
            auto(&[10, 20, 15], &[3.0, 2.0, 3.5]),
            Err(AlignStackError::ReferenceInsufficientStars {
                index: 1,
                found: 20,
                required: 40
            })
        ));
        assert!(matches!(
            select_reference(&[50, 60], &[Some(3.0), Some(3.0)], Reference::Index(5), 40),
            Err(AlignStackError::ReferenceOutOfRange { index: 5, count: 2 })
        ));
    }
}
