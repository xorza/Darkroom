//! [`DrizzleShape`]: what a drizzle run holds, and the tier its drizzled frames live in.

use crate::combine::frame_read::slots::Slots;
use crate::combine::stack::planned_combine::{PlannedCombine, PlannedFrames};
use crate::combine::stack_config::StackConfig;
use crate::drizzle::deposit::Deposit;
use crate::drizzle::error::DrizzleError;
use crate::frame::frame_stats::FrameStats;
use crate::frame_store::run_storage::{RunStorage, RunWrites};
use crate::frame_store::stored_layout::StoredLayout;
use crate::frame_store::tier::Tier;
use crate::image::image_dimensions::ImageDimensions;
use crate::ingest::ingest_run::IngestRun;
use crate::memory::footprint::Footprint;
use crate::memory::run_plan::RunPlan;
use crate::memory::stage::{Stage, Units};

/// Where a drizzle run's input frames come from, which decides whether the caller already holds
/// them when the run reads the machine's memory.
#[derive(Debug, Clone, Copy)]
pub(crate) enum FrameOrigin {
    /// Decoded from files, one at a time, `uncached` of them not yet in the decode cache.
    Files { uncached: usize },
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
    /// The tier of a run of this shape combined under `stack`, planned over its [`Self::stages`],
    /// on `storage` once its disk holds what the run writes: the decoded frames the decode cache
    /// does not hold yet, and the drizzled ones when they spill.
    pub(crate) fn tier(
        &self,
        stack: &StackConfig,
        run: &IngestRun,
        storage: &RunStorage,
    ) -> Result<Tier, DrizzleError> {
        let budget = run.budget;
        let plan = RunPlan::of(
            self.frame_count,
            &self.stages(stack, budget.workers()),
            budget,
        );
        let uncached = match self.origin {
            FrameOrigin::Files { uncached } => uncached,
            FrameOrigin::Memory => 0,
        };
        let writes = RunWrites {
            kept: (StoredLayout::decoded(self.input).heap_bytes(self.input) as u64)
                .saturating_mul(uncached as u64),
            spilled: (self.drizzled().heap_bytes(self.output) as u64)
                .saturating_mul(self.frame_count as u64),
        };
        Ok(storage
            .tier(&plan, writes)?
            .holding_beside(self.depth_bytes()))
    }

    /// How a drizzled frame is stored: its channels, a drop weight and Kish size per weight plane,
    /// and a flag byte.
    const fn drizzled(&self) -> StoredLayout {
        StoredLayout {
            channels: self.output.channels(),
            quality_planes: 2 * self.deposit.weight_planes(),
            flags: true,
            gain_channels: 0,
        }
    }

    /// The depth of every weight plane, summed as the frames arrive and read by the gate after the
    /// combine: what the run holds beside every stage.
    const fn depth_bytes(&self) -> usize {
        self.deposit.weight_planes() * self.output.pixel_count() * size_of::<f32>()
    }

    /// The stages of a run of this shape combined under `stack` on `workers`: the drizzle, a frame
    /// at a time; the normalization; the combine.
    ///
    /// A drizzled frame's accumulator holds the same sums as the frame and becomes it in place, so
    /// the frame in flight adds its input, and the statistics measured on that input before its
    /// drizzle starts.
    fn stages(&self, stack: &StackConfig, workers: usize) -> [Stage; 3] {
        let Self {
            input,
            output,
            deposit,
            frame_count,
            origin,
        } = *self;
        let drizzled = self.drizzled();
        let drizzled_bytes = drizzled.heap_bytes(output);
        let input_bytes = StoredLayout::decoded(input).heap_bytes(input);
        let mosaic = deposit.mosaic();
        let way = Footprint::kept(input_bytes)
            .beside(FrameStats::heap_bytes(input, mosaic.as_ref(), workers))
            .then(Footprint {
                kept: drizzled_bytes,
                peak: drizzled_bytes + input_bytes,
            });
        // A held input is freed as its frame is drizzled; a drizzled frame smaller than its input
        // frees at most its own size.
        let kept = match origin {
            FrameOrigin::Files { .. } => drizzled_bytes,
            FrameOrigin::Memory => drizzled_bytes - input_bytes.min(drizzled_bytes),
        };
        let depth = self.depth_bytes();
        // A drizzled frame covers only the pixels its drops reached; a mosaic's, in each channel by
        // its color's own.
        let [normalize, combine] = PlannedCombine {
            normalization: stack.normalization,
            quality: stack.quality,
        }
        .stages(
            PlannedFrames {
                count: frame_count,
                dimensions: output,
                layout: drizzled,
                slots: Slots::new(None, output.channels()),
                coverage: deposit.set_coverage(),
                workers,
            },
            kept,
        )
        .map(|stage| Stage {
            held: stage.held + depth,
            ..stage
        });
        [
            Stage {
                kept,
                held: depth,
                in_flight: way.transient(),
                units: Units::Serial,
            },
            normalize,
            combine,
        ]
    }
}

#[cfg(test)]
mod tests {
    use crate::combine::normalization::Normalization;
    use crate::combine::stack_config::StackConfig;
    use crate::drizzle::deposit::Deposit;
    use crate::drizzle::drizzle_shape::{DrizzleShape, FrameOrigin};
    use crate::image::channel_layout::ChannelLayout;
    use crate::image::image_dimensions::ImageDimensions;
    use crate::memory::stage::{Stage, Units};
    use crate::stack_product::quality_planes::QualityPlanes;

    /// Three mono frames of 2 × 2 drizzled from files onto 4 × 4, with no normalization and the
    /// image alone, on one worker.
    ///
    /// A drizzled frame is its channel, a drop weight and a Kish size, 3 planes of 16 pixels, and
    /// 16 flag bytes: 208 B. Its input is 4 pixels of 4 bytes and 4 flag bytes, 20 B, measured by
    /// MRS over one tile that is the frame: 4 coefficients of 33 bytes, 28 bytes of transform a
    /// pixel, a row of 8 B and 136 of bookkeeping, 388 B, the way's peak at 408 B, 200 past the
    /// frame it keeps. The depth of the one weight plane, 64 B, is held through every stage. The
    /// combine holds the image and its flag byte, 5 B a pixel, 80 B, beside the depth, and a row
    /// reads three frames of 13 B a pixel over 4 pixels, 156 B.
    #[test]
    fn the_depth_is_held_through_every_stage() {
        let shape = DrizzleShape {
            input: ImageDimensions::new((2, 2), ChannelLayout::Mono),
            output: ImageDimensions::new((4, 4), ChannelLayout::Mono),
            deposit: Deposit::Channels(ChannelLayout::Mono),
            frame_count: 3,
            origin: FrameOrigin::Files { uncached: 0 },
        };
        let stack = StackConfig {
            normalization: Normalization::None,
            quality: QualityPlanes::IMAGE_ONLY,
            ..StackConfig::light()
        };
        assert_eq!(
            shape.stages(&stack, 1),
            [
                Stage {
                    kept: 208,
                    held: 64,
                    in_flight: 200,
                    units: Units::Serial,
                },
                Stage {
                    kept: 208,
                    held: 64,
                    in_flight: 0,
                    units: Units::Whole,
                },
                Stage {
                    kept: 208,
                    held: 144,
                    in_flight: 156,
                    units: Units::Rows(4),
                },
            ]
        );
    }
}
