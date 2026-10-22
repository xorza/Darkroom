//! [`AlignStackRun`]: one registered stack under way, and its two schedules — every light detected
//! before any registers, or each light through all of it in one pass.

use std::mem;
use std::path::Path;

use common::CancelToken;

use crate::concurrency;
use crate::frame_store::run_storage::RunStorage;
use crate::image::image_dimensions::ImageDimensions;
use crate::image::linear_image::LinearImage;
use crate::ingest::frame_intake::{FrameIntake, IntakePlan};
use crate::ingest::ingest_run::IngestRun;
use crate::memory::run_plan::RunPlan;
use crate::pipeline::align_stack_config::{AlignStackConfig, Reference, ReferenceCandidate};
use crate::pipeline::align_stack_result::AlignStackResult;
use crate::pipeline::detected_frame::{DetectedFrame, DetectedLight};
use crate::pipeline::error::{AlignStackConfigError, AlignStackError};
use crate::pipeline::frame_parking::StagePlan;
use crate::pipeline::frame_registrar::{FrameRegistrar, LightSet, ParkedLight};
use crate::pipeline::light_files::LightFiles;
use crate::pipeline::light_shape::LightShape;
use crate::pipeline::light_source::LightSource;
use crate::pipeline::registered_set::RegisteredSet;
use crate::progress::stacking_progress::StackingStage;
use crate::progress::stage_counter::StageCounter;
use crate::registration::registrar::Registrar;
use crate::star_detection::star_detector::StarDetector;
use crate::star_detection::star_detector::detection_result::DetectionResult;

/// One registered stack under way: what it was asked to do, and the memory reading, decode policy,
/// cancel token and progress it took at its entry.
#[derive(Debug)]
pub(super) struct AlignStackRun<'a> {
    pub(super) config: &'a AlignStackConfig,
    /// The registration configuration, checked where the run was built.
    pub(super) registrar: Registrar<'a>,
    pub(super) ingest: IngestRun,
}

/// The detected lights, and the memory decisions the register-warp stage reads.
#[derive(Debug)]
struct DetectedLights {
    lights: Vec<DetectedLight>,
    stage: StagePlan,
}

/// How each light is taken in: admitted to the set, then its stars detected.
#[derive(Debug)]
struct LightIntake<'a> {
    intake: FrameIntake<'a>,
    count: usize,
}

impl LightIntake<'_> {
    /// Light `idx`, `image`, admitted to the set, its stars detected by `detector`.
    fn detect(
        &self,
        detector: &mut StarDetector,
        idx: usize,
        image: LinearImage,
    ) -> Result<DetectedLight<LinearImage>, AlignStackError> {
        let admitted = self.intake.admit(idx, &image)?;
        let result = detector.detect(&image);
        self.log(idx, &result);
        let DetectionResult { stars, diagnostics } = result;
        Ok(DetectedLight {
            frame: DetectedFrame {
                image,
                stars,
                admitted,
            },
            diagnostics,
        })
    }

    /// The detection funnel — candidates → deblended → centroided → kept — shows how confidently
    /// light `idx` resolved into usable stars.
    fn log(&self, idx: usize, result: &DetectionResult) {
        let diagnostics = &result.diagnostics;
        tracing::info!(
            light = idx,
            total = self.count,
            candidates = diagnostics.candidates_after_filtering,
            deblended = diagnostics.deblended_components,
            measured = diagnostics.stars_after_centroid,
            stars = result.stars.len(),
            "detected stars"
        );
    }
}

