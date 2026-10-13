//! [`LightSource`]: where a registered stack's lights come from, and the stage that detects them.

use std::path::Path;

use crate::calibration::calibration_masters::CalibrationMasters;
use crate::calibration::cosmic_ray;
use crate::calibration::cosmic_ray::cosmic_ray_config::CosmicRayConfig;
use crate::combine::frame_cache::slots::Slots;
use crate::combine::normalization::planned_normalization::{PlannedNormalization, SetCoverage};
use crate::error::Cancelled;
use crate::frame::frame_peek::FramePeek;
use crate::frame::frame_stats::FrameStats;
use crate::frame::stackable_image::StackableImage;
use crate::frame_store::decode_cache::DecodeCache;
use crate::frame_store::error::KeptDecodeError;
use crate::frame_store::stored_layout::StoredLayout;
use crate::frame_store::tier::Tier;
use crate::image::cfa::cfa_frame_info::CfaFrameInfo;
use crate::image::cfa::cfa_image::CfaImage;
use crate::image::image_dimensions::ImageDimensions;
use crate::image::linear_image::LinearImage;
use crate::ingest::frame_intake::{FrameIntake, IntakePlan};
use crate::ingest::ingest_run::IngestRun;
use crate::io::error::ImageError;
use crate::io::load_context::LoadContext;
use crate::math::noise::mosaic_noise::MosaicNoise;
use crate::memory::DETECTION_WORKING_PLANES;
use crate::memory::footprint::Footprint;
use crate::memory::run_plan::RunPlan;
use crate::memory::stage::{Stage, Units};
use crate::pipeline::align_stack_config::{AlignStackConfig, Reference};
use crate::pipeline::align_stack_result::AlignStackResult;
use crate::pipeline::align_stack_run::AlignStackRun;
use crate::pipeline::calibration_notes::CalibrationNotes;
use crate::pipeline::detected_frame::DetectedFrame;
use crate::pipeline::detector_pool::DetectorPool;
use crate::pipeline::error::AlignStackError;
use crate::pipeline::frame_parking::{FrameParking, StagePlan};
use crate::pipeline::frame_registrar::{FrameRegistrar, LightSet, ParkedFrame};
use crate::pipeline::pipeline_frame::PipelineFrame;
use crate::pipeline::registered_set::RegisteredSet;
use crate::progress::stacking_progress::StackingStage;
use crate::star_detection::star::Star;
use crate::star_detection::star_detector::StarDetector;
use crate::star_detection::star_detector::detection_result::{
    DetectionDiagnostics, DetectionResult,
};

/// Where a registered stack's lights come from.
#[derive(Debug)]
pub(crate) enum LightSource<'a, P> {
    /// Frames the caller decoded and handed over. They were in memory before the run read the
    /// machine, so the plan charges them only what the run adds, and they stay resident whatever
    /// the tier: spilling them would be a write and a read-back for nothing.
    Held(Vec<LinearImage>),
    /// Lights read from files, each made ready on a worker.
    Files(LightFiles<'a, P>),
}

/// Lights read from files, what each goes through before its stars are detected, and the decode
/// cache they are read through when the run keeps one.
#[derive(Debug)]
pub(crate) struct LightFiles<'a, P> {
    paths: &'a [P],
    preparation: Preparation<'a>,
    kept: Option<DecodeCache>,
}

/// What a light read from a file goes through before its stars are detected.
#[derive(Debug)]
pub(crate) enum Preparation<'a> {
    /// A mosaic, calibrated, cleaned of cosmic rays and demosaiced.
    Calibrate(MosaicPreparation<'a>),
    /// An image already calibrated, taken as it decodes.
    Linear,
}

/// What a mosaic light goes through before its stars are detected.
#[derive(Debug)]
pub(crate) struct MosaicPreparation<'a> {
    pub(crate) masters: &'a CalibrationMasters,
    pub(crate) cosmic_ray: Option<&'a CosmicRayConfig>,
    /// What calibrating the lights could not check, counted for the run's report.
    pub(crate) notes: &'a CalibrationNotes,
}

