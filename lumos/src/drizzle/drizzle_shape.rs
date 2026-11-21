//! [`DrizzleShape`]: what a drizzle run holds, and the tier its drizzled frames live in.

use imaginarium::PlanarDesc;

use crate::combine::frame_read::slots::Slots;
use crate::combine::stack::planned_combine::PlannedFrames;
use crate::combine::stack_config::StackConfig;
use crate::drizzle::deposit::Deposit;
use crate::frame::frame_info::{DecoderBytes, FrameInfo};
use crate::frame_store::file_set::FilesPeek;
use crate::frame_store::run_shape::RunShape;
use crate::frame_store::run_storage::RunWrites;
use crate::frame_store::stored_layout::StoredLayout;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::frame_desc;
use crate::memory::footprint::Footprint;
use crate::memory::stage::{Stage, Units};

/// What a drizzle run holds, which decides the tier it keeps its drizzled frames in.
/// `files` is what the plan read off a set of files, each decoded when its turn comes; `None` for
/// frames handed over decoded, every one resident from the start and freed once drizzled.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DrizzleShape {
    pub(crate) input: PlanarDesc,
    pub(crate) output: PlanarDesc,
    pub(crate) deposit: Deposit,
    pub(crate) frame_count: usize,
    pub(crate) files: Option<FilesPeek>,
}

impl DrizzleShape {
    /// The run of this shape combined under `stack`, on `workers`: what it writes to its disk —
    /// the decoded frames the decode cache does not hold yet, and the drizzled ones when they
    /// spill — and what it holds beside its combine.
    pub(crate) fn run_shape(&self, stack: &StackConfig, workers: usize) -> RunShape<3> {
        RunShape {
            frame_count: self.frame_count,
            stages: self.stages(stack, workers),
            writes: RunWrites {
                kept_bytes: self.files.map_or(0, |peek| peek.kept_write_bytes()),
                spilled_bytes: (self.drizzled().heap_bytes(self.output) as u64)
                    .saturating_mul(self.frame_count as u64),
            },
            beside_bytes: self.held_bytes(),
        }
    }

    /// How a drizzled frame is stored: its channels, a drop weight and Kish size per weight plane,
    /// a flag byte, and its flat gain warped onto the output grid, a channel each — charged to
    /// every drizzled frame, as lights are divided by a flat as a rule.
    const fn drizzled(&self) -> StoredLayout {
        StoredLayout {
            channels: self.output.channel_count(),
            quality_planes: 2 * self.deposit.weight_planes(),
            flags: true,
            gain_channels: self.output.channel_count(),
        }
    }

    /// The depth of every weight plane, summed as the frames arrive and read by the gate after the
    /// combine.
    const fn depth_bytes(&self) -> usize {
        (self.deposit.weight_planes() * size_of::<f32>()).saturating_mul(self.output.pixel_count())
    }

    /// What the run holds beside every stage: the depth, and the source gains frames decoded from
    /// files share, which the caller holds of frames it handed over.
    const fn held_bytes(&self) -> usize {
        let gains_bytes = match self.files {
            Some(peek) => peek.gains_bytes,
            None => 0,
        };
        self.depth_bytes().saturating_add(gains_bytes)
    }

    /// The stages of a run of this shape combined under `stack` on `workers`: the drizzle, a frame
    /// at a time, beside what the plan's read of a file holds until that frame's decode takes it;
    /// the normalization; the combine.
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
            files,
        } = *self;
        let drizzled = self.drizzled();
        let drizzled_bytes = drizzled.heap_bytes(output);
        let input_bytes = frame_desc::decoded_byte_len(input);
        let way = FrameInfo {
            desc: input,
            cfa_type: deposit.mosaic().map(CfaType::Mosaic),
            decoder: files.map_or_else(DecoderBytes::default, |peek| peek.info.decoder),
            gain_bytes: 0,
        }
        .footprint(workers)
        .then(Footprint {
            kept: drizzled_bytes,
            peak: drizzled_bytes.saturating_add(input_bytes),
        });
        // A held input is freed as its frame is drizzled; a drizzled frame smaller than its input
        // frees at most its own size.
        let kept = match files {
            Some(_) => drizzled_bytes,
            None => drizzled_bytes - input_bytes.min(drizzled_bytes),
        };
        let held = self.held_bytes();
        // A drizzled frame covers only the pixels its drops reached; a mosaic's, in each channel by
        // its color's own.
        let [normalize, combine] = stack
            .planned()
            .stages(
                PlannedFrames {
                    count: frame_count,
                    desc: output,
                    layout: drizzled,
                    slots: Slots::new(None, output.channel_count()),
                    coverage: deposit.set_coverage(),
                    workers,
                },
                kept,
            )
            .map(|stage| Stage {
                held_bytes: stage.held_bytes + held,
                ..stage
            });
        [
            Stage {
                kept_bytes: kept,
                held_bytes: held.saturating_add(files.map_or(0, |peek| peek.held_bytes)),
                in_flight_bytes: way.transient(),
                units: Units::Serial,
            },
            normalize,
            combine,
        ]
    }
}

