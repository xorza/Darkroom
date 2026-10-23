//! [`LightShape`]: what a run of lights holds, and the stages it is planned over.

use crate::combine::frame_read::slots::Slots;
use crate::combine::normalization::planned_normalization::SetCoverage;
use crate::combine::stack::planned_combine::{PlannedCombine, PlannedFrames};
use crate::frame::frame_peek::FramePeek;
use crate::frame_store::run_storage::{RunStorage, RunWrites};
use crate::frame_store::stored_layout::StoredLayout;
use crate::image::image_dimensions::ImageDimensions;
use crate::memory::footprint::Footprint;
use crate::memory::run_plan::RunPlan;
use crate::memory::stage::{Stage, Units};
use crate::pipeline::align_stack_config::AlignStackConfig;
use crate::pipeline::error::AlignStackError;
use crate::pipeline::frame_parking::{FrameParking, StagePlan};
use crate::registration::resample::WarpScratch;
use crate::star_detection::resources::DetectionResources;

/// What a run of lights holds: how many there are, each one's demosaiced dimensions, its way from
/// its file or the caller to detected, and what the caller held of it before the run read the
/// machine; what the plan's read of a light left for that light's preparation, held from the
/// plan until it is taken; the source gains the lights share, held from their decode to the end
/// of the run; and the bytes their decodes add to the decode cache.
#[derive(Debug)]
pub(super) struct LightShape {
    pub(super) count: usize,
    pub(super) dimensions: ImageDimensions,
    pub(super) prepared: Footprint,
    pub(super) held: usize,
    pub(super) peeked: usize,
    pub(super) gains: usize,
    pub(super) kept_writes: u64,
}

impl LightShape {
    /// The stage of [`Self::found_reference`] that prepares the lights.
    pub(super) const PREPARE: usize = 0;
    /// The stage of [`Self::found_reference`] that warps the lights.
    pub(super) const WARP: usize = 1;
    /// The stage of [`Self::named_reference`] that takes each light through all of it.
    pub(super) const PASS: usize = 0;

    /// [`Self::decoded`], but for lights the caller holds, which the plan charges only what the run
    /// adds to them.
    pub(super) fn held(peek: FramePeek, count: usize, workers: usize) -> Self {
        let decoded = Self::decoded(peek, count, workers);
        Self {
            held: decoded.prepared.kept,
            ..decoded
        }
    }

    /// `count` linear lights decoded one at a time, as `peek` reads them on `workers`: a light's
    /// way is its decode and its statistics.
    pub(super) fn decoded(peek: FramePeek, count: usize, workers: usize) -> Self {
        Self {
            count,
            dimensions: peek.dimensions,
            prepared: peek.footprint(workers),
            held: 0,
            peeked: 0,
            gains: 0,
            kept_writes: 0,
        }
    }

    /// The bytes a warped light keeps: its channels, its two quality planes, its flags and its
    /// gain grid.
    const fn warped(&self) -> usize {
        StoredLayout::warped(self.dimensions).heap_bytes(self.dimensions)
    }

    /// The most the lights write to a spill: each as it is warped, which is no smaller than as it
    /// was parked.
    const fn spill_bytes(&self) -> u64 {
        (self.warped() as u64).saturating_mul(self.count as u64)
    }

    /// Where `plan` parks the lights on `storage`, and the width of its stage `warp`, which warps
    /// them; the disk checked for room for what the lights write to the decode cache and the spill.
    pub(super) fn stage_plan(
        &self,
        plan: &RunPlan,
        warp: usize,
        storage: RunStorage<'_>,
    ) -> Result<StagePlan, AlignStackError> {
        let tier = storage.tier(
            plan,
            RunWrites {
                kept: self.kept_writes,
                spilled: self.spill_bytes(),
            },
        )?;
        Ok(StagePlan {
            parking: FrameParking::new(tier.holding_beside(self.gains)),
            warp_width: plan.width(warp),
        })
    }

