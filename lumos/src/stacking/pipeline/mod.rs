//! End-to-end registered stacking — raw lights to a stacked master in one call.
//!
//! [`align_and_stack`] runs the alignment + combine flow over calibrated frames: detect stars
//! → choose a reference → register every other frame to it → warp the ones that solve →
//! combine with [`stack_images`]. Frames that fail to register are dropped and reported in
//! [`AlignmentSummary::dropped`] rather than aborting the stack. [`calibrate_align_stack`]
//! prepends calibration: load each raw light, apply the calibration masters, demosaic, then
//! hand the calibrated frames to `align_and_stack`.

use rayon::prelude::*;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::io::astro_image::AstroImage;
use crate::io::astro_image::error::ImageError;
use crate::io::raw::{load_raw_cfa, raw_dimensions};
use crate::stacking::calibration_masters::CalibrationMasters;
use crate::stacking::calibration_masters::cosmic_ray::{CosmicRayConfig, reject_cosmic_rays};
use crate::stacking::combine::config::StackConfig;
use crate::stacking::combine::error::Error as StackError;
use crate::stacking::combine::progress::ProgressCallback;
use crate::stacking::combine::stack::{StackFrame, stack_images, stack_stored_frames};
use crate::stacking::frame_store::{
    MemoryPlan, SpillDirectory, StoredImage, StoredLightFrame, plan_memory, store_image,
    store_light_frame,
};
use crate::stacking::product::StackProduct;
use crate::stacking::registration::config::Config as RegistrationConfig;
use crate::stacking::registration::{register, warp};
use crate::stacking::star_detection::config::Config as StarDetectionConfig;
use crate::stacking::star_detection::detector::StarDetector;
use crate::stacking::star_detection::error::StarDetectionConfigError;
use crate::stacking::star_detection::star::Star;
use common::CancelToken;
use common::parallel::try_par_map_limited;

/// How the reference frame (the alignment anchor) is chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Reference {
    /// The frame with the most detected stars — the strongest registration anchor.
    #[default]
    Auto,
    /// A specific frame, by index into the input slice.
    Index(usize),
}

/// Configuration for [`align_and_stack`]: one config per pipeline stage plus the reference
/// choice. `Default` gives each stage its own default and `Reference::Auto`.
#[derive(Debug, Clone, Default)]
pub struct AlignStackConfig {
    pub detection: StarDetectionConfig,
    pub registration: RegistrationConfig,
    pub stack: StackConfig,
    pub reference: Reference,
    /// Optional single-frame cosmic-ray rejection, applied per light by [`calibrate_align_stack`]
    /// after calibration and before demosaic. `None` (default) skips it. Currently mono-only;
    /// Bayer/X-Trans frames are skipped with a warning (Phase 2 adds CFA support).
    pub cosmic_ray: Option<CosmicRayConfig>,
}

/// Registration bookkeeping for an aligned stack.
#[derive(Debug)]
pub struct AlignmentSummary {
    /// Index (into the input) of the reference frame the others were aligned to.
    pub reference: usize,
    /// Number of frames that went into the stack: the reference plus every frame that
    /// registered successfully.
    pub registered: usize,
    /// Indices of frames dropped because they failed to register, ascending.
    pub dropped: Vec<usize>,
}

/// Outcome of [`align_and_stack`].
#[derive(Debug)]
pub struct AlignStackResult {
    /// The combined image and its ancillary per-pixel science planes.
    pub product: StackProduct,
    /// Reference selection and frame registration outcome.
    pub alignment: AlignmentSummary,
}

impl AlignStackResult {
    /// Wrap a [`StackProduct`] with the alignment bookkeeping (shared by the RAM and
    /// streaming paths, which build this identically).
    fn from_product(
        product: StackProduct,
        reference: usize,
        registered: usize,
        dropped: Vec<usize>,
    ) -> Self {
        Self {
            product,
            alignment: AlignmentSummary {
                reference,
                registered,
                dropped,
            },
        }
    }
}