#[cfg(test)]
mod tests {
    use imaginarium::{ChannelLayout, PlanarDesc};

    use crate::combine::normalization::Normalization;
    use crate::combine::stack_config::StackConfig;
    use crate::drizzle::deposit::Deposit;
    use crate::drizzle::drizzle_shape::DrizzleShape;
    use crate::frame::frame_info::{DecoderBytes, FrameInfo};
    use crate::frame_store::file_set::FilesPeek;
    use crate::memory::stage::{Stage, Units};
    use crate::stack_product::quality_planes::QualityPlanes;

    /// Three mono frames of 2 × 2 drizzled from files onto 4 × 4, with no normalization and the
    /// image alone, on one worker.
    ///
    /// A drizzled frame is its channel, a drop weight and a Kish size, 3 planes of 16 pixels, 16
    /// flag bytes, and its flat gain warped onto the output, a node at pixels 0 and 4 of each axis,
    /// 2 × 2 nodes of 4 B: 224 B. Its input is 4 pixels of 4 bytes and 4 flag bytes, 20 B, measured
    /// by MRS over one tile that is the frame: 4 coefficients of 33 bytes, 28 bytes of transform a
    /// pixel, a row of 8 B and 136 of bookkeeping, 388 B, the way's peak at 408 B, 184 past the
    /// frame it keeps. The depth of the one weight plane, 64 B, is held through every stage. The
    /// combine holds the image and its flag byte, 5 B a pixel, 80 B, beside the depth, and a row
    /// copies three frames of 13 B a pixel over 4 pixels into each of two chunk copies, 312 B; the
    /// gain is read where it lies. The source gain the frames share, 2 × 2 nodes of 4 B, 16 B, is
    /// held beside the depth. A combine scratch for three frames on its one worker is 266 B: a
    /// value, a weight, three noise columns and a flag byte a frame, 21 B, the sorted keys, values,
    /// positions and kept marks, 17 B, the weights and the clamped copy, 8 B, four GESD critical
    /// values of 8 B, and the normal scores, a row of three for each of the three counts and their
    /// counts, 72 + 24 B: 3 × 46 + 32 + 96.
    ///
    /// A peek that holds 8 B for its frame's decode charges them to the drizzle alone, which takes
    /// them. The decode cache is written its two uncached frames whole, 20 B each with their 16 B
    /// gain, 72 B; frames handed over write nothing, and their gains are the caller's.
    #[test]
    fn the_depth_and_the_shared_gains_are_held_through_every_stage() {
        let stack = StackConfig {
            normalization: Normalization::None,
            quality: QualityPlanes::IMAGE_ONLY,
            ..StackConfig::light()
        };
        let input = PlanarDesc::new((2, 2), ChannelLayout::Mono);
        let peek = |decoder, gains_bytes, held_bytes| FilesPeek {
            idx: 0,
            info: FrameInfo {
                desc: input,
                cfa_type: None,
                decoder,
                gain_bytes: 16,
            },
            uncached_count: 2,
            held_bytes,
            gains_bytes,
        };
        let shape = |files| DrizzleShape {
            input,
            output: PlanarDesc::new((4, 4), ChannelLayout::Mono),
            deposit: Deposit::Channels(ChannelLayout::Mono),
            frame_count: 3,
            files,
        };
        for (gains, held) in [(0, 0), (16, 0), (16, 8)] {
            let run = shape(Some(peek(DecoderBytes::default(), gains, held))).run_shape(&stack, 1);
            assert_eq!(
                run.stages,
                [
                    Stage {
                        kept_bytes: 224,
                        held_bytes: 64 + gains + held,
                        in_flight_bytes: 184,
                        units: Units::Serial,
                    },
                    Stage {
                        kept_bytes: 224,
                        held_bytes: 64 + gains,
                        in_flight_bytes: 0,
                        units: Units::Whole,
                    },
                    Stage {
                        kept_bytes: 224,
                        held_bytes: 144 + 266 + gains,
                        in_flight_bytes: 312,
                        units: Units::Rows(4),
                    },
                ],
                "{gains} {held}"
            );
            assert_eq!(run.beside_bytes, 64 + gains);
            assert_eq!(run.writes.kept_bytes, 72);
            assert_eq!(run.writes.spilled_bytes, 3 * 224);
        }
        let held = shape(None).run_shape(&stack, 1);
        assert_eq!(held.writes.kept_bytes, 0);
        assert_eq!(held.beside_bytes, 64);
        // A decoder that holds 1000 B beside the frame it makes peaks the way at 20 + 1000 B,
        // past the statistics' 408: 1020 − 224 = 796 past the drizzled frame.
        let decoding = shape(Some(peek(
            DecoderBytes {
                source: 0,
                beside: 1000,
            },
            0,
            0,
        )));
        assert_eq!(decoding.stages(&stack, 1)[0].in_flight_bytes, 796);
    }
}
