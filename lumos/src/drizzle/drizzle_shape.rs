//! [`DrizzleShape`]: what a drizzle run holds, and the tier its drizzled frames live in.

use crate::combine::cache::slots::Slots;
use crate::combine::config::StackConfig;
use crate::combine::normalization::{FrameNorm, SetCoverage};
use crate::drizzle::deposit::Deposit;
use crate::drizzle::error::DrizzleError;
use crate::frame_store::frame_stats::FrameStats;
use crate::frame_store::stored_layout::StoredLayout;
use crate::frame_store::tier::Tier;
use crate::ingest::ingest_run::IngestRun;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::memory::footprint::Footprint;
use crate::memory::run_plan::RunPlan;
use crate::memory::stage::{Stage, Units};

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
    /// The tier of a run of this shape combined under `stack`, planned over its [`Self::stages`].
    pub(crate) fn tier(&self, stack: &StackConfig, run: &IngestRun) -> Result<Tier, DrizzleError> {
        let budget = run.budget;
        let plan = RunPlan::of(
            self.frame_count,
            &self.stages(stack, budget.workers()),
            budget,
        );
        let spill_bytes = (self.drizzled().heap_bytes(self.output) as u64)
            .saturating_mul(self.frame_count as u64);
        Tier::for_plan(&plan, budget, spill_bytes, &run.cache_dir)
            .map(|tier| tier.holding_beside(self.depth_bytes()))
            .map_err(|source| DrizzleError::Stack(source.into()))
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
        let mosaic = match deposit {
            Deposit::Mosaic(cfa_type) => Some(cfa_type),
            Deposit::Channels(_) => None,
        };
        let way = Footprint::kept(input_bytes)
            .beside(FrameStats::heap_bytes(input, mosaic.as_ref(), workers))
            .then(Footprint {
                kept: drizzled_bytes,
                peak: drizzled_bytes + input_bytes,
            });
        // A held input is freed as its frame is drizzled; a drizzled frame smaller than its input
        // frees at most its own size.
        let kept = match origin {
            FrameOrigin::Files => drizzled_bytes,
            FrameOrigin::Memory => drizzled_bytes - input_bytes.min(drizzled_bytes),
        };
        let depth = self.depth_bytes();
        // Each channel gathered by its own weights is covered by its own plane.
        let coverage_planes = usize::from(stack.quality.coverage) * deposit.weight_planes();
        [
            Stage {
                kept,
                held: depth,
                in_flight: way.transient(),
                units: Units::Serial,
            },
            Stage {
                kept,
                // A drizzled frame covers only the pixels its drops reached; a mosaic's, in each
                // channel by its colour's own.
                held: depth
                    + FrameNorm::heap_bytes(
                        frame_count,
                        output,
                        Slots::new(None, output.channels()),
                        stack.normalization,
                        match deposit {
                            Deposit::Mosaic(_) => SetCoverage::PerChannel,
                            Deposit::Channels(_) => SetCoverage::Common,
                        },
                        workers,
                    ),
                in_flight: 0,
                units: Units::Whole,
            },
            Stage {
                kept,
                held: stack
                    .quality
                    .pixel_bytes(output.channels(), coverage_planes, true)
                    * output.pixel_count()
                    + depth,
                in_flight: frame_count * drizzled.pixel_bytes() * output.width(),
                units: Units::Rows(output.height()),
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use crate::combine::config::{Normalization, StackConfig};
    use crate::drizzle::deposit::Deposit;
    use crate::drizzle::drizzle_shape::{DrizzleShape, FrameOrigin};
    use crate::io::image::image_dimensions::ImageDimensions;
    use crate::memory::stage::{Stage, Units};
    use crate::stack_product::quality_planes::QualityPlanes;

    /// Three mono frames of 2 × 2 drizzled from files onto 4 × 4, with no normalization and the
    /// image alone, on one worker.
    ///
    /// A drizzled frame is its channel, a drop weight and a Kish size, 3 planes of 16 pixels, and
    /// 16 flag bytes: 208 B. Its input is 4 pixels of 4 bytes and 4 flag bytes, 20 B, measured by
    /// MRS over one tile that is the frame: 4 coefficients of 33 bytes, 32 bytes of transform a
    /// pixel and 88 of bookkeeping, 348 B, the way's peak at 368 B, 160 past the frame it keeps.
    /// The depth of the one weight plane, 64 B, is held through every stage. The combine holds the
    /// image and its flag byte, 5 B a pixel, 80 B, beside the depth, and a row reads three frames
    /// of 13 B a pixel over 4 pixels, 156 B.
    #[test]
    fn the_depth_is_held_through_every_stage() {
        let shape = DrizzleShape {
            input: ImageDimensions::new((2, 2), 1),
            output: ImageDimensions::new((4, 4), 1),
            deposit: Deposit::Channels(1),
            frame_count: 3,
            origin: FrameOrigin::Files,
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
                    in_flight: 160,
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