/// Errors from [`align_and_stack`] and [`calibrate_align_stack`].
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no light frames provided")]
    NoFrames,
    #[error("failed to load light frame '{path}': {source}")]
    Load {
        path: PathBuf,
        #[source]
        source: ImageError,
    },
    #[error("reference index {index} out of range ({count} frames)")]
    ReferenceOutOfRange { index: usize, count: usize },
    #[error("reference frame {index} has only {found} stars (need {required})")]
    ReferenceInsufficientStars {
        index: usize,
        found: usize,
        required: usize,
    },
    #[error("all {count} non-reference frames failed to register")]
    AllFramesDropped { count: usize },
    #[error(transparent)]
    DetectionConfig(#[from] StarDetectionConfigError),
    #[error(transparent)]
    Stack(#[from] StackError),
}

/// Detect → register → warp → stack a set of light frames into one aligned, combined image.
///
/// All frames are expected to share the same dimensions (same sensor). The reference frame is
/// added to the stack unwarped; every other frame is aligned to it. Frames that fail to
/// register (too few stars, RANSAC failure, accuracy gate) are dropped and listed in
/// [`AlignmentSummary::dropped`]; the stack proceeds with whatever aligned. A single
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
                    stars: Arc::from([]),
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
                stars: result.stars.into(),
            })
        })
        .collect();
    let detected_frames = detected_frames?;
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
    let ref_stars = Arc::clone(&detected_frames[reference].stars);
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

/// Max light frames decoded+demosaiced concurrently. The RAW decode is the one
/// uninterruptible step, so this caps the work a cancel must drain and peak
/// memory; the demosaic (work-conserving across cores) keeps the pool busy
/// within a batch, so the cap costs little throughput.
const MAX_CONCURRENT_LIGHTS: usize = 4;

/// Calibrate, align, and stack raw light frames end to end — the full pipeline in one call.
///
/// For each raw light (in parallel): load it as a `CfaImage`, apply `masters`
/// (dark/flat/defect) in place, demosaic to an `AstroImage`, then hand the calibrated frames
/// to [`align_and_stack`]. A frame that fails to **load** is a hard error (bad input); a frame
/// that fails to **register** is dropped and reported in [`AlignmentSummary::dropped`].
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

    // Tier decision: peek the sensor dimensions (no decode) and plan the memory tier. If the warped
    // frames plus the RAM path's per-frame scratch won't fit ~75% RAM, stream through a disk cache so
    // peak RAM stays flat in the frame count.
    let sensor = raw_dimensions(light_paths[0].as_ref()).map_err(|source| Error::Load {
        path: light_paths[0].as_ref().to_path_buf(),
        source,
    })?;
    let plane_bytes = sensor.x * sensor.y * std::mem::size_of::<f32>();
    let available = config.stack.cache.get_available_memory();
    let plan = plan_memory(plane_bytes, total, rayon::current_num_threads(), available);
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
        "Loading, calibrating and demosaicing raw lights (RAW decode — the slow phase)"
    );
    let done = AtomicUsize::new(0);
    // Bound how many frames are in flight: the RAW decode (libraw) is the one
    // uninterruptible step, so capping it caps the work a cancel must drain and
    // peak demosaic memory. The demosaic itself polls `cancel` between stages
    // (see `CfaImage::demosaic`), so the heavy phase stays interruptible at full
    // core utilization within a batch.
    let calibrated: Vec<AstroImage> =
        try_par_map_limited(light_paths, MAX_CONCURRENT_LIGHTS, |path| {
            // Skip launching the RAW decode (the slow uninterruptible step) once cancelled.
            if cancel.is_cancelled() {
                return Err(Error::Stack(StackError::Cancelled));
            }
            let image = decode_calibrate_demosaic(path.as_ref(), masters, config, &cancel)?;
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            tracing::info!(frame = n, total, "calibrated light");
            Ok(image)
        })?;

    align_and_stack(calibrated, config, cancel)
}

/// Load one raw light, apply the calibration masters, optionally reject cosmic rays, and demosaic to
/// an `AstroImage`. The per-frame core shared by the RAM and streaming calibrate paths.
fn decode_calibrate_demosaic(
    path: &Path,
    masters: &CalibrationMasters,
    config: &AlignStackConfig,
    cancel: &CancelToken,
) -> Result<AstroImage, Error> {
    let mut cfa = load_raw_cfa(path).map_err(|source| Error::Load {
        path: path.to_path_buf(),
        source,
    })?;
    masters.calibrate(&mut cfa);
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
    cfa.demosaic(cancel)
        .map_err(|_| Error::Stack(StackError::Cancelled))
}

/// Choose the reference (alignment anchor) index from per-frame star counts, validating it has
/// enough stars. Shared by the RAM and streaming paths.
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

