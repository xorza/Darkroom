//! [`FrameParking`]: where the pipeline parks a frame between stages.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::frame::admitted::Admitted;
use crate::frame::frame_quality::FrameQuality;
use crate::frame_store::frame_buffers::NewFrame;
use crate::frame_store::storage_tier::StorageTier;
use crate::frame_store::stored_frame::{SetKind, StoredFrame};
use crate::frame_store::stored_image::StoredImage;
use crate::image::linear_image::LinearImage;
use crate::image::stackable_image::StackableImage;
use crate::pipeline::error::AlignStackError;
use crate::pipeline::pipeline_frame::PipelineFrame;

/// The memory decisions the register-warp stage reads, both taken from one
/// [`RunPlan`](crate::memory::run_plan::RunPlan): where a frame parks, and how many frames are
/// warped at once.
#[derive(Debug)]
pub(super) struct StagePlan {
    pub(super) parking: FrameParking,
    pub(super) warp_width: usize,
}

/// Where the pipeline keeps its frames: the run's [`StorageTier`], chosen once from the
/// [`RunPlan`](crate::memory::run_plan::RunPlan) and threaded through the pipeline body, which is
/// otherwise identical either way, and how many calibrated lights it wrote to disk before their
/// registration, for the run's report.
#[derive(Debug)]
pub(super) struct FrameParking {
    tier: StorageTier,
    parked: AtomicU64,
}

impl FrameParking {
    pub(super) const fn new(tier: StorageTier) -> Self {
        Self {
            tier,
            parked: AtomicU64::new(0),
        }
    }

    pub(super) const fn spills(&self) -> bool {
        self.tier.spills()
    }

    /// The tier a warped frame is stored in.
    pub(super) const fn tier(&self) -> &StorageTier {
        &self.tier
    }

    /// A calibrated light, `image`, where it lies: on the spilled tier, in the planes the decode
    /// cache keeps of it, `kept`, when it keeps any; in RAM otherwise.
    pub(super) fn in_place(&self, image: LinearImage, kept: Option<StoredImage>) -> PipelineFrame {
        match kept.filter(|_| self.spills()) {
            Some(kept) => PipelineFrame::Spilled(kept),
            None => PipelineFrame::Resident(image),
        }
    }

    /// Park a calibrated light, `image`, between detection and registration: on the spilled tier,
    /// in the planes the decode cache keeps of it, `kept`, or written to the scratch when it keeps
    /// none; in RAM on the resident tier.
    pub(super) fn hold(
        &self,
        image: LinearImage,
        kept: Option<StoredImage>,
    ) -> Result<PipelineFrame, AlignStackError> {
        match (&self.tier, kept) {
            (StorageTier::Spilled { scratch, .. }, None) => {
                let stored = StoredImage::write(&image, image.flags(), scratch)?;
                self.parked.fetch_add(1, Ordering::Relaxed);
                Ok(PipelineFrame::Spilled(stored))
            }
            (_, kept) => Ok(self.in_place(image, kept)),
        }
    }

    /// The calibrated lights [`Self::hold`] wrote to disk.
    pub(super) fn parked_lights(&self) -> u64 {
        self.parked.load(Ordering::Relaxed)
    }

    /// Park a reference frame, which is stored unwarped: it carries a mask of its flags only if its
    /// source flagged pixels the warp leaves out of every other frame, and its flags only for that
    /// mask or a flag the warp carries — see [`FrameQuality::for_reference`]. One already parked on
    /// disk keeps its planes there.
    pub(super) fn store_reference(
        &self,
        frame: PipelineFrame,
        admitted: Admitted,
    ) -> Result<StoredFrame, AlignStackError> {
        match (&self.tier, frame) {
            (_, PipelineFrame::Resident(image)) => {
                let quality = FrameQuality::for_reference(image.flag_counts());
                Ok(self
                    .tier
                    .store(NewFrame { image, quality }, admitted, SetKind::Registered)?
                    .frame)
            }
            (StorageTier::Spilled { .. }, PipelineFrame::Spilled(stored)) => {
                Ok(stored.into_reference_frame(admitted))
            }
            (StorageTier::Resident { .. }, PipelineFrame::Spilled(_)) => {
                unreachable!("the resident tier parks no frame on disk")
            }
        }
    }

    /// The tier the combine reads the stored frames through.
    pub(super) fn into_tier(self) -> StorageTier {
        self.tier
    }
}