impl MosaicPreparation<'_> {
    /// `cfa`, the light decoded from `path`, calibrated by the masters, cleaned of cosmic rays when
    /// asked, and demosaiced.
    pub(crate) fn prepare(
        &self,
        mut cfa: CfaImage,
        path: &Path,
        context: &LoadContext,
    ) -> Result<LinearImage, AlignStackError> {
        let outcome =
            self.masters
                .calibrate(&mut cfa)
                .map_err(|source| AlignStackError::Calibration {
                    path: path.to_path_buf(),
                    source: Box::new(source),
                })?;
        self.notes.record(outcome);
        if let Some(cosmic_ray) = self.cosmic_ray {
            let removed = cosmic_ray::reject(&mut cfa, cosmic_ray).map_err(|source| {
                AlignStackError::CosmicRay {
                    path: path.to_path_buf(),
                    source,
                }
            })?;
            tracing::info!(removed, "rejected cosmic rays");
        }
        // The demosaic polls the cancel token between its passes.
        cfa.demosaic(context.xtrans_passes, &context.cancel)
            .map_err(|Cancelled| AlignStackError::Cancelled)
    }
}

/// The detected lights, and the memory decisions the register-warp stage reads.
#[derive(Debug)]
pub(crate) struct DetectedLights {
    pub(crate) frames: Vec<DetectedFrame>,
    pub(crate) stage: StagePlan,
}

/// What a run of lights holds: how many there are, each one's demosaiced dimensions, its way from
/// its file or the caller to detected, and what the caller held of it before the run read the
/// machine.
#[derive(Debug)]
struct LightShape {
    count: usize,
    dimensions: ImageDimensions,
    prepared: Footprint,
    held: usize,
}

/// A light's samples and what its detection found, before the light is held where its tier says.
#[derive(Debug)]
struct DetectedSamples {
    image: LinearImage,
    stars: Vec<Star>,
    diagnostics: DetectionDiagnostics,
    stats: FrameStats,
}

impl DetectedSamples {
    /// The light kept where the caller put it.
    fn resident(self) -> DetectedFrame {
        let Self {
            image,
            stars,
            diagnostics,
            stats,
        } = self;
        DetectedFrame {
            image: PipelineFrame::Resident(image),
            stars,
            diagnostics,
            stats,
        }
    }

    /// The light parked where `parking` says, until its registration.
    fn parked(self, parking: &FrameParking) -> Result<DetectedFrame, AlignStackError> {
        let Self {
            image,
            stars,
            diagnostics,
            stats,
        } = self;
        Ok(DetectedFrame {
            image: parking.hold(image)?,
            stars,
            diagnostics,
            stats,
        })
    }
}

/// One light through the single pass: its parked frame and how it registered — `None` for the
/// reference, parked apart, and where the run was cancelled — and its detection funnel.
#[derive(Debug, Default)]
struct PassedLight {
    frame: Option<ParkedFrame>,
    diagnostics: DetectionDiagnostics,
}