    /// What each worker's detector holds throughout, its filters' bands on `workers`: its pool of
    /// image-sized planes.
    fn detector_bytes(&self, workers: usize) -> usize {
        DetectionResources::heap_bytes(self.dimensions.size(), workers)
    }

    /// What each light's warp on `workers` holds beside its source and its result: a flagged
    /// light's planes.
    const fn warp_bytes(&self, workers: usize) -> usize {
        WarpScratch::heap_bytes(self.dimensions.size(), workers)
    }

    /// The stages of the lights with the reference found, under `config` on `workers`: each light
    /// prepared and detected; then warped, the light it warps from and the warp's planes in hand;
    /// the normalization; the combine.
    pub(super) fn found_reference(&self, config: &AlignStackConfig, workers: usize) -> [Stage; 4] {
        let warped = self.warped() - self.held;
        let [normalize, combine] = self.combined(warped, config, workers);
        [
            Stage {
                kept: self.prepared.kept - self.held,
                held: self.peeked + self.gains,
                in_flight: self.prepared.transient() + self.detector_bytes(workers),
                units: Units::Frames,
            },
            Stage {
                kept: warped,
                held: self.gains,
                in_flight: self.prepared.kept + self.warp_bytes(workers),
                units: Units::Frames,
            },
            normalize,
            combine,
        ]
    }

    /// The stages of the lights with the reference named, under `config` on `workers`: each light
    /// taken through its preparation, its detection and its warp by one worker, which keeps only
    /// the warped light, and holds its detector and its warp's planes from one light to the next;
    /// the normalization; the combine.
    pub(super) fn named_reference(&self, config: &AlignStackConfig, workers: usize) -> [Stage; 3] {
        debug_assert_eq!(self.held, 0, "a single pass decodes its own lights");
        let warped = self.warped();
        let [normalize, combine] = self.combined(warped, config, workers);
        [
            Stage {
                kept: warped,
                held: self.peeked + self.gains,
                in_flight: self.prepared.peak
                    + self.detector_bytes(workers)
                    + self.warp_bytes(workers),
                units: Units::Frames,
            },
            normalize,
            combine,
        ]
    }

