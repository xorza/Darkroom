//! [`Tier`]: where a run keeps its frames, decided once from its memory plan.

use std::path::Path;

use imaginarium::Buffer2;

use crate::frame_store::error::FrameStoreError;
use crate::frame_store::frame_quality::FrameQuality;
use crate::frame_store::frame_stats::FrameStats;
use crate::frame_store::run_scratch::RunScratch;
use crate::frame_store::stackable_image::StackableImage;
use crate::frame_store::stored_frame::StoredFrame;
use crate::memory::memory_plan::MemoryPlan;
use crate::memory::run_memory::RunMemory;

/// Where a whole run keeps its frames: every one resident, or every one through the frame store's
/// memory maps. Chosen once from the run's [`MemoryPlan`], and the one value every stage that
/// stores a frame or sizes a combine chunk reads, so no two of them can disagree.
#[derive(Debug)]
pub(crate) enum Tier {
    /// Every plane in RAM: the combine walks whole planes and needs no budget.
    Resident,
    /// Planes in `scratch`, memory-mapped back and read in row chunks sized against
    /// `chunk_memory`: the run's planning figure, one number for the combine and for every pass
    /// after it.
    Spilled {
        scratch: RunScratch,
        chunk_memory: u64,
    },
}

impl Tier {
    /// Resident when `plan` fits the run in RAM; spilled to a scratch under `cache_dir` otherwise,
    /// its chunks sized against `memory`'s planning figure.
    pub(crate) fn for_plan(
        plan: &MemoryPlan,
        memory: RunMemory,
        cache_dir: &Path,
    ) -> Result<Self, FrameStoreError> {
        if plan.fits_in_ram {
            return Ok(Self::Resident);
        }
        Ok(Self::Spilled {
            scratch: RunScratch::create(cache_dir)?,
            chunk_memory: memory.planning(),
        })
    }

    pub(crate) const fn spills(&self) -> bool {
        matches!(self, Self::Spilled { .. })
    }

    /// What a spilled combine sizes its row chunks against; `None` for a resident one.
    pub(crate) const fn chunk_memory(&self) -> Option<u64> {
        match self {
            Self::Resident => None,
            Self::Spilled { chunk_memory, .. } => Some(*chunk_memory),
        }
    }

    /// Keep one frame: moved in whole on the resident tier, written to the scratch and mapped
    /// back on the spilled one.
    pub(crate) fn store(
        &self,
        image: impl StackableImage,
        quality: FrameQuality<Buffer2<f32>>,
        source_stats: FrameStats,
    ) -> Result<StoredFrame, FrameStoreError> {
        match self {
            Self::Resident => Ok(StoredFrame::from_memory(image, quality, source_stats)),
            Self::Spilled { scratch, .. } => {
                StoredFrame::spill(scratch, &image, &quality, source_stats)
            }
        }
    }
}