/// One detected frame whose pixels and stars advance through the pipeline together.
#[derive(Debug)]
struct DetectedFrame<I> {
    image: I,
    stars: Arc<[Star]>,
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
    let detected: Vec<DetectedFrame<StoredImage>> =
        try_par_map_limited(&indexed, decode_concurrency, |&(idx, ref path)| {
            if cancel.is_cancelled() {
                return Err(Error::Stack(StackError::Cancelled));
            }
            let image = decode_calibrate_demosaic(path.as_ref(), masters, config, &cancel)?;
            let stars: Arc<[Star]> = StarDetector::from_config(config.detection.clone())?
                .detect(&image)
                .stars
                .into();
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
        })?;
    if cancel.is_cancelled() {
        return Err(Error::Stack(StackError::Cancelled));
    }

    let dimensions = detected[0].image.dimensions;
    let star_counts: Vec<usize> = detected.iter().map(|d| d.stars.len()).collect();
    let reference = select_reference(
        &star_counts,
        config.reference,
        config.registration.required_stars(),
    )?;
    let metadata = detected[reference].image.metadata.clone();
    let ref_stars = Arc::clone(&detected[reference].stars);
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
                let name = format!("warped_{idx}");
                if idx == reference {
                    return store_light_frame(cache_dir, &name, calibrated, None)
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
                    &config.registration,
                );
                tracing::info!(
                    frame = n,
                    total = total - 1,
                    inliers = registration.num_inliers,
                    "registered (streaming)"
                );
                store_light_frame(cache_dir, &name, warped.image, Some(warped.coverage))
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

#[cfg(test)]
mod mem_budget_probe;
#[cfg(test)]
mod mem_budget_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ImageDimensions;
    use crate::stacking::registration::transform::{Transform, WarpTransform};
    use crate::testing::synthetic::fixtures::star_field;
    use glam::DVec2;

    fn base_field() -> (AstroImage, RegistrationConfig) {
        let image = star_field(256, 256, 40, 66666).image;
        (image, RegistrationConfig::default())
    }

    /// Warp `base` by a pure translation to fake a dithered exposure.
    fn shifted(base: &AstroImage, reg: &RegistrationConfig, dx: f64, dy: f64) -> AstroImage {
        let t = Transform::translation(DVec2::new(dx, dy));
        warp(base, &WarpTransform::new(t), reg).image
    }

    #[test]
    fn aligns_shifted_frames_into_a_sharp_stack() {
        let (base, reg) = base_field();
        let frames = vec![
            base.clone(),
            shifted(&base, &reg, 8.0, -5.0),
            shifted(&base, &reg, -6.0, 7.0),
        ];

        let config = AlignStackConfig {
            reference: Reference::Index(0),
            ..Default::default()
        };
        let result = align_and_stack(frames, &config, CancelToken::never()).expect("stack");

        assert_eq!(result.alignment.reference, 0);
        assert_eq!(
            result.alignment.registered, 3,
            "all three frames should stack"
        );
        assert!(
            result.alignment.dropped.is_empty(),
            "dropped: {:?}",
            result.alignment.dropped
        );

        // Alignment check: every frame was warped back to the reference, so the reference's
        // brightest star must reappear at the same place in the combined image.
        let mut det = StarDetector::from_config(StarDetectionConfig::default()).unwrap();
        let ref_pos = det.detect(&base).stars[0].pos;
        let stack_stars = det.detect(&result.product.image).stars;
        let nearest = stack_stars
            .iter()
            .map(|s| (s.pos - ref_pos).length())
            .fold(f64::MAX, f64::min);
        assert!(
            nearest < 0.5,
            "reference's brightest star not aligned in the stack: nearest {nearest:.3} px"
        );
    }

    #[test]
    fn drops_unregisterable_frame_and_stacks_the_rest() {
        let (base, reg) = base_field();
        let dims = base.dimensions;
        // A flat frame has no stars → registration fails → it is dropped, not fatal.
        let blank = AstroImage::from_pixels(dims, vec![0.1; dims.size.x * dims.size.y]);
        let frames = vec![base.clone(), shifted(&base, &reg, 5.0, 3.0), blank];

        let config = AlignStackConfig {
            reference: Reference::Index(0),
            ..Default::default()
        };
        let result = align_and_stack(frames, &config, CancelToken::never()).expect("stack");

        assert_eq!(
            result.alignment.dropped,
            vec![2],
            "blank frame should be dropped"
        );
        assert_eq!(
            result.alignment.registered, 2,
            "reference + one aligned frame"
        );
    }

