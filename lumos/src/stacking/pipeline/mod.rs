//! End-to-end registered stacking — raw lights to a stacked master in one call.
//!
//! [`align_and_stack`] runs the alignment + combine flow over calibrated frames: detect stars
//! → choose a reference → register every other frame to it → warp the ones that solve →
//! combine with [`stack_images`]. Frames that fail to register are dropped and reported in
//! [`AlignStackResult::dropped`] rather than aborting the stack. [`calibrate_align_stack`]
//! prepends calibration: load each raw light, apply the calibration masters, demosaic, then
//! hand the calibrated frames to `align_and_stack`.

use rayon::prelude::*;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use arrayvec::ArrayVec;
use common::CancelToken;
use common::parallel::try_par_map_limited;
use imaginarium::Buffer2;

use crate::io::astro_image::error::ImageError;
use crate::io::astro_image::{AstroImage, AstroImageMetadata, ImageDimensions};
use crate::io::raw::{load_raw_cfa, raw_dimensions};
use crate::stacking::calibration_masters::CalibrationMasters;
use crate::stacking::calibration_masters::cosmic_ray::{CosmicRayConfig, reject_cosmic_rays};
use crate::stacking::combine::cache::{
    FrameStats, Plane, WeightedFrame, image_from_spilled_channels, remove_spilled_channels,
    spill_channels, spill_weighted_frame,
};
use crate::stacking::combine::cache_config::{compute_load_concurrency, fits_in_memory};
use crate::stacking::combine::config::StackConfig;
use crate::stacking::combine::error::Error as StackError;
use crate::stacking::combine::progress::ProgressCallback;
use crate::stacking::combine::stack::{
    StackFrame, StackResult, stack_images, stack_weighted_frames,
};
use crate::stacking::registration::config::Config as RegistrationConfig;
use crate::stacking::registration::{register, warp};
use crate::stacking::star_detection::config::Config as StarDetectionConfig;
use crate::stacking::star_detection::detector::StarDetector;
use crate::stacking::star_detection::star::Star;

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

/// Outcome of [`align_and_stack`].
#[derive(Debug)]
pub struct AlignStackResult {
    /// The combined image.
    pub image: AstroImage,
    /// Per-pixel fraction of frames that contributed, `[0,1]` — `< 1` along warped-frame borders.
    pub coverage: Buffer2<f32>,
    /// Per-pixel WHT (`Σ wᵢcᵢ`): each pixel's absolute statistical weight.
    pub weight: Buffer2<f32>,
    /// Per-pixel output variance per unit input variance (`Σwᵢ²/(Σwᵢ)²`). See [`crate::StackResult`].
    pub variance: Buffer2<f32>,
    /// Index (into the input) of the reference frame the others were aligned to.
    pub reference: usize,
    /// Number of frames that went into the stack: the reference plus every frame that
    /// registered successfully.
    pub registered: usize,
    /// Indices of frames dropped because they failed to register, ascending.
    pub dropped: Vec<usize>,
}

