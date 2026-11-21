//! [`AlignStackConfig`]: the configuration of a registered stack, and its entries.

use std::cmp::Reverse;
use std::path::Path;

use crate::calibration::calibration_masters::CalibrationMasters;
use crate::calibration::cosmic_ray::cosmic_ray_config::CosmicRayConfig;
use crate::combine::stack_config::StackConfig;
use crate::image::linear_image::LinearImage;
use crate::ingest::ingest_config::IngestConfig;
use crate::ingest::ingest_run::IngestRun;
use crate::pipeline::align_stack_result::AlignStackResult;
use crate::pipeline::align_stack_run::AlignStackRun;
use crate::pipeline::calibration_notes::CalibrationNotes;
use crate::pipeline::error::{AlignStackConfigError, AlignStackError};
use crate::pipeline::light_files::{LightFiles, Preparation};
use crate::pipeline::light_source::LightSource;
use crate::pipeline::mosaic_preparation::MosaicPreparation;
use crate::progress::progress_monitor::ProgressMonitor;
use crate::registration::registrar::Registrar;
use crate::registration::registration_config::RegistrationConfig;
use crate::registration::resample::warp_config::WarpConfig;
use crate::star_detection::star_detection_config::StarDetectionConfig;

/// How the reference frame (the alignment anchor) is chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Reference {
    /// The sharpest frame — the lowest median FWHM — among those with the stars registration
    /// needs, ties to the lowest index.
    #[default]
    Auto,
    /// A specific frame, by index into the input slice.
    Idx(usize),
}

/// What [`Reference::select`] weighs of one frame: its detected stars, and their median FWHM.
#[derive(Debug, Clone, Copy)]
pub(super) struct ReferenceCandidate {
    pub(super) stars: usize,
    pub(super) median_fwhm: Option<f32>,
}

impl Reference {
    /// The index of the reference (alignment anchor) among `candidates`, one per light. An
    /// [`Self::Idx`] is in range: the run validated it against the lights.
    ///
    /// `Auto` takes the sharpest frame among those with the `required` stars registration needs:
    /// the lowest median FWHM, as Siril chooses, ties to the lowest index. Every other frame is
    /// warped onto it, so its seeing is what the stack is resampled to; the star count only has to
    /// clear the gate. When no frame clears it, the one with the most stars, which the reference
    /// catalog then refuses with the count.
    pub(super) fn select(self, candidates: &[ReferenceCandidate], required: usize) -> usize {
        match self {
            Self::Idx(idx) => {
                debug_assert!(idx < candidates.len());
                idx
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
        }
    }
}

/// One configuration per pipeline stage, and the reference choice.
#[derive(Debug, Clone)]
pub struct AlignStackConfig {
    pub detection: StarDetectionConfig,
    pub registration: RegistrationConfig,
    /// How a registered light is resampled onto the reference's grid.
    pub warp: WarpConfig,
    /// How the registered lights combine: [`StackConfig::light`] by default.
    pub stack: StackConfig,
    pub reference: Reference,
}

impl Default for AlignStackConfig {
    /// The default of every stage, the lights combined as [`StackConfig::light`].
    fn default() -> Self {
        Self {
            detection: StarDetectionConfig::default(),
            registration: RegistrationConfig::default(),
            warp: WarpConfig::default(),
            stack: StackConfig::light(),
            reference: Reference::default(),
        }
    }
}

/// What [`AlignStackConfig::calibrate_align_stack_paths`] stacks: the camera-RAW or mosaic-FITS
/// lights, and the masters each is calibrated by.
#[derive(Debug)]
pub struct CalibratedLights<'a, P> {
    pub paths: &'a [P],
    pub masters: &'a CalibrationMasters,
    /// Optional single-frame cosmic-ray rejection after calibration and before demosaic, for mono
    /// and Bayer lights; X-Trans lights are refused (see
    /// [`CosmicRayError::XTransUnsupported`](crate::CosmicRayError::XTransUnsupported)). A light
    /// already linear has no mosaic to reject them on, so only this entry takes the pass.
    pub cosmic_ray: Option<&'a CosmicRayConfig>,
}