impl<P: AsRef<Path> + Sync> LightSource<'_, P> {
    pub(crate) const fn len(&self) -> usize {
        match self {
            Self::Held(frames) => frames.len(),
            Self::Files(files) => files.paths.len(),
        }
    }

    /// Plan the run, then decode or take each light, check and measure it, detect its stars and
    /// park it where the tier says, as many at once as the plan allows.
    ///
    /// The statistics are measured before the warp correlates neighbouring pixels, and while the
    /// frame is in hand: a spilled frame would otherwise be read back for them. A demosaiced
    /// light's noise is the one its demosaic measured on the mosaic.
    pub(super) fn detect(self, run: &AlignStackRun<'_>) -> Result<DetectedLights, AlignStackError> {
        let AlignStackRun { config, ingest } = run;
        let total = self.len();
        debug_assert!(total > 0, "each entry refuses an empty set");
        let budget = ingest.budget;
        let shape = self.shape(ingest)?;
        let plan = RunPlan::of(
            total,
            &shape.found_reference(config, budget.workers()),
            budget,
        );
        let stage = shape.stage_plan(&plan, LightShape::WARP, ingest)?;
        tracing::info!(
            frames = total,
            budget_mb = budget.usable() / (1024 * 1024),
            concurrency = plan.width(LightShape::PREPARE),
            spilling = stage.parking.spills(),
            "Preparing lights and detecting stars"
        );

        let slots = plan.width(LightShape::PREPARE);
        let intake = FrameIntake::new(
            IntakePlan {
                dimensions: shape.dimensions,
                frame_count: total,
                decode_slots: slots,
                stage: StackingStage::Preparing,
            },
            ingest,
        );
        // Bounded by the plan, which charges each in-flight frame its decode and the detector's
        // whole working set. The RAW decode is the one step that cannot stop midway, so the bound
        // is also what a cancel has to drain.
        let mut detectors = DetectorPool::from_config(&config.detection, slots)
            .map_err(AlignStackError::DetectionConfig)?;
        let detect = |detector: &mut StarDetector, index: usize, image: LinearImage| {
            let stats = intake.admit(index, &image)?;
            let result = detector.detect(&image);
            log_detection(intake.complete_one(), total, &result);
            Ok::<_, AlignStackError>(DetectedSamples {
                image,
                stars: result.stars,
                diagnostics: result.diagnostics,
                stats,
            })
        };
        let frames = match self {
            // A held frame stays where the caller put it, whatever the tier.
            Self::Held(frames) => {
                intake.map_owned(frames, detectors.slots(), |detector, index, image| {
                    detect(detector, index, image).map(DetectedSamples::resident)
                })?
            }
            Self::Files(files) => intake.map(0..total, detectors.slots(), |detector, index| {
                let image = files.prepare(files.paths[index].as_ref(), intake.context())?;
                detect(detector, index, image)?.parked(&stage.parking)
            })?,
        };
        Ok(DetectedLights { frames, stage })
    }

    /// The run's shape, from the first held frame or the first file's header: no light is decoded
    /// before the tier is chosen.
    fn shape(&self, run: &IngestRun) -> Result<LightShape, AlignStackError> {
        match self {
            // They were in memory before the run read the machine: the plan charges them as held.
            Self::Held(frames) => Ok(LightShape::held(
                FramePeek::of_decoded(&frames[0]),
                frames.len(),
                run.budget.workers(),
            )),
            Self::Files(files) => files.shape(run),
        }
    }
}

impl LightShape {
    /// The stage of [`Self::found_reference`] that prepares the lights.
    const PREPARE: usize = 0;
    /// The stage of [`Self::found_reference`] that warps the lights.
    const WARP: usize = 1;
    /// The stage of [`Self::named_reference`] that takes each light through all of it.
    const PASS: usize = 0;

    /// [`Self::decoded`], but for lights the caller holds, which the plan charges only what the run
    /// adds to them.
    fn held(peek: FramePeek, count: usize, workers: usize) -> Self {
        let decoded = Self::decoded(peek, count, workers);
        Self {
            held: decoded.prepared.kept,
            ..decoded
        }
    }

    /// `count` linear lights decoded one at a time, as `peek` reads them on `workers`: a light's
    /// way is its decode and its statistics.
    fn decoded(peek: FramePeek, count: usize, workers: usize) -> Self {
        Self {
            count,
            dimensions: peek.dimensions,
            prepared: peek.footprint(workers),
            held: 0,
        }
    }

    /// The bytes a warped light keeps: its channels, its two quality planes, its flags and its
    /// gain grid.
    const fn warped(&self) -> usize {
        StoredLayout::warped(self.dimensions).heap_bytes(self.dimensions)
    }

    /// The most the lights write to a spill: each as it is warped, which is no smaller than as it
    /// was parked.
    const fn spill_bytes(&self) -> u64 {
        (self.warped() as u64).saturating_mul(self.count as u64)
    }

    /// Where `plan` parks the lights, and the width of its stage `warp`, which warps them; a spill
    /// checked for room for what the lights write to it.
    fn stage_plan(
        &self,
        plan: &RunPlan,
        warp: usize,
        run: &IngestRun,
    ) -> Result<StagePlan, AlignStackError> {
        let tier = Tier::for_plan(plan, self.spill_bytes(), &run.cache_dir)?;
        Ok(StagePlan {
            parking: FrameParking::new(tier),
            warp_width: plan.width(warp),
        })
    }

