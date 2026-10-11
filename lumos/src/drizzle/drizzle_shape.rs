//! [`DrizzleShape`]: what a drizzle run holds, and the tier its drizzled frames live in.

use crate::combine::config::StackConfig;
use crate::drizzle::deposit::Deposit;
use crate::drizzle::error::DrizzleError;
use crate::frame_store::tier::Tier;
use crate::ingest::ingest_run::IngestRun;
use crate::io::image::image_dimensions::ImageDimensions;
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

/// What a drizzle run holds, which decides the [`Tier`] it keeps its drizzled frames in.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DrizzleShape {
    pub(crate) input: ImageDimensions,
    pub(crate) output: ImageDimensions,
    pub(crate) deposit: Deposit,
    pub(crate) frame_count: usize,
    pub(crate) origin: FrameOrigin,
}

impl DrizzleShape {
    /// The tier of a run of this shape combined under `stack`: resident when the drizzled set
    /// fits beside the combine's output and the frame being drizzled with its input, every frame
    /// through the frame store's memory maps otherwise.
    pub(crate) fn tier(&self, stack: &StackConfig, run: &IngestRun) -> Result<Tier, DrizzleError> {
        let Self {
            input,
            output,
            deposit,
            frame_count,
            origin,
        } = *self;
        // A drizzled frame holds its channels, a drop weight and Kish size per weight plane, and a
        // flag byte. Its accumulator holds the same sums and becomes it in place, so the frame in
        // flight adds only its input.
        let weight_plane = output.pixel_count() * size_of::<f32>();
        let drizzled = output.frame_bytes()
            + 2 * deposit.weight_planes() * weight_plane
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
            // The run's drop depth in each weight plane, which the fill gate reads after the
            // combine, and a coverage plane per channel beyond the one counted when each channel
            // is gathered by its own weights.
            output_bytes: stack.quality.resident_bytes(output)
                + deposit.weight_planes() * weight_plane
                + usize::from(stack.quality.coverage)
                    * (deposit.weight_planes() - 1)
                    * weight_plane,
        };
        let plan = MemoryPlan::plan(shape, rayon::current_num_threads(), run.memory.planning());
        Tier::for_plan(&plan, run.memory, &run.cache_dir)
            .map_err(|source| DrizzleError::Stack(source.into()))
    }
}