/// Copied whatever its type parameters are, as it holds references alone: a derive would ask each
/// to be `Copy`.
impl<P> Clone for CalibratedLights<'_, P> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<P> Copy for CalibratedLights<'_, P> {}

impl AlignStackConfig {
    /// Detect → register → warp → stack a set of light frames into one aligned, combined image.
    ///
    /// The reference is stacked unwarped and every other frame is aligned to it. Frames that fail
    /// to register (too few stars, RANSAC failure, accuracy gate) are dropped and listed in
    /// [`AlignStackResult::dropped`]; a single input frame is its own "stack".
    ///
    /// Every light must share the first one's dimensions and hold only finite samples; each is
    /// checked before its stars are detected, so a bad input is reported as one instead of
    /// surfacing from the combine after a warp has spread it.
    ///
    /// The frames arrive decoded and resident, so the inputs are committed before this is called,
    /// but the warped outputs are a second full set, and those spill to the frame store when they
    /// would not fit alongside the inputs. For camera RAW, use
    /// [`Self::calibrate_align_stack_paths`], which tiers the decode as well.
    pub fn align_stack_images(
        &self,
        lights: Vec<LinearImage>,
        ingest: &IngestConfig,
        monitor: ProgressMonitor,
    ) -> Result<AlignStackResult, AlignStackError> {
        self.begin(lights.len(), ingest, monitor)?
            .stack(LightSource::Held(lights))
    }

    /// Detect → register → warp → stack calibrated linear lights read from files: FITS or
    /// floating-point TIFF, as [`LoadCtx::linear`](crate::LoadCtx::linear) reads them — the
    /// output of Siril or PixInsight, for one.
    ///
    /// [`Self::align_stack_images`] for frames on disk: each light is decoded on a worker, checked,
    /// and its stars detected, and when the set would not fit the memory it plans against, parked
    /// and warped through the frame store as [`Self::calibrate_align_stack_paths`] parks its
    /// lights, so peak RAM stays flat in the frame count. With [`Reference::Idx`] each light goes
    /// through one pass, written once.
    pub fn align_stack_paths<P: AsRef<Path>>(
        &self,
        light_paths: &[P],
        ingest: &IngestConfig,
        monitor: ProgressMonitor,
    ) -> Result<AlignStackResult, AlignStackError> {
        let light_paths: Vec<&Path> = light_paths.iter().map(AsRef::as_ref).collect();
        let run = self.begin(light_paths.len(), ingest, monitor)?;
        run.stack(LightSource::Files(Box::new(LightFiles::new(
            &light_paths,
            Preparation::Linear,
            &run.ingest,
        )?)))
    }

    /// Calibrate, align, and stack camera-RAW or mosaic-FITS light frames end to end.
    ///
    /// Each light is loaded as a `CfaImage`, calibrated by the masters, demosaiced and detected,
    /// then registered, warped and combined. A frame that fails to **load** is a hard error; one
    /// that fails to **register** is dropped and reported in [`AlignStackResult::dropped`].
    ///
    /// With [`Reference::Idx`] the reference is prepared first, and each other light then goes
    /// through its preparation, its registration and its warp in one pass, written once. With
    /// [`Reference::Auto`] every light is prepared and detected first, since the reference is the
    /// sharpest one, and parked until it registers.
    ///
    /// The sensor geometry is peeked from the first frame's header without a decode, so the memory
    /// tier is chosen before any pixels are read. When the frame set plus its per-frame scratch
    /// won't fit the budget, every parked and warped frame goes through the frame store's memory
    /// maps and peak RAM stays flat in the frame count.
    ///
    /// For lights already calibrated, call [`Self::align_stack_paths`], or
    /// [`Self::align_stack_images`] for frames already decoded.
    pub fn calibrate_align_stack_paths<P: AsRef<Path>>(
        &self,
        lights: CalibratedLights<'_, P>,
        ingest: &IngestConfig,
        monitor: ProgressMonitor,
    ) -> Result<AlignStackResult, AlignStackError> {
        let CalibratedLights {
            paths: light_paths,
            masters,
            cosmic_ray,
        } = lights;
        if let Some(cosmic_ray) = cosmic_ray {
            cosmic_ray
                .validate()
                .map_err(AlignStackConfigError::CosmicRay)?;
        }
        let light_paths: Vec<&Path> = light_paths.iter().map(AsRef::as_ref).collect();
        let run = self.begin(light_paths.len(), ingest, monitor)?;
        let notes = CalibrationNotes::default();
        let preparation = Preparation::Calibrate(MosaicPreparation {
            masters,
            cosmic_ray,
            notes: &notes,
        });
        let mut result = run.stack(LightSource::Files(Box::new(LightFiles::new(
            &light_paths,
            preparation,
            &run.ingest,
        )?)))?;
        result.product.report.calibration = Some(notes.report(masters));
        Ok(result)
    }

