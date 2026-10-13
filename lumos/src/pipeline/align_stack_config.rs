//! [`AlignStackConfig`]: the configuration of a registered stack, and its entries.

use std::cmp::Reverse;
use std::path::Path;

use crate::calibration::calibration_masters::CalibrationMasters;
use crate::calibration::cosmic_ray::cosmic_ray_config::CosmicRayConfig;
use crate::combine::error::StackConfigError;
use crate::combine::stack_config::{StackConfig, Weighting};
use crate::image::linear_image::LinearImage;
use crate::ingest::ingest_config::IngestConfig;
use crate::ingest::ingest_run::IngestRun;
use crate::pipeline::align_stack_result::AlignStackResult;
use crate::pipeline::align_stack_run::AlignStackRun;
use crate::pipeline::calibration_notes::CalibrationNotes;
use crate::pipeline::error::AlignStackError;
use crate::pipeline::light_source::{LightFiles, LightSource, MosaicPreparation, Preparation};
use crate::progress::progress_monitor::ProgressMonitor;
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

/// What [`Reference::select`] weighs of one frame: its detected stars, and their median FWHM.
#[derive(Debug, Clone, Copy)]
pub(super) struct ReferenceCandidate {
    pub(super) stars: usize,
    pub(super) median_fwhm: Option<f32>,
}

impl Reference {
    /// The index of the reference (alignment anchor) among `candidates`, which must have the
    /// `required` stars registration needs.
    ///
    /// `Auto` takes the sharpest frame among those with `required` stars: the lowest median FWHM,
    /// as Siril chooses, ties to the lowest index. Every other frame is warped onto it, so its
    /// seeing is what the stack is resampled to; the star count only has to clear the gate
    /// registration needs. When no frame clears it, the one with the most stars is reported.
    pub(super) fn select(
        self,
        candidates: &[ReferenceCandidate],
        required: usize,
    ) -> Result<usize, AlignStackError> {
        let index = match self {
            Self::Index(index) => {
                if index >= candidates.len() {
                    return Err(AlignStackError::ReferenceOutOfRange {
                        index,
                        count: candidates.len(),
                    });
                }
                index
            }
            Self::Auto => candidates
                .iter()
                .enumerate()
                .filter(|(_, candidate)| candidate.stars >= required)
                .filter_map(|(i, candidate)| candidate.median_fwhm.map(|fwhm| (i, fwhm)))
                .reduce(|best, candidate| {
                    if candidate.1 < best.1 {
                        candidate
                    } else {
                        best
                    }
                })
                .map_or_else(
                    || {
                        (0..candidates.len())
                            .max_by_key(|&i| (candidates[i].stars, Reverse(i)))
                            .expect("a run has one candidate at least")
                    },
                    |(i, _)| i,
                ),
        };
        let found = candidates[index].stars;
        if found < required {
            return Err(AlignStackError::ReferenceInsufficientStars {
                index,
                found,
                required,
            });
        }
        Ok(index)
    }
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
        monitor: ProgressMonitor,
    ) -> Result<AlignStackResult, AlignStackError> {
        if lights.is_empty() {
            return Err(AlignStackError::NoFrames);
        }
        self.validate_linear(lights.len())?;
        let run = self.begin(ingest, monitor);
        let detected = LightSource::<&Path>::Held(lights).detect(&run)?;
        run.register_warp_and_stack(detected)
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
        monitor: ProgressMonitor,
    ) -> Result<AlignStackResult, AlignStackError> {
        if light_paths.is_empty() {
            return Err(AlignStackError::NoFrames);
        }
        self.validate_linear(light_paths.len())?;
        let run = self.begin(ingest, monitor);
        LightFiles::new(light_paths, Preparation::Linear, &run.ingest)?.stack(&run)
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
        monitor: ProgressMonitor,
    ) -> Result<AlignStackResult, AlignStackError> {
        if light_paths.is_empty() {
            return Err(AlignStackError::NoFrames);
        }
        self.validate(light_paths.len())?;
        let run = self.begin(ingest, monitor);
        let notes = CalibrationNotes::default();
        let preparation = Preparation::Calibrate(MosaicPreparation {
            masters,
            cosmic_ray: self.cosmic_ray.as_ref(),
            notes: &notes,
        });
        let mut result = LightFiles::new(light_paths, preparation, &run.ingest)?.stack(&run)?;
        notes.report_into(&mut result.product.report, masters);
        Ok(result)
    }

    /// A registered stack under this configuration, taking its memory reading and decode policy
    /// from `ingest`, and its progress and cancel token from `monitor`, now.
    fn begin(&self, ingest: &IngestConfig, monitor: ProgressMonitor) -> AlignStackRun<'_> {
        AlignStackRun {
            config: self,
            ingest: IngestRun::new(ingest, monitor),
        }
    }

    /// Validate every stage's configuration for a run over `frame_count` lights.
    ///
    /// Each stage validates its own config where it runs, but by then the run has paid for
    /// everything upstream — and the registration stage cannot report a config problem at all,
    /// because it returns the same error type for "this config is invalid" and "these two star
    /// catalogs don't match", and the pipeline reads the latter as a frame to drop. Checking every
    /// stage here means a bad config is reported as one, before any frame is decoded. Manual
    /// weights are given one per input light, so their count is checked against the lights too.
    fn validate(&self, frame_count: usize) -> Result<(), AlignStackError> {
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
    fn validate_linear(&self, frame_count: usize) -> Result<(), AlignStackError> {
        if self.cosmic_ray.is_some() {
            return Err(AlignStackError::CosmicRayOnLinearLights);
        }
        self.validate(frame_count)
    }
}

#[cfg(test)]
mod tests {
    use crate::pipeline::align_stack_config::{Reference, ReferenceCandidate};
    use crate::pipeline::error::AlignStackError;

    /// `Auto` takes the lowest median FWHM among the frames with the stars registration needs:
    /// frame 2 at 2.5 of 2.5, 3.0 and 3.5; a tie goes to the lowest index; a frame below the gate
    /// is passed over however sharp, frame 1 at 2.0 with 30 of 40 stars; and when none clears the
    /// gate, the one with the most stars is the one reported. An index must lie in the input.
    #[test]
    fn auto_takes_the_sharpest_frame_with_enough_stars() {
        let candidates = |counts: &[usize], fwhms: &[f32]| -> Vec<ReferenceCandidate> {
            counts
                .iter()
                .zip(fwhms)
                .map(|(&stars, &fwhm)| ReferenceCandidate {
                    stars,
                    median_fwhm: Some(fwhm),
                })
                .collect()
        };
        let auto = |counts: &[usize], fwhms: &[f32]| {
            Reference::Auto.select(&candidates(counts, fwhms), 40)
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
            Reference::Index(5).select(&candidates(&[50, 60], &[3.0, 3.0]), 40),
            Err(AlignStackError::ReferenceOutOfRange { index: 5, count: 2 })
        ));
    }
}
