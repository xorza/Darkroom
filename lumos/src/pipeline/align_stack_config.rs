//! Configuration for registered stacking pipelines.

use std::path::Path;

use common::CancelToken;

use crate::calibration::calibration_masters::CalibrationMasters;
use crate::calibration::cosmic_ray::cosmic_ray_config::CosmicRayConfig;
use crate::combine::error::StackConfigError;
use crate::combine::stack_config::{StackConfig, Weighting};
use crate::image::linear_image::LinearImage;
use crate::ingest::ingest_config::IngestConfig;
use crate::pipeline::align_stack_result::AlignStackResult;
use crate::pipeline::error::AlignStackError;
use crate::pipeline::{align, calibrate};
use crate::progress::progress_callback::ProgressCallback;
use crate::registration::registration_config::RegistrationConfig;
use crate::star_detection::star_detection_config::StarDetectionConfig;

/// How the reference frame (the alignment anchor) is chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Reference {
    /// The sharpest frame — the lowest median FWHM — among those with the stars registration
    /// needs, ties to the lowest index.
    #[default]
    Auto,
    /// A specific frame, by index into the input slice.
    Index(usize),
}

/// One configuration per pipeline stage, and the reference choice.
#[derive(Debug, Clone)]
pub struct AlignStackConfig {
    pub detection: StarDetectionConfig,
    pub registration: RegistrationConfig,
    /// How the registered lights combine: [`StackConfig::light`] by default.
    pub stack: StackConfig,
    pub reference: Reference,
    /// Optional single-frame cosmic-ray rejection after calibration and before demosaic, for mono
    /// and Bayer lights; X-Trans lights are refused (see
    /// [`CosmicRayError::XTransUnsupported`](crate::CosmicRayError::XTransUnsupported)).
    pub cosmic_ray: Option<CosmicRayConfig>,
}

impl Default for AlignStackConfig {
    /// The default of every stage, the lights combined as [`StackConfig::light`].
    fn default() -> Self {
        Self {
            detection: StarDetectionConfig::default(),
            registration: RegistrationConfig::default(),
            stack: StackConfig::light(),
            reference: Reference::default(),
            cosmic_ray: None,
        }
    }
}

impl AlignStackConfig {
    /// Detect → register → warp → stack a set of light frames into one aligned, combined image.
    ///
    /// All frames are expected to share the same dimensions (same sensor). The reference frame is
    /// added to the stack unwarped; every other frame is aligned to it. Frames that fail to
    /// register (too few stars, RANSAC failure, accuracy gate) are dropped and listed in
    /// [`AlignmentSummary::dropped`](crate::AlignmentSummary::dropped); the stack proceeds with
    /// whatever aligned. A single input frame is returned as its own "stack".
    ///
    /// Every light must share the first one's dimensions and hold only finite samples; each is
    /// checked before its stars are detected, so a bad input is reported as one instead of
    /// surfacing from the combine after a warp has spread it.
    ///
    /// The frames arrive decoded and resident, so the inputs are committed before this is called —
    /// but the warped outputs are a second full set, and those spill to the frame store when they
    /// would not fit alongside the inputs. For camera RAW, enter through
    /// [`Self::calibrate_align_stack_paths`]
    /// instead, which tiers the decode as well.
    pub fn align_stack_images(
        &self,
        lights: Vec<LinearImage>,
        ingest: &IngestConfig,
        progress: ProgressCallback,
        cancel: CancelToken,
    ) -> Result<AlignStackResult, AlignStackError> {
        align::align_stack_images(lights, self, ingest, progress, cancel)
    }

