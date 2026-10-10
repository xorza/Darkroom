//! [`DrizzleTier`]: where a run keeps its drizzled frames until the combine reads them.

use imaginarium::Buffer2;

use crate::combine::cache::core::CacheTier;
use crate::combine::config::StackConfig;
use crate::drizzle::error::DrizzleError;
use crate::frame_store::frame_quality::FrameQuality;
use crate::frame_store::frame_stats::FrameStats;
use crate::frame_store::run_scratch::RunScratch;
use crate::frame_store::stored_frame::StoredFrame;
use crate::ingest::ingest_run::IngestRun;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::linear::LinearImage;
use crate::io::raw::demosaic::DemosaicMemory;
use crate::memory::memory_plan::{MemoryPlan, RunShape};

/// Where a drizzle run's input frames come from, which decides whether the caller already holds
/// them when the run reads the machine's memory.
#[derive(Debug, Clone, Copy)]
pub(crate) enum FrameOrigin {
    /// Decoded from files, one at a time.
    Files,
    /// Handed over decoded, every one resident from the start and freed once drizzled.
    Memory,
}

/// Where a drizzle run keeps its frames: every one resident, or every one through the frame
/// store's memory maps, as the [`MemoryPlan`] decides for frames `scale²` times their input.
#[derive(Debug)]
pub(crate) enum DrizzleTier {
    Ram,
    /// Spilled to `scratch`; the combine reads it in row chunks sized against `chunk_memory`.
    Spill {
        scratch: RunScratch,
        chunk_memory: u64,
    },
}

impl DrizzleTier {
    /// The tier of a run of `frame_count` frames of `input` from `origin`, drizzled onto `output`
    /// and combined under `stack`: resident when the drizzled set fits beside the combine's output
    /// and the frame being drizzled with its input.
    pub(crate) fn for_run(
        input: ImageDimensions,
        output: ImageDimensions,
        frame_count: usize,
        origin: FrameOrigin,
        stack: &StackConfig,
        run: &IngestRun,
    ) -> Result<Self, DrizzleError> {
        // A drizzled frame holds its channels, its drop weight and Kish size, and a flag byte. Its
        // accumulator holds the same sums and becomes it in place, so the frame in flight adds only
        // its input.
        let drizzled = output.frame_bytes()
            + 2 * output.pixel_count() * size_of::<f32>()
            + output.pixel_count();
        let input_bytes = input.frame_bytes() + input.pixel_count();
        // A held input is freed as its frame is drizzled; a drizzled frame smaller than its input
        // frees at most its own size from the plan's figures.
        let held_bytes = match origin {
            FrameOrigin::Files => 0,
            FrameOrigin::Memory => input_bytes.min(drizzled),
        };
        let shape = RunShape {
            frame_count,
            decode: DemosaicMemory {
                output_bytes: drizzled,
                peak_bytes: drizzled.saturating_add(input_bytes),
            },
            held_bytes,
            detection_bytes: 0,
            warp: None,
            // The run's drop depth, which the fill gate reads after the combine.
            output_bytes: stack.quality.resident_bytes(output)
                + output.pixel_count() * size_of::<f32>(),
        };
        let plan = MemoryPlan::plan(shape, rayon::current_num_threads(), run.memory.planning());
        if plan.fits_in_ram {
            return Ok(Self::Ram);
        }
        let scratch = RunScratch::create(&run.cache_dir)
            .map_err(|source| DrizzleError::Stack(source.into()))?;
        Ok(Self::Spill {
            scratch,
            chunk_memory: run.memory.planning(),
        })
    }

    /// Keep one drizzled frame.
    pub(crate) fn store(
        &self,
        image: LinearImage,
        quality: FrameQuality<Buffer2<f32>>,
        source_stats: FrameStats,
    ) -> Result<StoredFrame, DrizzleError> {
        match self {
            Self::Ram => Ok(StoredFrame::from_memory(image, quality, source_stats)),
            Self::Spill { scratch, .. } => {
                StoredFrame::spill(scratch, &image, &quality, source_stats)
                    .map_err(|source| DrizzleError::Stack(source.into()))
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
