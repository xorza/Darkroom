//! [`FrameTier`]: where the pipeline parks a frame between stages.

use std::sync::atomic::{AtomicU64, Ordering};

use imaginarium::Buffer2;

use crate::combine::cache::core::CacheTier;
use crate::frame_store::frame_quality::FrameQuality;
use crate::frame_store::frame_stats::FrameStats;
use crate::frame_store::run_scratch::RunScratch;
use crate::frame_store::stored_frame::StoredFrame;
use crate::ingest::ingest_run::IngestRun;
use crate::io::image::image_metadata::ImageMetadata;
use crate::io::image::linear::LinearImage;
use crate::memory::memory_plan::MemoryPlan;

use crate::frame_store::stored_image::StoredImage;
use crate::pipeline::error::AlignStackError;
use crate::pipeline::pipeline_frame::PipelineFrame;
use crate::registration::resample::WarpBuffers;

/// The memory decisions the register-warp stage reads, both taken from one [`MemoryPlan`]: where a
/// frame parks, and how many frames are warped at once.
#[derive(Debug)]
pub(crate) struct StagePlan {
    pub(crate) tier: FrameTier,
    pub(crate) warp_concurrency: usize,
}

impl StagePlan {
    pub(crate) fn new(plan: &MemoryPlan, run: &IngestRun) -> Result<Self, AlignStackError> {
        Ok(Self {
            tier: FrameTier::for_plan(plan, run)?,
            warp_concurrency: plan.warp_concurrency,
        })
    }
}

/// A frame in the store, plus the buffers the tier released — `None` when it kept them.
#[derive(Debug)]
pub(crate) struct StoredWarp {
    pub(crate) frame: StoredFrame,
    pub(crate) reusable: Option<WarpBuffers>,
}

/// The storage tier the whole run uses: everything resident, or everything through the frame
/// store's memory maps. Chosen once from the [`MemoryPlan`] and then threaded through the
/// pipeline body, which is otherwise identical either way.
#[derive(Debug)]
pub(crate) enum FrameTier {
    Ram,
    /// Spilled to `scratch`; the combine reads it in row chunks sized against `chunk_memory`, the
    /// run's planning figure. `parked` counts the calibrated lights written there before their
    /// registration, for the run's report.
    Spill {
        scratch: RunScratch,
        chunk_memory: u64,
        parked: AtomicU64,
    },
}

impl FrameTier {
    /// Spill when the plan says the frame set plus its scratch will not fit.
    fn for_plan(plan: &MemoryPlan, run: &IngestRun) -> Result<Self, AlignStackError> {
        if plan.fits_in_ram {
            return Ok(Self::Ram);
        }
        RunScratch::create(&run.cache_dir)
            .map(|scratch| Self::Spill {
                scratch,
                chunk_memory: run.memory.planning(),
                parked: AtomicU64::new(0),
            })
            .map_err(AlignStackError::from)
    }

    pub(crate) const fn spills(&self) -> bool {
        matches!(self, Self::Spill { .. })
    }

    /// Park a calibrated frame between detection and registration.
    pub(crate) fn hold(&self, image: LinearImage) -> Result<PipelineFrame, AlignStackError> {
        match self {
            Self::Ram => Ok(PipelineFrame::Resident(image)),
            Self::Spill {
                scratch, parked, ..
            } => {
                let stored = StoredImage::spill(scratch, &image)?;
                parked.fetch_add(1, Ordering::Relaxed);
                Ok(PipelineFrame::Spilled(stored))
            }
        }
    }

    /// The calibrated lights [`Self::hold`] wrote to disk.
    pub(crate) fn parked_lights(&self) -> u64 {
        match self {
            Self::Ram => 0,
            Self::Spill { parked, .. } => parked.load(Ordering::Relaxed),
        }
    }

    /// Hand a warped frame to the combine's frame store, giving its buffers back if this tier did
    /// not keep them.
    ///
    /// The RAM tier keeps them — the planes *are* the stored frame — so nothing comes back and the
    /// next frame allocates its own, which it must anyway. The spill tier writes them to disk and
    /// memory-maps the files, leaving the buffers free for the next frame: the caller can warp
    /// straight into pages already faulted in rather than into a fresh set.
    pub(crate) fn store(
        &self,
        metadata: ImageMetadata,
        buffers: WarpBuffers,
        source_stats: FrameStats,
    ) -> Result<StoredWarp, AlignStackError> {
        let WarpBuffers {
            pixels,
            coverage,
            confidence,
            flags,
            rows,
        } = buffers;
        let quality = FrameQuality::Planes {
            coverage,
            confidence,
        };
        let image = LinearImage {
            metadata,
            pixels,
            flags,
        };
        match self {
            Self::Ram => Ok(StoredWarp {
                frame: StoredFrame::from_memory(image, quality, source_stats),
                reusable: None,
            }),
            Self::Spill { scratch, .. } => {
                let frame = StoredFrame::spill(scratch, &image, &quality, source_stats)?;
                let FrameQuality::Planes {
                    coverage,
                    confidence,
                } = quality
                else {
                    unreachable!("built as `Planes` above")
                };
                Ok(StoredWarp {
                    frame,
                    reusable: Some(WarpBuffers {
                        pixels: image.pixels,
                        coverage,
                        confidence,
                        flags: None,
                        rows,
                    }),
                })
            }
        }
    }