    /// The normalization and the combine of the warped lights, each kept at `kept`, beside the
    /// source gains: every warped light covers only the pixels its warp reached.
    fn combined(&self, kept: usize, config: &AlignStackConfig, workers: usize) -> [Stage; 2] {
        let dimensions = self.dimensions;
        let stages = PlannedCombine {
            normalization: config.stack.normalization,
            quality: config.stack.quality,
        }
        .stages(
            PlannedFrames {
                count: self.count,
                dimensions,
                layout: StoredLayout::warped(dimensions),
                slots: Slots::new(None, dimensions.channels()),
                coverage: SetCoverage::Common,
                workers,
            },
            kept,
        );
        stages.map(|stage| Stage {
            held: stage.held + self.gains,
            ..stage
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::combine::normalization::Normalization;
    use crate::combine::stack_config::StackConfig;
    use crate::frame::frame_peek::FramePeek;
    use crate::image::channel_layout::ChannelLayout;
    use crate::image::image_dimensions::ImageDimensions;
    use crate::io::frame_info::DecoderBytes;
    use crate::memory::stage::{Stage, Units};
    use crate::pipeline::align_stack_config::AlignStackConfig;
    use crate::pipeline::light_shape::LightShape;
    use crate::stack_product::quality_planes::QualityPlanes;

    /// Three mono lights of 4 × 4 with no normalization and the image alone, on one worker.
    ///
    /// A light decodes to 16 pixels of 4 bytes and 16 flag bytes, 80 B, and its statistics are
    /// MRS over one tile that is the frame: 16 coefficients of 33 bytes, 28 bytes of transform a
    /// pixel, a row of 16 B and 136 of bookkeeping, 1128 B. Its detector holds 3 planes, 192 B,
    /// 4 bitmasks of 4 rows padded to 128 bits, 256 B, 2 surfaces of one tile, its value and second
    /// derivative, 2 centres and a reader's 3 + 4 samples, 44 B each, its worker's band of the
    /// widest filter, 57 rows of 4 samples, 912 B, and its worker's deblend set, room for the
    /// frame's 16 pixels at 41 B, 656 B: 2104 B.
    /// Warped it holds 3 planes, 192 B, its flags and a 2 × 2 gain grid, 16 B each: 224 B, and a
    /// row copies 12 + 1 B a pixel of three over 4 pixels into each of two chunk copies, 312 B;
    /// the gain is read where it lies. The combine holds the image and its flag byte, 80 B, and its
    /// one worker a combine scratch for three frames, 266 B (see the drizzle shape's test). A warp's
    /// flag planes are the grown flags and the warped flags', a byte a pixel each, 32 B, and its
    /// worker's block ORs of the growth, two of a strip of the 4 columns down the 4 rows, 32 B:
    /// 64 B.
    ///
    /// Found, a light in work holds its statistics and its detector, 3232 B, and a warp its 80 B
    /// source and its planes, 144 B. The source gains the lights share stay beside every stage. Named, a worker holds the light at its peak, 1208 B, the
    /// detector and the warp's planes, 3376 B. Held by the caller, a light keeps nothing until it
    /// is warped, then 224 − 80 = 144 B.
    #[test]
    fn a_light_is_kept_as_each_stage_leaves_it() {
        let peek = FramePeek {
            dimensions: ImageDimensions::new((4, 4), ChannelLayout::Mono),
            decoder: DecoderBytes::default(),
            mosaic: None,
        };
        let config = AlignStackConfig {
            stack: StackConfig {
                normalization: Normalization::None,
                quality: QualityPlanes::IMAGE_ONLY,
                ..StackConfig::light()
            },
            ..AlignStackConfig::default()
        };
        let stage = |kept, held, in_flight, units| Stage {
            kept,
            held,
            in_flight,
            units,
        };
        let tail = |kept| {
            [
                stage(kept, 0, 0, Units::Whole),
                stage(kept, 80 + 266, 312, Units::Rows(4)),
            ]
        };
        let decoded = LightShape::decoded(peek, 3, 1);
        let [normalize, combine] = tail(224);
        assert_eq!(
            decoded.found_reference(&config, 1),
            [
                stage(80, 0, 3232, Units::Frames),
                stage(224, 0, 144, Units::Frames),
                normalize,
                combine,
            ]
        );
        assert_eq!(
            decoded.named_reference(&config, 1),
            [stage(224, 0, 3376, Units::Frames), normalize, combine]
        );
        // A file the plan's read left for a light's preparation stays beside the preparing,
        // until the light is taken: on each route's first stage.
        let peeked = LightShape {
            peeked: 30,
            ..LightShape::decoded(peek, 3, 1)
        };
        assert_eq!(
            peeked.found_reference(&config, 1)[0],
            stage(80, 30, 3232, Units::Frames)
        );
        assert_eq!(
            peeked.named_reference(&config, 1)[0],
            stage(224, 30, 3376, Units::Frames)
        );
        // The source gains the lights share, 16 B, stay beside every stage of either route.
        let gained = LightShape {
            gains: 16,
            ..LightShape::decoded(peek, 3, 1)
        };
        let [normalize, combine] = tail(224);
        let beside = |stage: Stage| Stage {
            held: stage.held + 16,
            ..stage
        };
        assert_eq!(
            gained.found_reference(&config, 1),
            [
                stage(80, 16, 3232, Units::Frames),
                stage(224, 16, 144, Units::Frames),
                beside(normalize),
                beside(combine),
            ]
        );
        assert_eq!(
            gained.named_reference(&config, 1),
            [
                stage(224, 16, 3376, Units::Frames),
                beside(normalize),
                beside(combine),
            ]
        );
        let [normalize, combine] = tail(144);
        assert_eq!(
            LightShape::held(peek, 3, 1).found_reference(&config, 1),
            [
                stage(0, 0, 3232, Units::Frames),
                stage(144, 0, 144, Units::Frames),
                normalize,
                combine,
            ]
        );
    }
}
