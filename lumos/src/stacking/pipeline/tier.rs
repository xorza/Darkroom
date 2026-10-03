//! Where the pipeline parks a frame between stages.

use crate::io::image::image_metadata::ImageMetadata;
use crate::io::image::linear::LinearImage;
use crate::memory::MemoryPlan;
use crate::memory::run_memory::RunMemory;
use crate::stacking::combine::cache::core::CacheTier;
use crate::stacking::combine::cache_config::CacheConfig;
use crate::stacking::frame_store::frame_quality::FrameQuality;
use crate::stacking::frame_store::frame_spill::FrameSpill;
use crate::stacking::frame_store::frame_stats::FrameStats;
use crate::stacking::frame_store::spill_directory::SpillDirectory;
use crate::stacking::frame_store::{StoredFrame, StoredImage};
use crate::stacking::pipeline::frame::PipelineFrame;
use crate::stacking::pipeline::result::Error;
use crate::stacking::registration::resample::WarpBuffers;

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
        cache: &CacheConfig,
        memory: RunMemory,
    ) -> Result<Self, Error> {
        Ok(Self {
            tier: FrameTier::for_plan(plan, cache, memory)?,
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
    /// Spilled into `directory`; the combine reads it in row chunks sized against
    /// `chunk_memory`, the run's planning figure.
    Spill {
        directory: SpillDirectory,
        chunk_memory: u64,
    },
}

impl FrameTier {
    /// Spill when the plan says the frame set plus its scratch will not fit.
    fn for_plan(plan: &MemoryPlan, cache: &CacheConfig, memory: RunMemory) -> Result<Self, Error> {
        if plan.fits_in_ram {
            return Ok(Self::Ram);
        }
        SpillDirectory::create(&cache.cache_dir, cache.keep_cache)
            .map(|directory| Self::Spill {
                directory,
                chunk_memory: memory.planning(),
            })
            .map_err(Error::from)
    }

    pub(crate) const fn spills(&self) -> bool {
        matches!(self, Self::Spill { .. })
    }

    /// Park a calibrated frame between detection and registration.
    pub(crate) fn hold(&self, name: &str, image: LinearImage) -> Result<PipelineFrame, Error> {
        match self {
            Self::Ram => Ok(PipelineFrame::Resident(image)),
            Self::Spill { directory, .. } => {
                StoredImage::spill(&FrameSpill::new(directory.path(), name), &image)
                    .map(PipelineFrame::Spilled)
                    .map_err(Error::from)
            }
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
        name: &str,
        metadata: ImageMetadata,
        buffers: WarpBuffers,
        source_stats: FrameStats,
    ) -> Result<StoredWarp, Error> {
        let WarpBuffers {
            pixels,
            coverage,
            confidence,
            rows,
        } = buffers;
        let quality = FrameQuality::Planes {
            coverage,
            confidence,
        };
        let image = LinearImage {
            metadata,
            pixels,
            // Warped output; see `resample::warp` for why the source's mask does not come with it.
            nulls: None,
        };
        match self {
            Self::Ram => Ok(StoredWarp {
                frame: StoredFrame::from_memory(image, quality, source_stats),
                reusable: None,
            }),
            Self::Spill { directory, .. } => {
                let frame = StoredFrame::spill(
                    &FrameSpill::new(directory.path(), name),
                    &image,
                    &quality,
                    source_stats,
                )
                .map_err(Error::from)?;
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
                        rows,
                    }),
                })
            }
        }
    }

    /// Park a reference frame, which is stored unwarped: it carries quality planes only if its
    /// source declared pixels with no measurement.
    pub(crate) fn store_reference(
        &self,
        name: &str,
        image: LinearImage,
        source_stats: FrameStats,
    ) -> Result<StoredFrame, Error> {
        let quality = FrameQuality::for_unwarped(&image);
        match self {
            Self::Ram => Ok(StoredFrame::from_memory(image, quality, source_stats)),
            Self::Spill { directory, .. } => StoredFrame::spill(
                &FrameSpill::new(directory.path(), name),
                &image,
                &quality,
                source_stats,
            )
            .map_err(Error::from),
        }
    }

    /// Hand the tier to the combine, which owns the directory until its memory maps have dropped.
    pub(crate) fn into_cache_tier(self) -> CacheTier {
        match self {
            Self::Ram => CacheTier::Resident,
            Self::Spill {
                directory,
                chunk_memory,
            } => CacheTier::Spilled {
                directory,
                chunk_memory,
            },
        }
    }
}
