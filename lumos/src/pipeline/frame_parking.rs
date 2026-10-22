//! [`FrameParking`]: where the pipeline parks a frame between stages.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::frame::admitted::Admitted;
use crate::frame::frame_quality::FrameQuality;
use crate::frame_store::storage_tier::StorageTier;
use crate::frame_store::stored_frame::{SpillFrame, StoredFrame};
use crate::frame_store::stored_image::StoredImage;
use crate::image::linear_image::LinearImage;
use crate::pipeline::error::AlignStackError;
use crate::pipeline::pipeline_frame::PipelineFrame;
use crate::registration::resample::WarpResult;
use crate::scratch_pool::Pooled;

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
/// otherwise identical either way, and how many calibrated lights it parked on disk before their
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

    /// Park a calibrated frame between detection and registration.
    pub(super) fn hold(&self, image: LinearImage) -> Result<PipelineFrame, AlignStackError> {
        match &self.tier {
            StorageTier::Resident => Ok(PipelineFrame::Resident(image)),
            StorageTier::Spilled { scratch, .. } => {
                let stored = StoredImage::spill(scratch, &image)?;
                self.parked.fetch_add(1, Ordering::Relaxed);
                Ok(PipelineFrame::Spilled(stored))
            }
        }
    }

    /// The calibrated lights [`Self::hold`] wrote to disk.
    pub(super) fn parked_lights(&self) -> u64 {
        self.parked.load(Ordering::Relaxed)
    }

    /// Hand a warped frame to the combine's frame store.
    ///
    /// The RAM tier keeps the planes — they *are* the stored frame — so the set leaves its pool and
    /// the next frame builds its own, which it must anyway. The spill tier writes them to disk and
    /// memory-maps the files, and the set goes back to its pool when its lease drops: the next
    /// frame warps straight into pages already faulted in rather than into a fresh set.
    pub(super) fn store(
        &self,
        warped: Pooled<WarpResult>,
        admitted: Admitted,
    ) -> Result<StoredFrame, AlignStackError> {
        match &self.tier {
            StorageTier::Resident => {
                let WarpResult {
                    image,
                    coverage,
                    confidence,
                } = warped.into_inner();
                let quality = FrameQuality::Planes {
                    coverage,
                    confidence,
                };
                Ok(StoredFrame::from_memory(image, quality, admitted))
            }
            StorageTier::Spilled { scratch, .. } => {
                let quality = FrameQuality::Planes {
                    coverage: &warped.coverage,
                    confidence: &warped.confidence,
                };
                Ok(SpillFrame {
                    image: &warped.image,
                    quality: &quality,
                    admitted,
                }
                .spill(scratch)?)
            }
        }
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
                let quality = FrameQuality::for_reference(image.flags());
                self.tier
                    .store(image, quality, admitted)
                    .map_err(AlignStackError::from)
            }
            (StorageTier::Spilled { scratch, .. }, PipelineFrame::Spilled(stored)) => stored
                .into_reference_frame(scratch, admitted)
                .map_err(AlignStackError::from),
            (StorageTier::Resident, PipelineFrame::Spilled(_)) => {
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
    use crate::image::channel_layout::ChannelLayout;
    use crate::image::image_dimensions::ImageDimensions;
    use crate::image::linear_image::LinearImage;
    use crate::image::pixel_flags::PixelFlags;
    use crate::image::quality_flags::QualityFlags;
    use crate::pipeline::frame_parking::FrameParking;
    use crate::pipeline::pipeline_frame::PipelineFrame;

    /// A reference leaves out what the warp leaves out of every other frame and keeps the flags it
    /// carries, on either tier and whether or not it was parked on disk. Four pixels, the second a
    /// cosmic ray: a mask of the excluded flags gathers every pixel but that one, and the flags the
    /// mask reads stay, the first's saturation among them when it is saturated.
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
        for saturated in [false, true] {
            let mut image = LinearImage::from_interleaved(dims, vec![0.5, 0.25, 0.75, 1.0]);
            image.set_flags(PixelFlags::from_fn(dims.size(), |idx| match idx {
                0 if saturated => QualityFlags::SATURATED,
                1 => QualityFlags::COSMIC_RAY,
                _ => QualityFlags::default(),
            }));
            for (label, tier, parked) in [
                ("ram", FrameParking::new(StorageTier::Resident), false),
                ("spill", spill(), false),
                ("parked", spill(), true),
            ] {
                let frame = if parked {
                    tier.hold(image.clone()).unwrap()
                } else {
                    PipelineFrame::Resident(image.clone())
                };
                let stored = tier
                    .store_reference(frame, Admitted::measure(&image))
                    .unwrap();
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
            }
        }
    }
}