    /// What each worker's detector holds throughout: its pool of image-sized planes.
    const fn detector_bytes(&self) -> usize {
        DETECTION_WORKING_PLANES * self.dimensions.pixel_count() * size_of::<f32>()
    }

    /// The stages of the lights with the reference found, under `config` on `workers`: each light
    /// prepared and detected; then warped, the light it warps from in hand; the normalization; the
    /// combine.
    fn found_reference(&self, config: &AlignStackConfig, workers: usize) -> [Stage; 4] {
        let warped = self.warped() - self.held;
        let [normalize, combine] = self.combined(warped, config, workers);
        [
            Stage {
                kept: self.prepared.kept - self.held,
                held: 0,
                in_flight: self.prepared.transient() + self.detector_bytes(),
                units: Units::Frames,
            },
            Stage {
                kept: warped,
                held: 0,
                in_flight: self.prepared.kept,
                units: Units::Frames,
            },
            normalize,
            combine,
        ]
    }

    /// The stages of the lights with the reference named, under `config` on `workers`: each light
    /// taken through its preparation, its detection and its warp by one worker, which keeps only
    /// the warped light; the normalization; the combine.
    fn named_reference(&self, config: &AlignStackConfig, workers: usize) -> [Stage; 3] {
        debug_assert_eq!(self.held, 0, "a single pass decodes its own lights");
        let warped = self.warped();
        let [normalize, combine] = self.combined(warped, config, workers);
        [
            Stage {
                kept: warped,
                held: 0,
                in_flight: self.prepared.peak + self.detector_bytes(),
                units: Units::Frames,
            },
            normalize,
            combine,
        ]
    }

    /// The normalization and the combine of the warped lights, each kept at `kept`: every warped
    /// light covers only the pixels its warp reached.
    fn combined(&self, kept: usize, config: &AlignStackConfig, workers: usize) -> [Stage; 2] {
        let Self {
            count: frame_count,
            dimensions,
            ..
        } = *self;
        [
            Stage {
                kept,
                held: PlannedNormalization {
                    frame_count,
                    dimensions,
                    slots: Slots::new(None, dimensions.channels()),
                    normalization: config.stack.normalization,
                    coverage: SetCoverage::Common,
                    threads: workers,
                }
                .heap_bytes(),
                in_flight: 0,
                units: Units::Whole,
            },
            Stage {
                kept,
                held: config.stack.quality.heap_bytes(dimensions),
                in_flight: frame_count
                    * StoredLayout::warped(dimensions).pixel_bytes()
                    * dimensions.width(),
                units: Units::Rows(dimensions.height()),
            },
        ]
    }
}

impl<'a, P: AsRef<Path> + Sync> LightFiles<'a, P> {
    /// The lights of `paths`, each through `preparation`, read through the decode cache when
    /// `run` keeps one.
    ///
    /// # Errors
    ///
    /// The decode cache cannot be opened.
    pub(crate) fn new(
        paths: &'a [P],
        preparation: Preparation<'a>,
        run: &IngestRun,
    ) -> Result<Self, AlignStackError> {
        let kept = run
            .keep_cache
            .then(|| DecodeCache::open(&run.cache_dir))
            .transpose()?;
        Ok(Self {
            paths,
            preparation,
            kept,
        })
    }

    /// Stack the lights: with [`Reference::Index`] in one pass, each light prepared, detected,
    /// registered, warped and stored by one worker; with [`Reference::Auto`] every light prepared
    /// and detected first, since the reference is the sharpest, and parked until it registers.
    pub(super) fn stack(
        self,
        run: &AlignStackRun<'_>,
    ) -> Result<AlignStackResult, AlignStackError> {
        match run.config.reference {
            Reference::Index(reference) => self.stack_in_one_pass(reference, run),
            Reference::Auto => {
                let detected = LightSource::Files(self).detect(run)?;
                run.register_warp_and_stack(detected)
            }
        }
    }