    #[test]
    fn stacked_master_inherits_reference_frame_metadata() {
        // The master's metadata comes from the reference frame (the alignment anchor), not frame 0,
        // so the RAM and streaming tiers agree. With reference = index 1, frame 0 is a (warped)
        // non-reference frame whose metadata must NOT win.
        let (base, reg) = base_field();
        let mut f0 = shifted(&base, &reg, 5.0, 3.0);
        let mut f1 = base.clone(); // the reference (index 1)
        let mut f2 = shifted(&base, &reg, -4.0, 6.0);
        f0.metadata.exposure_time = Some(10.0);
        f1.metadata.exposure_time = Some(20.0);
        f2.metadata.exposure_time = Some(30.0);

        let config = AlignStackConfig {
            reference: Reference::Index(1),
            ..Default::default()
        };
        let result =
            align_and_stack(vec![f0, f1, f2], &config, CancelToken::never()).expect("stack");

        assert_eq!(result.alignment.reference, 1);
        assert_eq!(
            result.product.image.metadata.exposure_time,
            Some(20.0),
            "master must inherit the reference (index 1) metadata, not frame 0's"
        );
    }

    #[test]
    fn all_non_reference_frames_dropped_errors() {
        // With the reference produced in-place (it survives in `frames`), "nothing aligned" means
        // only the reference remains — guard the changed `frames.len() <= 1` condition.
        let (base, _) = base_field();
        let dims = base.dimensions;
        let blank = || AstroImage::from_pixels(dims, vec![0.1; dims.size.x * dims.size.y]);
        // Reference has stars; both others are blank → both fail to register → nothing aligns.
        let frames = vec![base, blank(), blank()];

        let config = AlignStackConfig {
            reference: Reference::Index(0),
            ..Default::default()
        };
        let err = align_and_stack(frames, &config, CancelToken::never()).unwrap_err();
        assert!(
            matches!(err, Error::AllFramesDropped { count: 2 }),
            "all non-reference frames dropped → AllFramesDropped {{ count: 2 }}, got {err:?}"
        );
    }

    #[test]
    fn auto_reference_picks_the_richest_frame() {
        let (base, reg) = base_field();
        // Frame 1 (full field) has far more stars than frame 0 (a near-blank), so Auto must
        // anchor on frame 1.
        let dims = base.dimensions;
        let sparse = AstroImage::from_pixels(dims, vec![0.1; dims.size.x * dims.size.y]);
        let frames = vec![sparse, base.clone(), shifted(&base, &reg, 4.0, -3.0)];

        let result = align_and_stack(frames, &AlignStackConfig::default(), CancelToken::never())
            .expect("stack");
        assert_ne!(
            result.alignment.reference, 0,
            "Auto must not anchor on the near-blank frame"
        );
        assert_eq!(
            result.alignment.dropped,
            vec![0],
            "the near-blank frame can't register"
        );
    }

    #[test]
    fn public_input_errors() {
        let err = align_and_stack(
            Vec::new(),
            &AlignStackConfig::default(),
            CancelToken::never(),
        )
        .unwrap_err();
        assert!(matches!(err, Error::NoFrames));

        let config = AlignStackConfig {
            detection: StarDetectionConfig {
                sigma_threshold: 0.0,
                ..StarDetectionConfig::default()
            },
            ..AlignStackConfig::default()
        };
        let image = AstroImage::from_pixels(ImageDimensions::new((1, 1), 1), vec![0.0]);
        let error = align_and_stack(vec![image], &config, CancelToken::never()).unwrap_err();
        assert!(matches!(
            error,
            Error::DetectionConfig(StarDetectionConfigError::InvalidSigmaThreshold { value: 0.0 })
        ));
    }

