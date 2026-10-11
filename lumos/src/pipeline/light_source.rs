//! [`LightSource`]: where a registered stack's lights come from, and the stage that detects them.

use std::path::Path;

use common::CancelToken;

use crate::calibration_masters::CalibrationMasters;
use crate::calibration_masters::cosmic_ray;
use crate::calibration_masters::cosmic_ray::config::CosmicRayConfig;
use crate::calibration_masters::cosmic_ray::reject_cosmic_rays;
use crate::combine::cache::slots::Slots;
use crate::combine::normalization::{FrameNorm, SetCoverage};
use crate::frame_store::decode_cache::DecodeCache;
use crate::frame_store::error::KeptDecodeError;
use crate::frame_store::frame_stats::FrameStats;
use crate::frame_store::stackable_image::StackableImage;
use crate::ingest::frame_intake::{FrameIntake, IntakePlan};
use crate::ingest::ingest_run::IngestRun;
use crate::io::cancelled::Cancelled;
use crate::io::image::cfa::{CfaFrameInfo, CfaImage};
use crate::io::image::error::ImageError;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::linear::LinearImage;
use crate::io::image::load_context::LoadContext;
use crate::io::image::mosaic_noise::MosaicNoise;
use crate::io::raw::demosaic::DemosaicMemory;
use crate::memory::DETECTION_WORKING_PLANES;
use crate::memory::memory_plan::{MemoryPlan, PerFrameBytes, RunShape};
use crate::pipeline::align::register_warp_and_stack;
use crate::pipeline::calibrate::CalibrationNotes;
use crate::pipeline::config::{AlignStackConfig, Reference};
use crate::pipeline::detected_frame::DetectedFrame;
use crate::pipeline::detector_pool::DetectorPool;
use crate::pipeline::error::AlignStackError;
use crate::pipeline::frame_parking::{FrameParking, StagePlan};
use crate::pipeline::frame_registrar::{FrameRegistrar, ParkedFrame};
use crate::pipeline::pipeline_frame::PipelineFrame;
use crate::pipeline::registered_set::RegisteredSet;
use crate::pipeline::result::AlignStackResult;
use crate::progress::progress_callback::ProgressCallback;
use crate::progress::stacking_progress::StackingStage;
use crate::registration::resample::WarpBuffers;
use crate::star_detection::detector::{DetectionResult, Diagnostics, StarDetector};
use crate::star_detection::star::Star;

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
    Calibrate {
        masters: &'a CalibrationMasters,
        cosmic_ray: Option<&'a CosmicRayConfig>,
        /// What calibrating them could not check, counted for the run's report.
        notes: &'a CalibrationNotes,
    },
    /// An image already calibrated, taken as it decodes.
    Linear,
}

/// The detected lights, and the memory decisions the register-warp stage reads.
#[derive(Debug)]
pub(crate) struct DetectedLights {
    pub(crate) frames: Vec<DetectedFrame>,
    pub(crate) stage: StagePlan,
}

/// What a run of lights costs: each one's demosaiced dimensions, and the shape the plan reads.
#[derive(Debug)]
struct LightShape {
    dimensions: ImageDimensions,
    run: RunShape,
}