    /// A registered stack of `count` lights under this configuration, taking its memory reading
    /// and decode policy from `ingest`, and its progress and cancel token from `monitor`, now.
    ///
    /// # Errors
    ///
    /// [`AlignStackError::NoFrames`], or a configuration [`Self::validate`] refuses.
    fn begin<'a>(
        &'a self,
        count: usize,
        ingest: &'a IngestConfig,
        monitor: ProgressMonitor,
    ) -> Result<AlignStackRun<'a>, AlignStackError> {
        if count == 0 {
            return Err(AlignStackError::NoFrames);
        }
        self.validate(count)?;
        Ok(AlignStackRun {
            config: self,
            registrar: Registrar::checked(self.registration.clone())
                .map_err(AlignStackConfigError::Registration)?,
            ingest: IngestRun::new(ingest, monitor),
        })
    }

    /// Validate every stage's configuration for a run over `frame_count` lights, where the run is
    /// built: every stage after it takes the configuration as checked, so a failure past here is
    /// the data's. Manual weights are given one per light, so their count is checked against the
    /// lights too.
    fn validate(&self, frame_count: usize) -> Result<(), AlignStackConfigError> {
        self.detection
            .validate()
            .map_err(AlignStackConfigError::Detection)?;
        self.stack
            .validate()
            .map_err(AlignStackConfigError::Stack)?;
        if let Reference::Idx(idx) = self.reference
            && idx >= frame_count
        {
            return Err(AlignStackConfigError::ReferenceOutOfRange {
                idx,
                count: frame_count,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::ingest::ingest_config::IngestConfig;
    use crate::pipeline::align_stack_config::{AlignStackConfig, Reference, ReferenceCandidate};
    use crate::pipeline::error::{AlignStackConfigError, AlignStackError};
    use crate::progress::progress_monitor::ProgressMonitor;

    /// `Auto` takes the lowest median FWHM among the frames with the stars registration needs:
    /// frame 2 at 2.5 of 2.5, 3.0 and 3.5; a tie goes to the lowest index; a frame below the gate
    /// is passed over however sharp, frame 1 at 2.0 with 30 of 40 stars; and when none clears the
    /// gate, the one with the most stars is the one reported.
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
        assert_eq!(auto(&[50, 80, 60], &[3.0, 3.5, 2.5]), 2);
        assert_eq!(auto(&[50, 80, 60], &[2.5, 3.0, 2.5]), 0);
        assert_eq!(auto(&[50, 30, 60], &[3.0, 2.0, 3.5]), 0);
        assert_eq!(auto(&[10, 20, 15], &[3.0, 2.0, 3.5]), 1);
    }

    /// A reference index must lie among the lights, and is checked before any light is decoded:
    /// the path here does not exist, so a decode would have reported a load error instead.
    #[test]
    fn a_reference_index_past_the_lights_is_refused_before_decoding() {
        let config = AlignStackConfig {
            reference: Reference::Idx(1),
            ..AlignStackConfig::default()
        };
        let result = config.align_stack_paths(
            &[Path::new("does-not-exist.fits")],
            &IngestConfig::default(),
            ProgressMonitor::unattended(),
        );
        assert!(matches!(
            result,
            Err(AlignStackError::Config(
                AlignStackConfigError::ReferenceOutOfRange { idx: 1, count: 1 }
            ))
        ));
        assert!(config.validate(2).is_ok());
    }
}