    /// Park a reference frame, which is stored unwarped: it carries a mask of its flags only if its
    /// source flagged pixels the warp leaves out of every other frame, and its flags only for that
    /// mask or a flag the warp carries — see [`FrameQuality::for_reference`]. One already parked on
    /// disk keeps its planes there.
    pub(crate) fn store_reference(
        &self,
        frame: PipelineFrame,
        source_stats: FrameStats,
    ) -> Result<StoredFrame, AlignStackError> {
        match (self, frame) {
            (Self::Ram, PipelineFrame::Resident(image)) => {
                let quality = Self::reference_quality(&image);
                Ok(StoredFrame::from_memory(image, quality, source_stats))
            }
            (Self::Spill { scratch, .. }, PipelineFrame::Resident(image)) => {
                let quality = Self::reference_quality(&image);
                StoredFrame::spill(scratch, &image, &quality, source_stats)
                    .map_err(AlignStackError::from)
            }
            (Self::Spill { scratch, .. }, PipelineFrame::Spilled(stored)) => stored
                .into_reference_frame(scratch, source_stats)
                .map_err(AlignStackError::from),
            (Self::Ram, PipelineFrame::Spilled(_)) => {
                unreachable!("the RAM tier parks no frame on disk")
            }
        }
    }

    /// A resident reference's quality: the stored frame keeps its flags for a mask or a flag the
    /// warp carries.
    fn reference_quality(image: &LinearImage) -> FrameQuality<Buffer2<f32>> {
        FrameQuality::for_reference(image.flags.as_ref())
    }

    /// The tier the combine reads the stored frames through.
    pub(crate) const fn cache_tier(&self) -> CacheTier {
        match self {
            Self::Ram => CacheTier::Resident,
            Self::Spill { chunk_memory, .. } => CacheTier::Spilled {
                chunk_memory: *chunk_memory,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU64;

    use common::TempDir;

    use crate::combine::cache::frame_gate::FrameGate;
    use crate::frame_store::frame_stats::FrameStats;
    use crate::frame_store::run_scratch::RunScratch;
    use crate::internals::prelude::*;
    use crate::io::image::pixel_flags::{PixelFlags, QualityFlags};
    use crate::pipeline::frame_tier::FrameTier;
    use crate::pipeline::pipeline_frame::PipelineFrame;

    /// A reference leaves out what the warp leaves out of every other frame and keeps the flags it
    /// carries, on either tier and whether or not it was parked on disk. Four pixels, the second a
    /// cosmic ray: a mask of the excluded flags gathers every pixel but that one, and the flags the
    /// mask reads stay, the first's saturation among them when it is saturated.
    #[test]
    fn a_reference_leaves_out_the_excluded_and_keeps_the_carried() {
        let directory = TempDir::new("frame_tier_reference");
        let spill = || FrameTier::Spill {
            scratch: RunScratch::create(directory.path()).unwrap(),
            chunk_memory: 1 << 20,
            parked: AtomicU64::new(0),
        };
        let dims = ImageDimensions::new((4, 1), 1);
        for saturated in [false, true] {
            let mut image = LinearImage::from_pixels(dims, vec![0.5, 0.25, 0.75, 1.0]);
            image.flags = PixelFlags::from_fn(dims.size(), |index| match index {
                0 if saturated => QualityFlags::SATURATED,
                1 => QualityFlags::COSMIC_RAY,
                _ => QualityFlags::default(),
            });
            for (label, tier, parked) in [
                ("ram", FrameTier::Ram, false),
                ("spill", spill(), false),
                ("parked", spill(), true),
            ] {
                let frame = if parked {
                    tier.hold(image.clone()).unwrap()
                } else {
                    PipelineFrame::Resident(image.clone())
                };
                let stored = tier
                    .store_reference(frame, FrameStats::measure(&image))
                    .unwrap();
                let gate = FrameGate::of(&stored, 0, 4);
                assert_eq!(
                    (0..4)
                        .map(|index| gate.sample(index).map(|sample| sample.confidence))
                        .collect::<Vec<_>>(),
                    [Some(1.0), None, Some(1.0), Some(1.0)],
                    "{label} {saturated}"
                );
                let first = if saturated {
                    QualityFlags::SATURATED.byte()
                } else {
                    0
                };
                assert_eq!(
                    stored.flags.as_ref().unwrap().chunk(0, 4),
                    [first, QualityFlags::COSMIC_RAY.byte(), 0, 0],
                    "{label} {saturated}"
                );
            }
        }
    }
}
