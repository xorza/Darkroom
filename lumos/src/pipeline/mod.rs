//! End-to-end registered stacking — the pipeline's single deliverable in one call.
//!
//! [`align_and_stack`] runs the full alignment + combine flow over a set of light frames:
//! detect stars → choose a reference → register every other frame to it → warp the ones that
//! solve → combine with [`stack_images`]. Frames that fail to register are dropped and
//! reported in [`AlignStackResult::dropped`] rather than aborting the stack.

use rayon::prelude::*;

use crate::astro_image::AstroImage;
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

/// Errors from [`align_and_stack`].
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no light frames provided")]
    NoFrames,
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
    let star_sets: Vec<Vec<Star>> = lights
        .par_iter()
        .map(|img| {
            StarDetector::from_config(config.detection.clone())
                .detect(img)
                .stars
        })
        .collect();

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

    // Register + warp every non-reference frame to the reference. A frame that fails to solve
    // is dropped (its index returned), not fatal.
    let outcomes: Vec<Result<AstroImage, usize>> = lights
        .par_iter()
        .enumerate()
        .filter(|(index, _)| *index != reference)
        .map(
            |(index, img)| match register(ref_stars, &star_sets[index], &config.registration) {
                Ok(result) => Ok(warp(img, &result.warp_transform(), &config.registration).image),
                Err(_) => Err(index),
            },
        )
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
    let image = stack_images(frames, config.stack.clone(), ProgressCallback::default())?;

    Ok(AlignStackResult {
        image,
        reference,
        registered,
        dropped,
    })
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
        let blank = AstroImage::from_pixels(dims, vec![0.1; dims.width * dims.height]);
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
        let sparse = AstroImage::from_pixels(dims, vec![0.1; dims.width * dims.height]);
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
}