    /// The run's shape, from the first file's header — or, for a linear format with none to read,
    /// its decode — once the decode cache, when the run keeps one, is known to have room for the
    /// lights it does not hold yet.
    fn shape(&self, run: &IngestRun) -> Result<LightShape, AlignStackError> {
        let first = self.paths[0].as_ref();
        let workers = run.budget.workers();
        let cosmic_ray = match self.preparation {
            Preparation::Calibrate(MosaicPreparation { cosmic_ray, .. }) => cosmic_ray,
            Preparation::Linear => {
                let peek = match LinearImage::peek(first, &run.context) {
                    Some(peek) => peek,
                    None => {
                        FramePeek::of_decoded(&self.decode::<LinearImage>(first, &run.context)?)
                    }
                };
                self.ensure_kept_space::<LinearImage>(peek.resident_bytes(), &run.context)?;
                return Ok(LightShape::decoded(peek, self.paths.len(), workers));
            }
        };
        let info = CfaFrameInfo::from_file(first, &run.context).map_err(|source| {
            AlignStackError::Load {
                path: first.to_path_buf(),
                source: Box::new(source),
            }
        })?;
        let size = info.dimensions.size();
        let mosaic = StoredLayout::decoded(info.dimensions).heap_bytes(info.dimensions);
        self.ensure_kept_space::<CfaImage>(mosaic, &run.context)?;
        // Refused here, before any light is decoded, when the pass cannot run on this sensor.
        let cosmic_ray = match cosmic_ray {
            Some(_) => cosmic_ray::heap_bytes(&info.cfa_type, size).map_err(|source| {
                AlignStackError::CosmicRay {
                    path: first.to_path_buf(),
                    source,
                }
            })?,
            None => 0,
        };
        let noise = if info.cfa_type.is_mosaic() {
            MosaicNoise::heap_bytes(size, &info.cfa_type)
        } else {
            0
        };
        // The flags travel from the mosaic to the frame its demosaic makes.
        let flags = info.dimensions.flag_plane_bytes();
        let demosaic = info.cfa_type.demosaic_footprint(info.dimensions, workers);
        let dimensions = ImageDimensions::new(size, info.cfa_type.num_colors());
        // The decoder's bytes beside the mosaic it makes; the cosmic-ray pass over the mosaic; the
        // noise the demosaic measures on it; the demosaic; the statistics, or the detection's
        // noise estimate, beside the demosaiced frame.
        let prepared = Footprint {
            kept: mosaic,
            peak: mosaic + info.decoder_bytes,
        }
        .beside(cosmic_ray)
        .beside(noise)
        .then(Footprint {
            kept: demosaic.kept + flags,
            peak: demosaic.peak + flags,
        })
        .beside(FrameStats::heap_bytes(dimensions, None, workers));
        Ok(LightShape {
            count: self.paths.len(),
            dimensions,
            prepared,
            held: 0,
        })
    }

