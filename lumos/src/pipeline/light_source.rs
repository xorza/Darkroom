//! [`LightSource`]: where a registered stack's lights come from, and the stage that detects them.

use std::path::Path;

use crate::calibration_masters::CalibrationMasters;
use crate::calibration_masters::cosmic_ray;
use crate::calibration_masters::cosmic_ray::config::CosmicRayConfig;
use crate::calibration_masters::cosmic_ray::reject_cosmic_rays;
use crate::frame_store::frame_stats::FrameStats;
use crate::ingest::frame_intake::{FrameIntake, IntakePlan};
use crate::ingest::ingest_run::IngestRun;
use crate::io::cancelled::Cancelled;
use crate::io::image::cfa::{CfaFrameInfo, CfaImage};
use crate::io::image::error::ImageError;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::linear::LinearImage;
use crate::io::image::load_context::LoadContext;
use crate::io::raw::demosaic::DemosaicMemory;
use crate::memory::memory_plan::{MemoryPlan, PerFrameBytes, RunShape};
use crate::memory::{DECODE_TRANSIENT_FACTOR, DETECTION_WORKING_PLANES};
use crate::pipeline::align::log_detection;
use crate::pipeline::calibrate::CalibrationNotes;
use crate::pipeline::config::AlignStackConfig;
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
use crate::star_detection::detector::{Diagnostics, StarDetector};
use crate::star_detection::star::Star;

/// Where a registered stack's lights come from.
#[derive(Debug)]
pub(crate) enum LightSource<'a, P> {
    /// Frames the caller decoded and handed over. They were in memory before the run read the
    /// machine, so the plan charges them only what the run adds, and they stay resident whatever
    /// the tier: spilling them would be a write and a read-back for nothing.
    Held(Vec<LinearImage>),
    /// Mosaics read from files, each calibrated, cleaned of cosmic rays and demosaiced on a worker.
    Raw(RawLights<'a, P>),
}

/// Mosaic lights and what each goes through before its stars are detected.
#[derive(Debug)]
pub(crate) struct RawLights<'a, P> {
    pub(crate) paths: &'a [P],
    pub(crate) masters: &'a CalibrationMasters,
    pub(crate) cosmic_ray: Option<&'a CosmicRayConfig>,
    /// What calibrating them could not check, counted for the run's report.
    pub(crate) notes: &'a CalibrationNotes,
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
            Self::Raw(raw) => raw.paths.len(),
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
        let shape = self.shape(config, run)?;
        let plan = MemoryPlan::plan(
            shape.run,
            rayon::current_num_threads(),
            run.memory.planning(),
        );
        let stage = StagePlan::new(&plan, run)?;
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
            Self::Raw(raw) => intake.map(0..total, detectors.slots(), |detector, index| {
                let image = raw.prepare(raw.paths[index].as_ref(), intake.context())?;
                detect(detector, index, image)?.parked(&stage.parking)
            })?,
        };
        Ok(DetectedLights { frames, stage })
    }

    /// The run's shape, from the first held frame or the first file's header: no light is decoded
    /// before the tier is chosen.
    fn shape(
        &self,
        config: &AlignStackConfig,
        run: &IngestRun,
    ) -> Result<LightShape, AlignStackError> {
        let quality = config.stack.quality;
        match self {
            Self::Held(frames) => {
                let dimensions = frames[0].dimensions();
                let frame_bytes = dimensions.frame_bytes();
                let plane_bytes = dimensions.pixel_count() * size_of::<f32>();
                Ok(LightShape {
                    dimensions,
                    run: RunShape {
                        frame_count: frames.len(),
                        // What a held frame's pass adds is the copy its statistics sort.
                        decode: DemosaicMemory {
                            output_bytes: frame_bytes,
                            peak_bytes: DECODE_TRANSIENT_FACTOR * frame_bytes,
                        },
                        held_bytes: frame_bytes,
                        detection_bytes: DETECTION_WORKING_PLANES * plane_bytes,
                        warp: Some(PerFrameBytes::new(
                            plane_bytes,
                            frame_bytes,
                            dimensions.flat_gain_bytes(),
                        )),
                        output_bytes: quality.resident_bytes(dimensions),
                    },
                })
            }
            Self::Raw(raw) => raw.shape(config, run),
        }
    }
}

impl<P: AsRef<Path> + Sync> RawLights<'_, P> {
    /// The run's shape, from the first file's header.
    fn shape(
        &self,
        config: &AlignStackConfig,
        run: &IngestRun,
    ) -> Result<LightShape, AlignStackError> {
        let first = self.paths[0].as_ref();
        let info = CfaFrameInfo::from_file(first, &run.context).map_err(|source| {
            AlignStackError::Load {
                path: first.to_path_buf(),
                source: Box::new(source),
            }
        })?;
        let plane_bytes = info.dimensions.pixel_count() * size_of::<f32>();
        let demosaic = info.cfa_type.demosaic_memory(info.dimensions);
        let dimensions = ImageDimensions::new(info.dimensions.size(), info.cfa_type.num_colors());
        // One frame's pass peaks at the largest of: the decoder's own bytes beside the mosaic it
        // makes, the demosaic, the cosmic-ray pass over the mosaic, and the statistics, a copy of
        // every channel beside the demosaiced frame.
        // Refused here, before any light is decoded, when the pass cannot run on this sensor.
        let cosmic_ray = match self.cosmic_ray {
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
                    .with_peak_at_least(DECODE_TRANSIENT_FACTOR * demosaic.output_bytes),
                held_bytes: 0,
                detection_bytes: DETECTION_WORKING_PLANES * plane_bytes,
                warp: Some(PerFrameBytes::new(
                    plane_bytes,
                    demosaic.output_bytes,
                    dimensions.flat_gain_bytes(),
                )),
                output_bytes: config.stack.quality.resident_bytes(dimensions),
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
        let shape = self.shape(config, run)?;
        let plan = MemoryPlan::single_pass(
            shape.run,
            rayon::current_num_threads(),
            run.memory.planning(),
        );
        let StagePlan {
            parking,
            warp_concurrency: workers,
        } = StagePlan::new(&plan, run)?;
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
        // A worker's buffers are a warped frame's worth each, and the combine sizes its chunks
        // against the memory without them.
        drop(buffers);

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

    /// Load one light, apply the masters, reject its cosmic rays when asked, and demosaic it.
    fn prepare(&self, path: &Path, context: &LoadContext) -> Result<LinearImage, AlignStackError> {
        let mut cfa = match CfaImage::from_file(path, context) {
            Ok(image) => image,
            Err(ImageError::Cancelled { .. }) => return Err(AlignStackError::Cancelled),
            Err(source) => {
                return Err(AlignStackError::Load {
                    path: path.to_path_buf(),
                    source: Box::new(source),
                });
            }
        };
        self.notes.record(self.masters.calibrate(&mut cfa)?);
        if let Some(cosmic_ray) = self.cosmic_ray {
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
}