    /// Detect → register → warp → stack calibrated linear lights read from files: FITS or
    /// floating-point TIFF, as [`LinearImage::from_file`] reads them — the output of Siril or
    /// PixInsight, for one.
    ///
    /// [`Self::align_stack_images`] for frames on disk: each light is decoded on a worker, checked,
    /// and its stars detected, and when the set would not fit the memory it plans against, parked
    /// and warped through the frame store as [`Self::calibrate_align_stack_paths`] parks its
    /// lights, so peak RAM stays flat in the frame count. With [`Reference::Index`] each light goes
    /// through one pass, written once.
    pub fn align_stack_paths<P: AsRef<Path> + Sync>(
        &self,
        light_paths: &[P],
        ingest: &IngestConfig,
        progress: ProgressCallback,
        cancel: CancelToken,
    ) -> Result<AlignStackResult, AlignStackError> {
        align::align_stack_paths(light_paths, self, ingest, progress, cancel)
    }

    /// Calibrate, align, and stack camera-RAW or mosaic-FITS light frames end to end.
    ///
    /// For each raw light: load it as a `CfaImage`, apply `masters` (dark/flat/defect) in place,
    /// demosaic to a `LinearImage`, and detect its stars, then register, warp and combine. A frame
    /// that fails to **load** is a hard error (bad input); a frame that fails to **register** is
    /// dropped and reported in [`AlignmentSummary::dropped`](crate::AlignmentSummary::dropped).
    ///
    /// With [`Reference::Index`] the reference is prepared first, and each other light then goes
    /// through its preparation, its registration and its warp in one pass, written once. With
    /// [`Reference::Auto`] every light is prepared and detected first, since the reference is the
    /// sharpest one, and parked until it registers.
    ///
    /// The sensor geometry is peeked from the first frame's header without a decode, so the memory
    /// tier is chosen before any pixels are read. When the frame set plus its per-frame scratch
    /// won't fit the budget, every parked and warped frame goes through the frame store's memory
    /// maps and peak RAM stays flat in the frame count.
    ///
    /// For lights already calibrated (e.g. pre-processed FITS), skip this and call
    /// [`Self::align_stack_paths`], or
    /// [`Self::align_stack_images`] for frames already decoded.
    pub fn calibrate_align_stack_paths<P: AsRef<Path> + Sync>(
        &self,
        light_paths: &[P],
        masters: &CalibrationMasters,
        ingest: &IngestConfig,
        progress: ProgressCallback,
        cancel: CancelToken,
    ) -> Result<AlignStackResult, AlignStackError> {
        calibrate::calibrate_align_stack_paths(light_paths, masters, self, ingest, progress, cancel)
    }

    /// Validate every stage's configuration for a run over `frame_count` lights.
    ///
    /// Each stage validates its own config where it runs, but by then the run has paid for
    /// everything upstream — and the registration stage cannot report a config problem at all,
    /// because it returns the same error type for "this config is invalid" and "these two star
    /// catalogs don't match", and the pipeline reads the latter as a frame to drop. Checking every
    /// stage here means a bad config is reported as one, before any frame is decoded. Manual
    /// weights are given one per input light, so their count is checked against the lights too.
    pub(super) fn validate(&self, frame_count: usize) -> Result<(), AlignStackError> {
        self.detection
            .validate()
            .map_err(AlignStackError::DetectionConfig)?;
        self.registration
            .validate()
            .map_err(AlignStackError::RegistrationConfig)?;
        self.stack
            .validate()
            .map_err(|source| AlignStackError::Stack(source.into()))?;
        if let Weighting::Manual(weights) = &self.stack.weighting
            && weights.len() != frame_count
        {
            return Err(AlignStackError::Stack(
                StackConfigError::ManualWeightCountMismatch {
                    expected: frame_count,
                    actual: weights.len(),
                }
                .into(),
            ));
        }
        if let Some(cosmic_ray) = &self.cosmic_ray {
            cosmic_ray
                .validate()
                .map_err(AlignStackError::CosmicRayConfig)?;
        }
        Ok(())
    }

    /// [`Self::validate`] for lights already linear, which no cosmic-ray pass can run on.
    pub(super) fn validate_linear(&self, frame_count: usize) -> Result<(), AlignStackError> {
        if self.cosmic_ray.is_some() {
            return Err(AlignStackError::CosmicRayOnLinearLights);
        }
        self.validate(frame_count)
    }
}