    /// Stack the lights against the light at index `reference` in one pass: the reference is
    /// prepared and detected first and alone, then each worker takes a light through its
    /// preparation, its detection, its registration, its warp and its store, so a light is
    /// written once and no prepared set is ever parked.
    fn stack_in_one_pass(
        &self,
        reference: usize,
        stack: &AlignStackRun<'_>,
    ) -> Result<AlignStackResult, AlignStackError> {
        let AlignStackRun {
            config,
            ingest: run,
        } = stack;
        let total = self.paths.len();
        if reference >= total {
            return Err(AlignStackError::ReferenceOutOfRange {
                index: reference,
                count: total,
            });
        }
        let budget = run.budget;
        let shape = self.shape(run)?;
        let plan = RunPlan::of(
            total,
            &shape.named_reference(config, budget.workers()),
            budget,
        );
        let StagePlan {
            parking,
            warp_width: workers,
        } = shape.stage_plan(&plan, LightShape::PASS, run)?;
        tracing::info!(
            frames = total,
            reference,
            budget_mb = budget.usable() / (1024 * 1024),
            concurrency = workers,
            spilling = parking.spills(),
            "Preparing, detecting and registering lights in one pass"
        );
        let cancel = &run.context.cancel;
        let slots = workers.min(total);
        let intake = FrameIntake::new(
            IntakePlan {
                dimensions: shape.dimensions,
                frame_count: total,
                decode_slots: slots,
                stage: StackingStage::Preparing,
            },
            run,
        );
        let mut detectors = DetectorPool::from_config(&config.detection, slots)
            .map_err(AlignStackError::DetectionConfig)?;

        if cancel.is_cancelled() {
            return Err(AlignStackError::Cancelled);
        }
        // The reference decodes alone, on every thread a decode may use.
        let image = self.prepare(self.paths[reference].as_ref(), &run.context)?;
        let stats = intake.admit(reference, &image)?;
        let result = detectors.first().detect(&image);
        log_detection(intake.complete_one(), total, &result);
        let required = config
            .registration
            .matching
            .required_stars(config.registration.transform_type);
        if result.stars.len() < required {
            return Err(AlignStackError::ReferenceInsufficientStars {
                index: reference,
                found: result.stars.len(),
                required,
            });
        }
        let metadata = image.metadata.clone();
        let dimensions = image.dimensions();
        let reference_diagnostics = result.diagnostics;
        let registrar = FrameRegistrar::new(
            stack,
            &parking,
            LightSet {
                count: total,
                dimensions,
                reference,
                reference_stars: result.stars,
            },
        );
        let reference_frame = registrar.park(
            reference,
            DetectedFrame {
                image: PipelineFrame::Resident(image),
                stars: Vec::new(),
                diagnostics: DetectionDiagnostics::default(),
                stats,
            },
        )?;

        let others = intake.map(0..total, detectors.slots(), |detector, index| {
            if index == reference {
                return Ok::<_, AlignStackError>(PassedLight::default());
            }
            let image = self.prepare(self.paths[index].as_ref(), intake.context())?;
            let stats = intake.admit(index, &image)?;
            let result = detector.detect(&image);
            log_detection(intake.complete_one(), total, &result);
            let frame = registrar.park(
                index,
                DetectedFrame {
                    image: PipelineFrame::Resident(image),
                    stars: result.stars,
                    diagnostics: DetectionDiagnostics::default(),
                    stats,
                },
            )?;
            Ok(PassedLight {
                frame,
                diagnostics: result.diagnostics,
            })
        })?;
        // The registrar's warp sets are a warped frame's worth each, and a detector's pool the
        // planes detection holds: the combine sizes its chunks against the memory without them.
        drop(registrar);
        drop(detectors);

        let mut outcomes = Vec::with_capacity(total);
        let mut detection = Vec::with_capacity(total);
        for light in others {
            outcomes.push(light.frame);
            detection.push(light.diagnostics);
        }
        outcomes[reference] = reference_frame;
        detection[reference] = reference_diagnostics;
        RegisteredSet {
            outcomes,
            reference,
            metadata,
            dimensions,
            detection,
            parking,
        }
        .combine(stack)
    }

    /// Load one light and make it ready: a mosaic calibrated by the masters, cleaned of cosmic
    /// rays when asked, and demosaiced; a linear image as it decodes.
    fn prepare(&self, path: &Path, context: &LoadContext) -> Result<LinearImage, AlignStackError> {
        let Preparation::Calibrate(mosaic) = &self.preparation else {
            return self.decode(path, context);
        };
        mosaic.prepare(self.decode(path, context)?, path, context)
    }

    /// `path` decoded as an `I` under `context`: through the decode cache when the run keeps one,
    /// a RAW light as its uncalibrated mosaic, so a later run with other masters reuses it too.
    fn decode<I: StackableImage>(
        &self,
        path: &Path,
        context: &LoadContext,
    ) -> Result<I, AlignStackError> {
        let decoded = match &self.kept {
            Some(kept) => kept.decode::<I>(path, context),
            None => I::load(path, context).map_err(KeptDecodeError::from),
        };
        decoded.map_err(|error| match error {
            KeptDecodeError::Decode(source) => load_error(path, source),
            KeptDecodeError::Store(source) => AlignStackError::FrameStore(source),
        })
    }