/// A light's samples and what its detection found, before the light is held where its tier says.
#[derive(Debug)]
struct DetectedSamples {
    image: LinearImage,
    stars: Vec<Star>,
    diagnostics: Diagnostics,
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
    diagnostics: Diagnostics,
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
    pub(crate) fn detect(
        self,
        config: &AlignStackConfig,
        run: &IngestRun,
        progress: &ProgressCallback,
    ) -> Result<DetectedLights, AlignStackError> {
        let total = self.len();
        debug_assert!(total > 0, "each entry refuses an empty set");
        let threads = rayon::current_num_threads();
        let shape = self.shape(config, run, threads)?;
        let plan = MemoryPlan::plan(shape.run, threads, run.memory.planning());
        let stage = StagePlan::new(&plan, shape.run.spill_bytes(), run)?;
        tracing::info!(
            frames = total,
            planning_mb = run.memory.planning() / (1024 * 1024),
            concurrency = plan.decode_concurrency,
            spilling = stage.parking.spills(),
            "Preparing lights and detecting stars"
        );

        let slots = plan.decode_concurrency.min(total);
        let intake = FrameIntake::new(
            IntakePlan {
                dimensions: shape.dimensions,
                frame_count: total,
                decode_slots: slots,
                stage: StackingStage::Preparing,
            },
            run,
            progress,
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

    /// The run's shape with `threads` workers, from the first held frame or the first file's
    /// header: no light is decoded before the tier is chosen.
    fn shape(
        &self,
        config: &AlignStackConfig,
        run: &IngestRun,
        threads: usize,
    ) -> Result<LightShape, AlignStackError> {
        match self {
            // They were in memory before the run read the machine: the plan charges them as held.
            Self::Held(frames) => Ok(LightShape::linear(
                frames[0].dimensions(),
                frames.len(),
                true,
                config,
                threads,
            )),
            Self::Files(files) => files.shape(config, run, threads),
        }
    }
}

impl LightShape {
    /// What normalizing `frame_count` registered lights of `dimensions` holds: each warped frame
    /// covers only the pixels its warp reached.
    fn normalization_bytes(
        config: &AlignStackConfig,
        dimensions: ImageDimensions,
        frame_count: usize,
        threads: usize,
    ) -> usize {
        FrameNorm::working_bytes(
            frame_count,
            dimensions,
            Slots::new(None, dimensions.channels()),
            config.stack.normalization,
            SetCoverage::Common,
            threads,
        )
    }

    /// The shape of `frame_count` linear lights of `dimensions`, `held` by the caller or decoded
    /// one at a time, with `threads` workers: what one's pass adds is what its statistics and its
    /// detection's noise estimate hold.
    fn linear(
        dimensions: ImageDimensions,
        frame_count: usize,
        held: bool,
        config: &AlignStackConfig,
        threads: usize,
    ) -> Self {
        let frame_bytes = dimensions.frame_bytes();
        let plane_bytes = dimensions.pixel_count() * size_of::<f32>();
        Self {
            dimensions,
            run: RunShape {
                frame_count,
                decode: DemosaicMemory {
                    output_bytes: frame_bytes,
                    peak_bytes: frame_bytes + FrameStats::scratch_bytes(dimensions, None, threads),
                },
                held_bytes: if held { frame_bytes } else { 0 },
                detection_bytes: DETECTION_WORKING_PLANES * plane_bytes,
                warp: Some(PerFrameBytes::new(
                    plane_bytes,
                    frame_bytes,
                    dimensions.flat_gain_bytes(),
                )),
                output_bytes: config.stack.quality.resident_bytes(dimensions),
                normalization_bytes: Self::normalization_bytes(
                    config,
                    dimensions,
                    frame_count,
                    threads,
                ),
            },
        }
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
    pub(crate) fn stack(
        self,
        config: &AlignStackConfig,
        run: &IngestRun,
        progress: ProgressCallback,
        cancel: CancelToken,
    ) -> Result<AlignStackResult, AlignStackError> {
        match config.reference {
            Reference::Index(reference) => self.stack_in_one_pass(reference, config, run, progress),
            Reference::Auto => {
                let detected = LightSource::Files(self).detect(config, run, &progress)?;
                register_warp_and_stack(detected.frames, config, detected.stage, progress, cancel)
            }
        }
    }

    /// The run's shape, from the first file's header — or, for a linear format with none to read,
    /// its decode — once the decode cache, when the run keeps one, is known to have room for the
    /// lights it does not hold yet.
    fn shape(
        &self,
        config: &AlignStackConfig,
        run: &IngestRun,
        threads: usize,
    ) -> Result<LightShape, AlignStackError> {
        let first = self.paths[0].as_ref();
        let cosmic_ray = match self.preparation {
            Preparation::Calibrate { cosmic_ray, .. } => cosmic_ray,
            Preparation::Linear => {
                let dimensions = match LinearImage::peek(first, &run.context) {
                    Some(peek) => peek.dimensions,
                    None => self
                        .decode::<LinearImage>(first, &run.context)?
                        .dimensions(),
                };
                self.ensure_kept_space::<LinearImage>(
                    dimensions.frame_bytes() + dimensions.pixel_count(),
                    &run.context,
                )?;
                return Ok(LightShape::linear(
                    dimensions,
                    self.paths.len(),
                    false,
                    config,
                    threads,
                ));
            }
        };
        let info = CfaFrameInfo::from_file(first, &run.context).map_err(|source| {
            AlignStackError::Load {
                path: first.to_path_buf(),
                source: Box::new(source),
            }
        })?;
        let plane_bytes = info.dimensions.pixel_count() * size_of::<f32>();
        // A kept mosaic is its plane and a flag byte a photosite.
        self.ensure_kept_space::<CfaImage>(
            plane_bytes + info.dimensions.pixel_count(),
            &run.context,
        )?;
        let demosaic = info.cfa_type.demosaic_memory(info.dimensions);
        let dimensions = ImageDimensions::new(info.dimensions.size(), info.cfa_type.num_colors());
        // One frame's pass peaks at the largest of: the decoder's own bytes beside the mosaic it
        // makes, the demosaic, the cosmic-ray pass over the mosaic, the noise the demosaic measures
        // on the mosaic and its flags, and the statistics or the detection's noise estimate beside
        // the demosaiced frame, which run one after the other.
        // Refused here, before any light is decoded, when the pass cannot run on this sensor.
        let cosmic_ray = match cosmic_ray {
            Some(_) => {
                plane_bytes
                    + cosmic_ray::heap_bytes(&info.cfa_type, info.dimensions.size()).map_err(
                        |source| AlignStackError::CosmicRay {
                            path: first.to_path_buf(),
                            source,
                        },
                    )?
            }
            None => 0,
        };
        Ok(LightShape {
            dimensions,
            run: RunShape {
                frame_count: self.paths.len(),
                decode: demosaic
                    .with_peak_at_least(plane_bytes.saturating_add(info.decoder_bytes))
                    .with_peak_at_least(cosmic_ray)
                    .with_peak_at_least(if info.cfa_type.is_mosaic() {
                        plane_bytes
                            + info.dimensions.flag_plane_bytes()
                            + MosaicNoise::peak_bytes(info.dimensions.size(), &info.cfa_type)
                    } else {
                        0
                    })
                    .with_peak_at_least(
                        demosaic.output_bytes
                            + FrameStats::scratch_bytes(dimensions, None, threads),
                    ),
                held_bytes: 0,
                detection_bytes: DETECTION_WORKING_PLANES * plane_bytes,
                warp: Some(PerFrameBytes::new(
                    plane_bytes,
                    demosaic.output_bytes,
                    dimensions.flat_gain_bytes(),
                )),
                output_bytes: config.stack.quality.resident_bytes(dimensions),
                normalization_bytes: LightShape::normalization_bytes(
                    config,
                    dimensions,
                    self.paths.len(),
                    threads,
                ),
            },
        })
    }

    /// Stack the lights against the light at index `reference` in one pass: the reference is
    /// prepared and detected first and alone, then each worker takes a light through its
    /// preparation, its detection, its registration, its warp and its store, so a light is
    /// written once and no prepared set is ever parked.
    pub(crate) fn stack_in_one_pass(
        &self,
        reference: usize,
        config: &AlignStackConfig,
        run: &IngestRun,
        progress: ProgressCallback,
    ) -> Result<AlignStackResult, AlignStackError> {
        let total = self.paths.len();
        if reference >= total {
            return Err(AlignStackError::ReferenceOutOfRange {
                index: reference,
                count: total,
            });
        }
        let threads = rayon::current_num_threads();
        let shape = self.shape(config, run, threads)?;
        let plan = MemoryPlan::single_pass(shape.run, threads, run.memory.planning());
        let StagePlan {
            parking,
            warp_concurrency: workers,
        } = StagePlan::new(&plan, shape.run.spill_bytes(), run)?;
        tracing::info!(
            frames = total,
            reference,
            planning_mb = run.memory.planning() / (1024 * 1024),
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
            &progress,
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
            reference,
            result.stars,
            config,
            &parking,
            total,
            &progress,
            cancel,
        );
        let mut buffers: Vec<Option<WarpBuffers>> = (0..slots).map(|_| None).collect();
        let reference_frame = registrar.park(
            &mut buffers[0],
            reference,
            DetectedFrame {
                image: PipelineFrame::Resident(image),
                stars: Vec::new(),
                diagnostics: Diagnostics::default(),
                stats,
            },
        )?;

        let mut slot_pairs = detectors.paired_with(&mut buffers);
        let others = intake.map(0..total, &mut slot_pairs, |(detector, buffers), index| {
            if index == reference {
                return Ok::<_, AlignStackError>(PassedLight::default());
            }
            let image = self.prepare(self.paths[index].as_ref(), intake.context())?;
            let stats = intake.admit(index, &image)?;
            let result = detector.detect(&image);
            log_detection(intake.complete_one(), total, &result);
            let frame = registrar.park(
                buffers,
                index,
                DetectedFrame {
                    image: PipelineFrame::Resident(image),
                    stars: result.stars,
                    diagnostics: Diagnostics::default(),
                    stats,
                },
            )?;
            Ok(PassedLight {
                frame,
                diagnostics: result.diagnostics,
            })
        })?;
        drop(slot_pairs);
        drop(registrar);
        // A worker's buffers are a warped frame's worth each, and its detector's pool the planes
        // detection holds: the combine sizes its chunks against the memory without them.
        drop(buffers);
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
        }
        .combine(parking, config, progress, cancel.clone())
    }

    /// Load one light and make it ready: a mosaic calibrated by the masters, cleaned of cosmic
    /// rays when asked, and demosaiced; a linear image as it decodes.
    fn prepare(&self, path: &Path, context: &LoadContext) -> Result<LinearImage, AlignStackError> {
        let Preparation::Calibrate {
            masters,
            cosmic_ray,
            notes,
        } = self.preparation
        else {
            return self.decode(path, context);
        };
        let mut cfa: CfaImage = self.decode(path, context)?;
        let outcome =
            masters
                .calibrate(&mut cfa)
                .map_err(|source| AlignStackError::Calibration {
                    path: path.to_path_buf(),
                    source: Box::new(source),
                })?;
        notes.record(outcome);
        if let Some(cosmic_ray) = cosmic_ray {
            let removed = reject_cosmic_rays(&mut cfa, cosmic_ray).map_err(|source| {
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