#[cfg(test)]
mod tests {
    use crate::frame_store::storage_tier::CombineBudget;
    use crate::memory::run_memory::RunMemory;
    use common::TempDir;

    use crate::combine::frame_read::frame_gate::FrameGate;
    use crate::frame::admitted::Admitted;
    use crate::frame_store::run_scratch::RunScratch;
    use crate::frame_store::storage_tier::StorageTier;
    use crate::frame_store::stored_image::StoredImage;
    use crate::image::channel_layout::ChannelLayout;
    use crate::image::image_dimensions::ImageDimensions;
    use crate::image::linear_image::LinearImage;
    use crate::image::pixel_flags::PixelFlags;
    use crate::image::quality_flags::QualityFlags;
    use crate::pipeline::frame_parking::FrameParking;
    use crate::pipeline::pipeline_frame::PipelineFrame;

    /// A reference leaves out what the warp leaves out of every other frame and keeps the flags it
    /// carries, on either tier, whether or not it was parked on disk, and whether it parked in
    /// planes the decode cache keeps or ones written for it. Four pixels, the second a cosmic ray:
    /// a mask of the excluded flags gathers every pixel but that one, and the flags the mask reads
    /// stay, the first's saturation among them when it is saturated. Only the light written for
    /// its parking counts as parked. A reference of a registered set gathers no samples, as
    /// normalization samples the pixels the warped frames share: one with no flag either, which
    /// carries no quality, on the spilled tier.
    #[test]
    fn a_reference_leaves_out_the_excluded_and_keeps_the_carried() {
        let dir = TempDir::new("frame_tier_reference");
        let spill = || {
            FrameParking::new(StorageTier::Spilled {
                scratch: RunScratch::create(dir.path()).unwrap(),
                combine: CombineBudget::new(RunMemory::new(1 << 20, None).budget(1)),
            })
        };
        let dims = ImageDimensions::new((4, 1), ChannelLayout::Mono);
        let kept_scratch = RunScratch::create(dir.path()).unwrap();
        for saturated in [false, true] {
            let mut image = LinearImage::from_interleaved(dims, vec![0.5, 0.25, 0.75, 1.0]);
            image.set_flags(PixelFlags::from_fn(dims.size(), |idx| match idx {
                0 if saturated => QualityFlags::SATURATED,
                1 => QualityFlags::COSMIC_RAY,
                _ => QualityFlags::default(),
            }));
            let kept = || StoredImage::write(&image, image.flags(), &kept_scratch).unwrap();
            for (label, tier, parking, parked) in [
                ("ram", FrameParking::new(StorageTier::UNPLANNED), None, 0),
                ("spill", spill(), None, 0),
                ("parked", spill(), Some(None), 1),
                ("kept", spill(), Some(Some(kept())), 0),
            ] {
                let frame = match parking {
                    Some(kept) => tier.hold(image.clone(), kept).unwrap(),
                    None => PipelineFrame::Resident(image.clone()),
                };
                let stored = tier
                    .store_reference(frame, Admitted::measure(&image))
                    .unwrap();
                assert_eq!(tier.parked_lights(), parked, "{label} {saturated}");
                let gate = FrameGate::of(&stored, 0, 0..4);
                assert_eq!(
                    (0..4)
                        .map(|idx| gate.sample(idx).map(|sample| sample.confidence))
                        .collect::<Vec<_>>(),
                    [Some(1.0), None, Some(1.0), Some(1.0)],
                    "{label} {saturated}"
                );
                let first = if saturated {
                    QualityFlags::SATURATED.byte()
                } else {
                    0
                };
                assert_eq!(
                    stored.flags.as_ref().unwrap().chunk(0..4),
                    [first, QualityFlags::COSMIC_RAY.byte(), 0, 0],
                    "{label} {saturated}"
                );
                assert!(stored.samples.is_none(), "{label} {saturated}");
            }
        }
        let plain = LinearImage::from_interleaved(dims, vec![0.5, 0.25, 0.75, 1.0]);
        let stored = spill()
            .store_reference(
                PipelineFrame::Resident(plain.clone()),
                Admitted::measure(&plain),
            )
            .unwrap();
        assert!(stored.quality.is_none());
        assert!(stored.flags.is_none());
        assert!(stored.samples.is_none());
    }
}