impl AlignStackResult {
    /// Wrap a combine [`StackResult`] with the alignment bookkeeping (shared by the RAM and
    /// streaming paths, which build this identically).
    fn from_stack(
        stacked: StackResult,
        reference: usize,
        registered: usize,
        dropped: Vec<usize>,
    ) -> Self {
        Self {
            image: stacked.image,
            coverage: stacked.coverage,
            weight: stacked.weight,
            variance: stacked.variance,
            reference,
            registered,
            dropped,
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
    Stack(#[from] StackError),
}

/// Detect → register → warp → stack a set of light frames into one aligned, combined image.
///
/// All frames are expected to share the same dimensions (same sensor). The reference frame is
/// added to the stack unwarped; every other frame is aligned to it. Frames that fail to
/// register (too few stars, RANSAC failure, accuracy gate) are dropped and listed in
/// [`AlignStackResult::dropped`]; the stack proceeds with whatever aligned. A single
/// input frame is returned as its own "stack".
pub fn align_and_stack(
    lights: Vec<AstroImage>,
    config: &AlignStackConfig,
    cancel: CancelToken,
) -> Result<AlignStackResult, Error> {
    if lights.is_empty() {
        return Err(Error::NoFrames);
    }

    // Detect stars on every frame. Each rayon task owns its detector — `detect` is `&mut`.
    let total = lights.len();
    tracing::info!(frames = total, "Detecting stars");
    let detected = AtomicUsize::new(0);
    let star_sets: Vec<Vec<Star>> = lights
        .par_iter()
        .map(|img| {
            // Cancelled: skip this frame's detection (cheap empty result); the
            // post-loop check below turns the run into `Cancelled`.
            if cancel.is_cancelled() {
                return Vec::new();
            }
            let result = StarDetector::from_config(config.detection.clone()).detect(img);
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
            result.stars
        })
        .collect();
    let total_stars: usize = star_sets.iter().map(|s| s.len()).sum();
    tracing::info!(total_stars, "Star detection complete");
    if cancel.is_cancelled() {
        return Err(Error::Stack(StackError::Cancelled));
    }

    let star_counts: Vec<usize> = star_sets.iter().map(|s| s.len()).collect();
    let reference = select_reference(
        &star_counts,
        config.reference,
        config.registration.required_stars(),
    )?;
    let ref_stars = &star_sets[reference];
    // The stacked master inherits the reference frame's (the alignment anchor's) metadata — captured
    // before `lights` is consumed by the warp, and matching the streaming path which combines with
    // `detected[reference].metadata`. (Otherwise the combine would derive it from frame 0.)
    let ref_metadata = lights[reference].metadata.clone();
    tracing::info!(
        reference,
        ref_stars = ref_stars.len(),
        "Reference frame selected"
    );

    // Register + warp every non-reference frame to the reference; the reference passes through
    // unwarped. `lights` is consumed **by value** (`into_par_iter`) so each input frame is freed as
    // its warped output is produced — we never hold the input set and the warped set at once, which
    // roughly halves the peak of this (the heaviest) stage. A frame that fails to register is dropped
    // (its index returned), not fatal.
    let n_lights = lights.len();
    let reg_total = n_lights - 1;
    tracing::info!(frames = reg_total, "Registering frames to the reference");
    let registered_so_far = AtomicUsize::new(0);
    let outcomes: Vec<Result<StackFrame, usize>> = lights
        .into_par_iter()
        .enumerate()
        .map(|(index, img)| {
            if index == reference {
                // Reference goes in unwarped → fully covered; `coverage: None` weights it 1
                // everywhere (no throwaway full-coverage map to allocate).
                return Ok(StackFrame {
                    image: img,
                    coverage: None,
                });
            }
            // Cancelled: drop this frame (skips the heavy register + warp); the
            // post-loop check below turns the run into `Cancelled`.
            if cancel.is_cancelled() {
                return Err(index);
            }
            let n = registered_so_far.fetch_add(1, Ordering::Relaxed) + 1;
            match register(ref_stars, &star_sets[index], &config.registration) {
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
                    let warped = warp(&img, &result.warp_transform(), &config.registration);
                    Ok(StackFrame {
                        image: warped.image,
                        coverage: Some(warped.coverage),
                    })
                    // `img` (the input frame) is dropped here, freeing its planes.
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

    Ok(AlignStackResult::from_stack(
        stacked, reference, registered, dropped,
    ))
}

/// Max light frames decoded+demosaiced concurrently. The RAW decode is the one
/// uninterruptible step, so this caps the work a cancel must drain and peak
/// memory; the demosaic (work-conserving across cores) keeps the pool busy
/// within a batch, so the cap costs little throughput.
const MAX_CONCURRENT_LIGHTS: usize = 4;

/// Rough per-frame working-set estimate, in f32 planes, for memory budgeting: a calibrated/warped
/// frame plus its in-flight scratch (detection buffers, or a warp's input+output). Used to reserve
/// RAM-path scratch headroom in the tier decision and to bound the streaming **warp** concurrency.
const PER_FRAME_WORKING_PLANES: usize = 8;

/// Per-frame working set for the streaming **decode→demosaic→detect** step, which additionally holds
/// the transient ~10-plane demosaic arena (`io::raw::demosaic`) on top of the output + detect
/// scratch. Larger than [`PER_FRAME_WORKING_PLANES`], so step 1 fans out less and doesn't overshoot.
const PER_FRAME_DECODE_PLANES: usize = 14;

/// The raw-light stack's memory-tier decision, as pure arithmetic over the sensor size, frame count,
/// worker count, and RAM budget — extracted from [`calibrate_align_stack`] so the budget logic is
/// testable without decoding a single frame. `fits_in_ram` picks the tier; the two concurrencies
/// bound the streaming path's fan-out when it doesn't.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MemoryPlan {
    /// All warped frames plus the RAM path's per-frame scratch fit the usable budget → take the
    /// all-in-memory path. Otherwise stream through the disk cache (memory-bounded, flat in N).
    fits_in_ram: bool,
    /// Streaming step 1 (decode → demosaic → detect) max in-flight frames: each holds the transient
    /// ~[`PER_FRAME_DECODE_PLANES`]-plane demosaic arena, so this fans out less than the warp step.
    decode_concurrency: usize,
    /// Streaming step 2 (register → warp) max in-flight frames: each holds
    /// ~[`PER_FRAME_WORKING_PLANES`] planes (input + output + scratch).
    warp_concurrency: usize,
}

/// Decide the memory tier for `frame_count` raw lights whose single-channel f32 plane is
/// `plane_bytes`, against a `threads`-wide pool and an `available`-byte RAM budget. Pure — no I/O, no
/// globals — so the budget invariants are unit-testable ([`mem_budget_tests`]).
fn plan_memory(
    plane_bytes: usize,
    frame_count: usize,
    threads: usize,
    available: u64,
) -> MemoryPlan {
    // Resident warped frame: 3 channels + coverage (conservative for a mono sensor).
    let warped_bytes = 4 * plane_bytes;
    // The RAM path runs detection/warp at full core count, each concurrent frame holding the
    // per-frame working set of scratch. Reserve that worst case so the RAM path is only taken when
    // the *frames and the scratch* fit — otherwise stream, which is memory-bounded (validated
    // flat-in-N). This keeps a scratch overshoot from OOMing a stack whose frames alone would fit,
    // on a high-core machine.
    let concurrent = frame_count.min(threads);
    let scratch_reserve = (PER_FRAME_WORKING_PLANES as u64)
        .saturating_mul(plane_bytes as u64)
        .saturating_mul(concurrent as u64);
    let frame_budget = available.saturating_sub(scratch_reserve);
    let fits_in_ram = fits_in_memory(warped_bytes, frame_count, frame_budget);

    // Streaming fan-out: both steps spill to disk (0 resident frames), so headroom is divided by the
    // per-decode transient. Decode holds the demosaic arena on top of the warp working set, so it
    // fans out less and can't overshoot.
    let decode_concurrency = compute_load_concurrency(
        plane_bytes,
        PER_FRAME_DECODE_PLANES * plane_bytes,
        0,
        available,
        threads,
    );
    let warp_concurrency = compute_load_concurrency(
        plane_bytes,
        PER_FRAME_WORKING_PLANES * plane_bytes,
        0,
        available,
        threads,
    );

    MemoryPlan {
        fits_in_ram,
        decode_concurrency,
        warp_concurrency,
    }
}

/// Calibrate, align, and stack raw light frames end to end — the full pipeline in one call.
///
/// For each raw light (in parallel): load it as a `CfaImage`, apply `masters`
/// (dark/flat/defect) in place, demosaic to an `AstroImage`, then hand the calibrated frames
/// to [`align_and_stack`]. A frame that fails to **load** is a hard error (bad input); a frame
/// that fails to **register** is dropped and reported in [`AlignStackResult::dropped`].
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

/// A detected-but-not-yet-warped frame in the streaming path: its stars (kept in RAM) plus its
/// calibrated channels spilled to disk (mmap), so no pixel data of the full set stays resident.
/// `metadata`/`dimensions` are the small per-frame headers captured before the image was dropped.
struct DetectedFrame {
    stars: Vec<Star>,
    calibrated: ArrayVec<Plane, 3>,
    metadata: AstroImageMetadata,
    dimensions: ImageDimensions,
}

/// Memory-bounded `calibrate → align → stack` for sets that don't fit ~75% RAM. Spills calibrated
/// and warped frames to the disk cache (mmap), so peak RAM is `concurrency × one-frame-working-set`
/// plus the combine's chunk window — flat in the frame count. Reuses the combine cache's tiering
/// primitives; the disk dir is removed when the combine's cache drops.
fn calibrate_align_stack_streaming<P: AsRef<Path> + Sync>(
    light_paths: &[P],
    masters: &CalibrationMasters,
    config: &AlignStackConfig,
    cancel: CancelToken,
    plan: MemoryPlan,
) -> Result<AlignStackResult, Error> {
    let total = light_paths.len();
    let cache_dir = config.stack.cache.cache_dir.clone();
    std::fs::create_dir_all(&cache_dir).map_err(|source| {
        Error::Stack(StackError::CreateCacheDir {
            path: cache_dir.clone(),
            source,
        })
    })?;
    // Fan-out for the two streaming steps was sized in `plan_memory` (decode holds the extra
    // demosaic arena, so it fans out less than the warp step). Both stream to disk, so peak RAM is
    // `concurrency × one-frame-working-set` plus the combine's chunk window — flat in the frame count.
    let MemoryPlan {
        decode_concurrency,
        warp_concurrency,
        ..
    } = plan;

    // --- Step 1: decode → calibrate → demosaic → detect → spill calibrated. ---
    tracing::info!(
        frames = total,
        concurrency = decode_concurrency,
        "Streaming: calibrating + detecting (spilling to disk)"
    );
    let done = AtomicUsize::new(0);
    let indexed: Vec<(usize, &P)> = light_paths.iter().enumerate().collect();
    let detected: Vec<DetectedFrame> =
        try_par_map_limited(&indexed, decode_concurrency, |&(idx, ref path)| {
            if cancel.is_cancelled() {
                return Err(Error::Stack(StackError::Cancelled));
            }
            let image = decode_calibrate_demosaic(path.as_ref(), masters, config, &cancel)?;
            let dimensions = image.dimensions();
            let metadata = image.metadata.clone();
            let stars = StarDetector::from_config(config.detection.clone())
                .detect(&image)
                .stars;
            let calibrated =
                spill_channels(&cache_dir, &format!("calib_{idx}"), &image, dimensions)
                    .map_err(Error::Stack)?;
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            tracing::info!(
                frame = n,
                total,
                stars = stars.len(),
                "calibrated + detected"
            );
            Ok(DetectedFrame {
                stars,
                calibrated,
                metadata,
                dimensions,
            })
        })?;
    if cancel.is_cancelled() {
        return Err(Error::Stack(StackError::Cancelled));
    }

    let dimensions = detected[0].dimensions;
    let star_counts: Vec<usize> = detected.iter().map(|d| d.stars.len()).collect();
    let reference = select_reference(
        &star_counts,
        config.reference,
        config.registration.required_stars(),
    )?;
    let metadata = detected[reference].metadata.clone();
    let ref_stars = &detected[reference].stars;
    tracing::info!(
        reference,
        ref_stars = ref_stars.len(),
        "Reference selected (streaming)"
    );

    // --- Step 2: load calibrated (mmap) → register + warp → spill warped → free calibrated. ---
    tracing::info!(
        frames = total - 1,
        "Streaming: registering + warping (spilling to disk)"
    );
    let registered_so_far = AtomicUsize::new(0);
    let indices: Vec<usize> = (0..total).collect();
    let outcomes: Vec<Option<(WeightedFrame, FrameStats)>> =
        try_par_map_limited(&indices, warp_concurrency, |&idx| {
            if cancel.is_cancelled() {
                // Treated as dropped here; the post-loop check turns the run into `Cancelled`.
                return Ok(None);
            }
            let d = &detected[idx];
            let calib = image_from_spilled_channels(&d.calibrated, dimensions);
            let base = format!("warped_{idx}");
            let spilled = if idx == reference {
                // Reference goes in unwarped → fully covered (`coverage: None`).
                Some(spill_weighted_frame(
                    &cache_dir, &base, calib, None, dimensions,
                ))
            } else {
                let n = registered_so_far.fetch_add(1, Ordering::Relaxed) + 1;
                match register(ref_stars, &d.stars, &config.registration) {
                    Ok(reg) => {
                        let warped = warp(&calib, &reg.warp_transform(), &config.registration);
                        tracing::info!(
                            frame = n,
                            total = total - 1,
                            inliers = reg.num_inliers,
                            "registered (streaming)"
                        );
                        Some(spill_weighted_frame(
                            &cache_dir,
                            &base,
                            warped.image,
                            Some(warped.coverage),
                            dimensions,
                        ))
                    }
                    Err(error) => {
                        tracing::info!(frame = n, total = total - 1, %error, "registration failed");
                        None
                    }
                }
            };
            // The calibrated spill is no longer needed once warped — free it to keep peak temp disk
            // at ~`N × warped frame`, not `N × (calibrated + warped)`.
            remove_spilled_channels(&cache_dir, &format!("calib_{idx}"), dimensions.channels);
            match spilled {
                Some(Ok(frame)) => Ok(Some(frame)),
                Some(Err(e)) => Err(Error::Stack(e)), // a spill I/O failure is fatal
                None => Ok(None),                     // registration drop
            }
        })?;
    if cancel.is_cancelled() {
        return Err(Error::Stack(StackError::Cancelled));
    }

    // `try_par_map_limited` preserves input order, so the position is the frame index.
    let mut frames = Vec::with_capacity(outcomes.len());
    let mut frame_stats = Vec::with_capacity(outcomes.len());
    let mut dropped = Vec::new();
    for (idx, outcome) in outcomes.into_iter().enumerate() {
        match outcome {
            Some((frame, stats)) => {
                frames.push(frame);
                frame_stats.push(stats);
            }
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
    let stacked = stack_weighted_frames(
        frames,
        frame_stats,
        Some(cache_dir),
        dimensions,
        metadata,
        config.stack.clone(),
        ProgressCallback::default(),
        cancel,
    )
    .map_err(Error::Stack)?;

    Ok(AlignStackResult::from_stack(
        stacked, reference, registered, dropped,
    ))
}

#[cfg(test)]
mod mem_budget_tests;

#[cfg(test)]
mod tests {
    use super::*;
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

        assert_eq!(result.reference, 0);
        assert_eq!(result.registered, 3, "all three frames should stack");
        assert!(result.dropped.is_empty(), "dropped: {:?}", result.dropped);

        // Alignment check: every frame was warped back to the reference, so the reference's
        // brightest star must reappear at the same place in the combined image.
        let mut det = StarDetector::from_config(StarDetectionConfig::default());
        let ref_pos = det.detect(&base).stars[0].pos;
        let stack_stars = det.detect(&result.image).stars;
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

        assert_eq!(result.dropped, vec![2], "blank frame should be dropped");
        assert_eq!(result.registered, 2, "reference + one aligned frame");
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

        assert_eq!(result.reference, 1);
        assert_eq!(
            result.image.metadata.exposure_time,
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
            result.reference, 0,
            "Auto must not anchor on the near-blank frame"
        );
        assert_eq!(
            result.dropped,
            vec![0],
            "the near-blank frame can't register"
        );
    }

    #[test]
    fn empty_input_errors() {
        let err = align_and_stack(
            Vec::new(),
            &AlignStackConfig::default(),
            CancelToken::never(),
        )
        .unwrap_err();
        assert!(matches!(err, Error::NoFrames));
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
        assert!(result.image.width() > 0 && result.image.height() > 0);
        assert_eq!(result.registered + result.dropped.len(), lights.len());
        assert!(result.registered >= 1, "at least the reference is stacked");
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

        assert_eq!(ram.registered, disk.registered, "same frames stacked");
        assert_eq!(ram.dropped, disk.dropped, "same frames dropped");
        assert_eq!(ram.reference, disk.reference, "same reference");
        assert_eq!(ram.image.dimensions(), disk.image.dimensions());
        // Bit-identical: same frames, same (seeded) registration, same combine — only the frame
        // storage (RAM vs mmap) differs.
        for c in 0..ram.image.channels() {
            let a: Vec<u32> = ram
                .image
                .channel(c)
                .pixels()
                .iter()
                .map(|x| x.to_bits())
                .collect();
            let b: Vec<u32> = disk
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
