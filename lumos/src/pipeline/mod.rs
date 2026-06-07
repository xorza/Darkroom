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

use crate::astro_image::AstroImage;
use crate::astro_image::error::ImageError;
use crate::calibration_masters::CalibrationMasters;
use crate::raw::load_raw_cfa;
use crate::registration::config::Config as RegistrationConfig;
use crate::registration::{register, warp};
use crate::stacking::config::StackConfig;
use crate::stacking::error::Error as StackError;
use crate::stacking::progress::ProgressCallback;
use crate::stacking::stack::stack_images;
use crate::star_detection::config::Config as StarDetectionConfig;
use crate::star_detection::detector::StarDetector;
use crate::star_detection::star::Star;

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
}

/// Outcome of [`align_and_stack`].
#[derive(Debug)]
pub struct AlignStackResult {
    /// The combined image.
    pub image: AstroImage,
    /// Index (into the input) of the reference frame the others were aligned to.
    pub reference: usize,
    /// Number of frames that went into the stack: the reference plus every frame that
    /// registered successfully.
    pub registered: usize,
    /// Indices of frames dropped because they failed to register, ascending.
    pub dropped: Vec<usize>,
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

    let reference = match config.reference {
        Reference::Index(index) => {
            if index >= lights.len() {
                return Err(Error::ReferenceOutOfRange {
                    index,
                    count: lights.len(),
                });
            }
            index
        }
        // Most stars → most anchors for the other frames to match against.
        Reference::Auto => star_sets
            .iter()
            .enumerate()
            .max_by_key(|(_, stars)| stars.len())
            .map(|(index, _)| index)
            .expect("lights is non-empty"),
    };

    let ref_stars = &star_sets[reference];
    if ref_stars.len() < config.registration.min_stars {
        return Err(Error::ReferenceInsufficientStars {
            index: reference,
            found: ref_stars.len(),
            required: config.registration.min_stars,
        });
    }
    tracing::info!(
        reference,
        ref_stars = ref_stars.len(),
        "Reference frame selected"
    );

    // Register + warp every non-reference frame to the reference. A frame that fails to solve
    // is dropped (its index returned), not fatal.
    let reg_total = lights.len() - 1;
    tracing::info!(frames = reg_total, "Registering frames to the reference");
    let registered_so_far = AtomicUsize::new(0);
    let outcomes: Vec<Result<AstroImage, usize>> = lights
        .par_iter()
        .enumerate()
        .filter(|(index, _)| *index != reference)
        .map(|(index, img)| {
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
                    Ok(warp(img, &result.warp_transform(), &config.registration).image)
                }
                Err(error) => {
                    tracing::info!(frame = n, total = reg_total, %error, "registration failed");
                    Err(index)
                }
            }
        })
        .collect();

    let mut frames: Vec<AstroImage> = Vec::with_capacity(outcomes.len() + 1);
    let mut dropped = Vec::new();
    for outcome in outcomes {
        match outcome {
            Ok(warped) => frames.push(warped),
            Err(index) => dropped.push(index),
        }
    }
    dropped.sort_unstable();
    tracing::info!(
        aligned = frames.len(),
        dropped = dropped.len(),
        "Registration complete"
    );

    // Every non-reference frame dropped → nothing aligned. (A lone reference frame is fine.)
    if frames.is_empty() && lights.len() > 1 {
        return Err(Error::AllFramesDropped {
            count: lights.len() - 1,
        });
    }

    // The reference goes in unwarped — move it out of `lights` (which is no longer borrowed).
    let reference_image = lights
        .into_iter()
        .nth(reference)
        .expect("reference index is in range");
    frames.push(reference_image);

    let registered = frames.len();
    tracing::info!(frames = registered, "Stacking aligned frames");
    let image = stack_images(frames, config.stack.clone(), ProgressCallback::default())?;
    tracing::info!("Stack complete");

    Ok(AlignStackResult {
        image,
        reference,
        registered,
        dropped,
    })
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
) -> Result<AlignStackResult, Error> {
    let total = light_paths.len();
    tracing::info!(
        frames = total,
        "Loading, calibrating and demosaicing raw lights (RAW decode — the slow phase)"
    );
    let done = AtomicUsize::new(0);
    let calibrated: Vec<AstroImage> = light_paths
        .par_iter()
        .map(|path| {
            let mut cfa = load_raw_cfa(path.as_ref()).map_err(|source| Error::Load {
                path: path.as_ref().to_path_buf(),
                source,
            })?;
            masters.calibrate(&mut cfa);
            let image = cfa.demosaic();
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            tracing::info!(frame = n, total, "calibrated light");
            Ok(image)
        })
        .collect::<Result<Vec<_>, Error>>()?;

    align_and_stack(calibrated, config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::astro_image::ImageDimensions;
    use crate::registration::transform::{Transform, WarpTransform};
    use crate::testing::synthetic::star_field::{self, StarFieldConfig};
    use glam::DVec2;

    fn base_field() -> (AstroImage, RegistrationConfig) {
        let cfg = StarFieldConfig {
            width: 256,
            height: 256,
            num_stars: 40,
            seed: 66666,
            ..star_field::sparse_field_config()
        };
        let (pixels, _) = star_field::generate_star_field(&cfg);
        let image = AstroImage::from_pixels(
            ImageDimensions::new(cfg.width, cfg.height, 1),
            pixels.to_vec(),
        );
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
        let result = align_and_stack(frames, &config).expect("stack");

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
        let result = align_and_stack(frames, &config).expect("stack");

        assert_eq!(result.dropped, vec![2], "blank frame should be dropped");
        assert_eq!(result.registered, 2, "reference + one aligned frame");
    }

    #[test]
    fn auto_reference_picks_the_richest_frame() {
        let (base, reg) = base_field();
        // Frame 1 (full field) has far more stars than frame 0 (a near-blank), so Auto must
        // anchor on frame 1.
        let dims = base.dimensions;
        let sparse = AstroImage::from_pixels(dims, vec![0.1; dims.size.x * dims.size.y]);
        let frames = vec![sparse, base.clone(), shifted(&base, &reg, 4.0, -3.0)];

        let result = align_and_stack(frames, &AlignStackConfig::default()).expect("stack");
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
        let err = align_and_stack(Vec::new(), &AlignStackConfig::default()).unwrap_err();
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

        let result = calibrate_align_stack(lights, &masters, &AlignStackConfig::default())
            .expect("calibrate_align_stack");

        // A real stacked image came out, and every input frame is accounted for.
        assert!(result.image.width() > 0 && result.image.height() > 0);
        assert_eq!(result.registered + result.dropped.len(), lights.len());
        assert!(result.registered >= 1, "at least the reference is stacked");
    }
}