impl AlignStackRun<'_> {
    pub(super) const fn cancel(&self) -> &CancelToken {
        &self.ingest.ctx.cancel
    }

    /// Stack `lights`. Lights from files with [`Reference::Idx`] go in one pass, each prepared,
    /// detected, registered, warped and stored by one worker; otherwise every light is prepared
    /// and detected first — the reference is the sharpest, or the lights are already in memory —
    /// and parked until it registers.
    pub(super) fn stack<P: AsRef<Path> + Sync>(
        &self,
        lights: LightSource<'_, P>,
    ) -> Result<AlignStackResult, AlignStackError> {
        match (self.config.reference, lights) {
            (Reference::Idx(reference), LightSource::Files(files)) => {
                self.stack_in_one_pass(&files, reference)
            }
            (_, lights) => {
                let detected = self.detect(lights)?;
                self.register_warp_and_stack(detected)
            }
        }
    }

    /// Plan the run, then decode or take each light, check and measure it, detect its stars and
    /// park it where the tier says, as many at once as the plan allows.
    ///
    /// The statistics are measured before the warp correlates neighbouring pixels, and while the
    /// frame is in hand: a spilled frame would otherwise be read back for them. A demosaiced
    /// light's noise is the one its demosaic measured on the mosaic.
    fn detect<P: AsRef<Path> + Sync>(
        &self,
        lights: LightSource<'_, P>,
    ) -> Result<DetectedLights, AlignStackError> {
        let Self { config, ingest, .. } = self;
        let count = lights.len();
        debug_assert!(count > 0, "each entry refuses an empty set");
        let budget = ingest.budget;
        let shape = lights.shape(ingest)?;
        let plan = RunPlan::of(
            count,
            &shape.found_reference(config, budget.workers()),
            budget,
        );
        let storage = match &lights {
            LightSource::Held(_) => &RunStorage::holding(ingest),
            LightSource::Files(files) => files.storage(),
        };
        let stage = shape.stage_plan(&plan, LightShape::WARP, storage)?;
        let slots = plan.width(LightShape::PREPARE);
        tracing::info!(
            lights = count,
            budget_mb = budget.usable() / (1024 * 1024),
            concurrency = slots,
            spilling = stage.parking.spills(),
            "Preparing lights and detecting stars"
        );

        // Bounded by the plan, which charges each in-flight light its decode and the detector's
        // whole working set. The RAW decode is the one step that cannot stop midway, so the bound
        // is also what a cancel has to drain.
        let intake = self.intake(shape.dimensions, count, slots);
        let mut detectors = self.detectors(slots)?;
        let lights = match lights {
            // A held light stays where the caller put it, whatever the tier.
            LightSource::Held(frames) => {
                intake
                    .intake
                    .map_owned(frames, &mut detectors, |detector, idx, image| {
                        let DetectedLight { frame, diagnostics } =
                            intake.detect(detector, idx, image)?;
                        intake.intake.complete_one();
                        Ok::<_, AlignStackError>(DetectedLight {
                            frame: frame.resident(),
                            diagnostics,
                        })
                    })?
            }
            LightSource::Files(files) => {
                intake
                    .intake
                    .map(0..count, &mut detectors, |detector, idx| {
                        let image = files.prepare(idx, intake.intake.ctx())?;
                        let DetectedLight { frame, diagnostics } =
                            intake.detect(detector, idx, image)?;
                        let frame = frame.parked(&stage.parking)?;
                        intake.intake.complete_one();
                        Ok::<_, AlignStackError>(DetectedLight { frame, diagnostics })
                    })?
            }
        };
        Ok(DetectedLights { lights, stage })
    }

    /// Register every light to the chosen reference, warp it, and combine the survivors.
    ///
    /// Every route checked each light's dimensions against the first before detection: `warp`
    /// reprojects into the *source* frame's grid, so a mismatch here would reach the combine as
    /// differently-sized planes.
    fn register_warp_and_stack(
        &self,
        detected: DetectedLights,
    ) -> Result<AlignStackResult, AlignStackError> {
        let DetectedLights {
            lights,
            stage: StagePlan {
                parking,
                warp_width,
            },
        } = detected;
        let count = lights.len();
        if self.cancel().is_cancelled() {
            return Err(AlignStackError::Cancelled);
        }
        debug_assert!(
            lights
                .iter()
                .all(|light| light.frame.image.dimensions() == lights[0].frame.image.dimensions()),
            "every entry checks the dimensions before detection"
        );
        let (mut frames, detection): (Vec<_>, Vec<_>) = lights
            .into_iter()
            .map(|light| (light.frame, light.diagnostics))
            .unzip();

        let candidates: Vec<ReferenceCandidate> = frames
            .iter()
            .zip(&detection)
            .map(|(frame, funnel)| ReferenceCandidate {
                stars: frame.stars.len(),
                median_fwhm: funnel.median_fwhm,
            })
            .collect();
        let reference = self
            .config
            .reference
            .select(&candidates, self.required_stars())?;
        let metadata = frames[reference].image.metadata().clone();
        let dimensions = frames[reference].image.dimensions();
        let registrar = FrameRegistrar::new(
            self,
            &parking,
            LightSet {
                count,
                dimensions,
                reference,
                reference_stars: mem::take(&mut frames[reference].stars),
            },
        );
        // Counted once a light is parked; the reference is not registered, so not among them.
        let registering =
            StageCounter::new(&self.ingest.progress, StackingStage::Registering, count - 1);
        // Taking each detected light by value frees its input image as soon as the warped output
        // exists, so this stage never holds the complete input and warped sets simultaneously.
        let parked = concurrency::try_par_map_bounded_owned(
            frames,
            &mut vec![(); warp_width],
            |(), idx, frame| {
                let parked = registrar.park(idx, frame)?;
                if idx != reference {
                    registering.complete_one();
                }
                Ok::<_, AlignStackError>(parked)
            },
        )?;
        // The registrar's warp sets are a warped frame's worth each, and the combine sizes its
        // chunks against the memory without them.
        drop(registrar);

        RegisteredSet {
            lights: parked
                .into_iter()
                .zip(detection)
                .map(|(parked, detection)| ParkedLight { parked, detection })
                .collect(),
            reference,
            metadata,
            dimensions,
            parking,
        }
        .combine(self)
    }

    /// Stack `files` against the light at index `reference` in one pass: the reference is
    /// prepared and detected first and alone, then each worker takes a light through its
    /// preparation, its detection, its registration, its warp and its store, so a light is
    /// written once and no prepared set is ever parked. The pass reports as one stage, a light
    /// counted once it is parked.
    fn stack_in_one_pass<P: AsRef<Path> + Sync>(
        &self,
        files: &LightFiles<'_, P>,
        reference: usize,
    ) -> Result<AlignStackResult, AlignStackError> {
        let Self { config, ingest, .. } = self;
        let count = files.len();
        let budget = ingest.budget;
        let shape = files.shape(ingest)?;
        let plan = RunPlan::of(
            count,
            &shape.named_reference(config, budget.workers()),
            budget,
        );
        let StagePlan {
            parking,
            warp_width: workers,
        } = shape.stage_plan(&plan, LightShape::PASS, files.storage())?;
        tracing::info!(
            lights = count,
            reference,
            budget_mb = budget.usable() / (1024 * 1024),
            concurrency = workers,
            spilling = parking.spills(),
            "Preparing, detecting and registering lights in one pass"
        );
        let slots = workers.min(count);
        let intake = self.intake(shape.dimensions, count, slots);
        let mut detectors = self.detectors(slots)?;

        if self.cancel().is_cancelled() {
            return Err(AlignStackError::Cancelled);
        }
        // The reference decodes alone, on every thread a decode may use.
        let image = files.prepare(reference, &ingest.ctx)?;
        let DetectedLight {
            frame: mut first,
            diagnostics,
        } = intake.detect(&mut detectors[0], reference, image)?;
        ReferenceCandidate {
            stars: first.stars.len(),
            median_fwhm: diagnostics.median_fwhm,
        }
        .check(reference, self.required_stars())?;
        let metadata = first.image.metadata.clone();
        let dimensions = first.image.dimensions();
        let registrar = FrameRegistrar::new(
            self,
            &parking,
            LightSet {
                count,
                dimensions,
                reference,
                reference_stars: mem::take(&mut first.stars),
            },
        );
        let reference_light = ParkedLight {
            parked: registrar.park(reference, first.resident())?,
            detection: diagnostics,
        };
        intake.intake.complete_one();

        let others: Vec<usize> = (0..count).filter(|&idx| idx != reference).collect();
        let mut lights = intake
            .intake
            .map_owned(others, &mut detectors, |detector, _, idx| {
                let image = files.prepare(idx, intake.intake.ctx())?;
                let DetectedLight { frame, diagnostics } = intake.detect(detector, idx, image)?;
                let parked = registrar.park(idx, frame.resident())?;
                intake.intake.complete_one();
                Ok::<_, AlignStackError>(ParkedLight {
                    parked,
                    detection: diagnostics,
                })
            })?;
        lights.insert(reference, reference_light);
        // The registrar's warp sets are a warped frame's worth each, and a detector's pool the
        // planes detection holds: the combine sizes its chunks against the memory without them.
        drop(registrar);
        drop(detectors);

        RegisteredSet {
            lights,
            reference,
            metadata,
            dimensions,
            parking,
        }
        .combine(self)
    }

    /// The intake of `count` lights of `dimensions`, `slots` of them decoded at once.
    fn intake(&self, dimensions: ImageDimensions, count: usize, slots: usize) -> LightIntake<'_> {
        LightIntake {
            intake: FrameIntake::new(
                IntakePlan {
                    dimensions,
                    frame_count: count,
                    decode_slots: slots,
                    stage: StackingStage::Preparing,
                },
                &self.ingest,
            ),
            count,
        }
    }

    /// One detector for each of `slots` lights in flight, each carrying its warmed buffers from
    /// one light to the next it picks up. Built up front, so an invalid configuration fails here
    /// rather than from inside the parallel closure.
    fn detectors(&self, slots: usize) -> Result<Vec<StarDetector>, AlignStackError> {
        debug_assert!(slots > 0, "a run detects one light at a time at least");
        (0..slots)
            .map(|_| StarDetector::from_config(self.config.detection.clone()))
            .collect::<Result<_, _>>()
            .map_err(|invalid| AlignStackConfigError::Detection(invalid).into())
    }

    /// The stars a reference needs for the others to register to it.
    fn required_stars(&self) -> usize {
        let registration = &self.config.registration;
        registration.matching.required_stars(registration.model)
    }
}