    /// Check the decode cache's disk holds the `kept_bytes` each light it does not hold yet will
    /// write, decoded as an `I`.
    fn ensure_kept_space<I: StackableImage>(
        &self,
        kept_bytes: usize,
        context: &LoadContext,
    ) -> Result<(), AlignStackError> {
        if let Some(kept) = &self.kept {
            let missing = kept.missing::<I>(self.paths, context) as u64;
            kept.ensure_space(missing.saturating_mul(kept_bytes as u64))?;
        }
        Ok(())
    }
}

/// The detection funnel — candidates → deblended → centroided → kept — shows how confidently
/// the frame resolved into usable stars.
fn log_detection(frame: usize, total: usize, result: &DetectionResult) {
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

/// A light that could not be decoded: the run's cancel when that stopped it, else the file named.
fn load_error(path: &Path, source: ImageError) -> AlignStackError {
    match source {
        ImageError::Cancelled { .. } => AlignStackError::Cancelled,
        source => AlignStackError::Load {
            path: path.to_path_buf(),
            source: Box::new(source),
        },
    }
}

#[cfg(test)]
mod tests {
    use crate::combine::stack_config::{Normalization, StackConfig};
    use crate::frame::frame_peek::FramePeek;
    use crate::image::image_dimensions::ImageDimensions;
    use crate::memory::stage::{Stage, Units};
    use crate::pipeline::align_stack_config::AlignStackConfig;
    use crate::pipeline::light_source::LightShape;
    use crate::stack_product::quality_planes::QualityPlanes;

    /// Three mono lights of 4 × 4 with no normalization and the image alone, on one worker.
    ///
    /// A light decodes to 16 pixels of 4 bytes and 16 flag bytes, 80 B, and its statistics are
    /// MRS over one tile that is the frame: 16 coefficients of 33 bytes, 28 bytes of transform a
    /// pixel, a row of 16 B and 136 of bookkeeping, 1128 B. Its detector holds 9 planes, 576 B.
    /// Warped it holds 3 planes, 192 B, its flags and a 2 × 2 gain grid, 16 B each: 224 B, and a
    /// row reads 12 + 1 B a pixel and 4 B of gain over 16 pixels, rounded up to 1: 14 B a pixel,
    /// 168 B for three over 4 pixels. The combine holds the image and its flag byte, 80 B.
    ///
    /// Found, a light in work holds its statistics and its detector, 1704 B, and a warp its 80 B
    /// source. Named, a worker holds the light at its peak, 1208 B, and the detector. Held by the
    /// caller, a light keeps nothing until it is warped, then 224 − 80 = 144 B.
    #[test]
    fn a_light_is_kept_as_each_stage_leaves_it() {
        let peek = FramePeek {
            dimensions: ImageDimensions::new((4, 4), 1),
            decoder_bytes: 0,
            mosaic: None,
        };
        let config = AlignStackConfig {
            stack: StackConfig {
                normalization: Normalization::None,
                quality: QualityPlanes::IMAGE_ONLY,
                ..StackConfig::light()
            },
            ..AlignStackConfig::default()
        };
        let stage = |kept, held, in_flight, units| Stage {
            kept,
            held,
            in_flight,
            units,
        };
        let tail = |kept| {
            [
                stage(kept, 0, 0, Units::Whole),
                stage(kept, 80, 168, Units::Rows(4)),
            ]
        };
        let decoded = LightShape::decoded(peek, 3, 1);
        let [normalize, combine] = tail(224);
        assert_eq!(
            decoded.found_reference(&config, 1),
            [
                stage(80, 0, 1704, Units::Frames),
                stage(224, 0, 80, Units::Frames),
                normalize,
                combine,
            ]
        );
        assert_eq!(
            decoded.named_reference(&config, 1),
            [stage(224, 0, 1784, Units::Frames), normalize, combine]
        );
        let [normalize, combine] = tail(144);
        assert_eq!(
            LightShape::held(peek, 3, 1).found_reference(&config, 1),
            [
                stage(0, 0, 1704, Units::Frames),
                stage(144, 0, 80, Units::Frames),
                normalize,
                combine,
            ]
        );
    }
}