    #[test]
    #[cfg_attr(
        not(feature = "real-data"),
        ignore = "requires the bundled real-data dataset"
    )]
    fn calibrate_align_stack_runs_end_to_end_on_real_lights() {
        use crate::testing::calibration_image_paths;
        use crate::{CalibrationFrames, DEFAULT_SIGMA_THRESHOLD};

        let dark_paths = calibration_image_paths("Darks").unwrap_or_default();
        let bias_paths = calibration_image_paths("Bias").unwrap_or_default();
        let flat_paths = calibration_image_paths("Flats").unwrap_or_default();
        let empty: Vec<std::path::PathBuf> = Vec::new();
        let masters = CalibrationMasters::from_files(
            CalibrationFrames {
                darks: &dark_paths,
                flats: &flat_paths,
                bias: &bias_paths,
                flat_darks: &empty,
            },
            DEFAULT_SIGMA_THRESHOLD,
        )
        .expect("build calibration masters");

        let all = calibration_image_paths("Lights").expect("Lights subdirectory");
        let lights = &all[..all.len().min(3)];
        assert!(lights.len() >= 2, "need ≥2 lights to exercise registration");

        let result = calibrate_align_stack(
            lights,
            &masters,
            &AlignStackConfig::default(),
            CancelToken::never(),
        )
        .expect("calibrate_align_stack");

        // A real stacked image came out, and every input frame is accounted for.
        assert!(result.product.image.width() > 0 && result.product.image.height() > 0);
        assert_eq!(
            result.alignment.registered + result.alignment.dropped.len(),
            lights.len()
        );
        assert!(
            result.alignment.registered >= 1,
            "at least the reference is stacked"
        );
    }

    #[test]
    #[cfg_attr(
        not(feature = "real-data"),
        ignore = "requires the bundled real-data dataset"
    )]
    fn streaming_disk_tier_matches_ram_on_real_lights() {
        use crate::testing::calibration_image_paths;
        use crate::{CalibrationFrames, DEFAULT_SIGMA_THRESHOLD};

        let dark_paths = calibration_image_paths("Darks").unwrap_or_default();
        let bias_paths = calibration_image_paths("Bias").unwrap_or_default();
        let flat_paths = calibration_image_paths("Flats").unwrap_or_default();
        let empty: Vec<std::path::PathBuf> = Vec::new();
        let masters = CalibrationMasters::from_files(
            CalibrationFrames {
                darks: &dark_paths,
                flats: &flat_paths,
                bias: &bias_paths,
                flat_darks: &empty,
            },
            DEFAULT_SIGMA_THRESHOLD,
        )
        .expect("build calibration masters");

        let all = calibration_image_paths("Lights").expect("Lights subdirectory");
        let lights = &all[..all.len().min(3)];
        assert!(lights.len() >= 2, "need ≥2 lights to exercise registration");

        // Seed RANSAC so both tiers are bit-comparable (registration is the only nondeterminism).
        let mut config = AlignStackConfig::default();
        config.registration.seed = Some(0x00C0_FFEE);

        // RAM tier: huge memory budget → the all-in-memory path.
        let mut ram_cfg = config.clone();
        ram_cfg.stack.cache.available_memory = Some(u64::MAX);
        let ram = calibrate_align_stack(lights, &masters, &ram_cfg, CancelToken::never())
            .expect("RAM-tier stack");

        // Disk tier: a 1-byte budget forces the streaming disk path; clean its cache on drop.
        let mut disk_cfg = config;
        disk_cfg.stack.cache.available_memory = Some(1);
        disk_cfg.stack.cache.keep_cache = false;
        let disk = calibrate_align_stack(lights, &masters, &disk_cfg, CancelToken::never())
            .expect("disk-tier (streaming) stack");

        assert_eq!(
            ram.alignment.registered, disk.alignment.registered,
            "same frames stacked"
        );
        assert_eq!(
            ram.alignment.dropped, disk.alignment.dropped,
            "same frames dropped"
        );
        assert_eq!(
            ram.alignment.reference, disk.alignment.reference,
            "same reference"
        );
        assert_eq!(
            ram.product.image.dimensions(),
            disk.product.image.dimensions()
        );
        // Bit-identical: same frames, same (seeded) registration, same combine — only the frame
        // storage (RAM vs mmap) differs.
        for c in 0..ram.product.image.channels() {
            let a: Vec<u32> = ram
                .product
                .image
                .channel(c)
                .pixels()
                .iter()
                .map(|x| x.to_bits())
                .collect();
            let b: Vec<u32> = disk
                .product
                .image
                .channel(c)
                .pixels()
                .iter()
                .map(|x| x.to_bits())
                .collect();
            assert_eq!(a, b, "channel {c} differs between the RAM and disk tiers");
        }
    }
}
