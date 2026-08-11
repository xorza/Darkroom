//! Where the pipeline parks a frame between stages.

use crate::io::image::image_metadata::ImageMetadata;
use crate::io::image::linear::LinearImage;
use crate::memory::MemoryPlan;
use crate::stacking::combine::cache_config::CacheConfig;
use crate::stacking::combine::error::Error as StackError;
use crate::stacking::frame_store::frame_stats::FrameStats;
use crate::stacking::frame_store::spill::SpillDirectory;
use crate::stacking::frame_store::warp_quality::WarpQuality;
use crate::stacking::frame_store::{StoredFrame, StoredImage};
use crate::stacking::pipeline::frame::PipelineFrame;
use crate::stacking::pipeline::result::Error;
use crate::stacking::registration::resample::WarpBuffers;

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
    Spill(SpillDirectory),
}

impl FrameTier {
    /// Spill when the plan says the frame set plus its scratch will not fit.
    pub(crate) fn for_plan(plan: &MemoryPlan, cache: &CacheConfig) -> Result<Self, Error> {
        if plan.fits_in_ram {
            return Ok(Self::Ram);
        }
        SpillDirectory::create(cache.cache_dir.clone(), cache.keep_cache)
            .map(Self::Spill)
            .map_err(|source| Error::Stack(StackError::from(source)))
    }

    pub(crate) fn spills(&self) -> bool {
        matches!(self, Self::Spill(_))
    }

    /// Park a calibrated frame between detection and registration.
    pub(crate) fn hold(&self, name: &str, image: LinearImage) -> Result<PipelineFrame, Error> {
        match self {
            Self::Ram => Ok(PipelineFrame::Resident(image)),
            Self::Spill(directory) => StoredImage::spill(&directory.path, name, &image)
                .map(PipelineFrame::Spilled)
                .map_err(|source| Error::Stack(StackError::from(source))),
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
        let quality = WarpQuality::Planes {
            coverage: buffers.coverage,
            confidence: buffers.confidence,
        };
        let image = LinearImage {
            metadata,
            pixels: buffers.pixels,
        };
        match self {
            Self::Ram => Ok(StoredWarp {
                frame: StoredFrame::from_memory(image, quality, source_stats),
                reusable: None,
            }),
            Self::Spill(directory) => {
                let frame =
                    StoredFrame::spill(&directory.path, name, &image, &quality, source_stats)
                        .map_err(|source| Error::Stack(StackError::from(source)))?;
                let WarpQuality::Planes {
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
                    }),
                })
            }
        }
    }

    /// Park a reference frame, which is stored unwarped and so carries no quality planes.
    pub(crate) fn store_reference(
        &self,
        name: &str,
        image: LinearImage,
        source_stats: FrameStats,
    ) -> Result<StoredFrame, Error> {
        match self {
            Self::Ram => Ok(StoredFrame::from_memory(
                image,
                WarpQuality::None,
                source_stats,
            )),
            Self::Spill(directory) => StoredFrame::spill(
                &directory.path,
                name,
                &image,
                &WarpQuality::None,
                source_stats,
            )
            .map_err(|source| Error::Stack(StackError::from(source))),
        }
    }

    /// Hand the directory to the combine, which owns it until its memory maps have dropped.
    pub(crate) fn into_spill_directory(self) -> Option<SpillDirectory> {
        match self {
            Self::Ram => None,
            Self::Spill(directory) => Some(directory),
        }
    }
}
