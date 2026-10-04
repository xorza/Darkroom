//! Where the pipeline parks a frame between stages.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::combine::cache::core::CacheTier;
use crate::frame_store::frame_quality::FrameQuality;
use crate::frame_store::frame_stats::FrameStats;
use crate::frame_store::run_scratch::RunScratch;
use crate::frame_store::stored_frame::StoredFrame;
use crate::ingest::ingest_config::IngestConfig;
use crate::io::image::image_metadata::ImageMetadata;
use crate::io::image::linear::LinearImage;
use crate::memory::MemoryPlan;
use crate::memory::run_memory::RunMemory;

use crate::frame_store::stored_image::StoredImage;
use crate::pipeline::error::AlignStackError;
use crate::pipeline::frame::PipelineFrame;
use crate::registration::resample::WarpBuffers;

/// The memory decisions the register-warp stage reads, both taken from one [`MemoryPlan`]: where a
/// frame parks, and how many frames are warped at once.
#[derive(Debug)]
pub(crate) struct StagePlan {
    pub(crate) tier: FrameTier,
    pub(crate) warp_concurrency: usize,
}

impl StagePlan {
    pub(crate) fn new(
        plan: &MemoryPlan,
        ingest: &IngestConfig,
        memory: RunMemory,
    ) -> Result<Self, AlignStackError> {
        Ok(Self {
            tier: FrameTier::for_plan(plan, ingest, memory)?,
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
    fn for_plan(
        plan: &MemoryPlan,
        ingest: &IngestConfig,
        memory: RunMemory,
    ) -> Result<Self, AlignStackError> {
        if plan.fits_in_ram {
            return Ok(Self::Ram);
        }
        RunScratch::create(&ingest.cache_dir)
            .map(|scratch| Self::Spill {
                scratch,
                chunk_memory: memory.planning(),
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
            // The source's flags at each output pixel; its nulls are in `coverage` (see
            // `WarpBuffers::flags`).
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

    /// Park a reference frame, which is stored unwarped: it carries quality planes only if its
    /// source declared pixels with no measurement. One already parked on disk keeps its planes
    /// there.
    pub(crate) fn store_reference(
        &self,
        frame: PipelineFrame,
        source_stats: FrameStats,
    ) -> Result<StoredFrame, AlignStackError> {
        match (self, frame) {
            (Self::Ram, PipelineFrame::Resident(image)) => {
                let quality = FrameQuality::for_unwarped(&image);
                Ok(StoredFrame::from_memory(image, quality, source_stats))
            }
            (Self::Spill { scratch, .. }, PipelineFrame::Resident(image)) => {
                let quality = FrameQuality::for_unwarped(&image);
                StoredFrame::spill(scratch, &image, &quality, source_stats)
                    .map_err(AlignStackError::from)
            }
            (Self::Spill { scratch, .. }, PipelineFrame::Spilled(stored)) => stored
                .into_frame(scratch, source_stats)
                .map_err(AlignStackError::from),
            (Self::Ram, PipelineFrame::Spilled(_)) => {
                unreachable!("the RAM tier parks no frame on disk")
            }
        }
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
