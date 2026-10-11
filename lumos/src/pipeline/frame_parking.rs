//! [`FrameParking`]: where the pipeline parks a frame between stages.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::frame_store::frame_quality::FrameQuality;
use crate::frame_store::frame_stats::FrameStats;
use crate::frame_store::stored_frame::StoredFrame;
use crate::frame_store::stored_image::StoredImage;
use crate::frame_store::tier::Tier;
use crate::ingest::ingest_run::IngestRun;
use crate::io::image::image_metadata::ImageMetadata;
use crate::io::image::linear::LinearImage;
use crate::memory::memory_plan::MemoryPlan;
use crate::pipeline::error::AlignStackError;
use crate::pipeline::pipeline_frame::PipelineFrame;
use crate::registration::resample::WarpBuffers;

/// The memory decisions the register-warp stage reads, both taken from one [`MemoryPlan`]: where a
/// frame parks, and how many frames are warped at once.
#[derive(Debug)]
pub(crate) struct StagePlan {
    pub(crate) parking: FrameParking,
    pub(crate) warp_concurrency: usize,
}

impl StagePlan {
    /// The stage of a run `plan` decided, whose spill writes `spill_bytes` when it spills.
    pub(crate) fn new(
        plan: &MemoryPlan,
        spill_bytes: u64,
        run: &IngestRun,
    ) -> Result<Self, AlignStackError> {
        Ok(Self {
            parking: FrameParking::new(Tier::for_plan(
                plan,
                spill_bytes,
                run.memory,
                &run.cache_dir,
            )?),
            warp_concurrency: plan.warp_concurrency,
        })
    }
}

/// A frame in the store, plus the buffers the tier released — `None` when it kept them.
#[derive(Debug)]
pub(crate) struct StoredWarp {
    pub(crate) frame: StoredFrame,
    pub(crate) reusable: Option<WarpBuffers>,
}

/// Where the pipeline keeps its frames: the run's [`Tier`], chosen once from the [`MemoryPlan`] and
/// threaded through the pipeline body, which is otherwise identical either way, and how many
/// calibrated lights it parked on disk before their registration, for the run's report.
#[derive(Debug)]
pub(crate) struct FrameParking {
    tier: Tier,
    parked: AtomicU64,
}

impl FrameParking {
    pub(crate) const fn new(tier: Tier) -> Self {
        Self {
            tier,
            parked: AtomicU64::new(0),
        }
    }

    pub(crate) const fn spills(&self) -> bool {
        self.tier.spills()
    }

    /// Park a calibrated frame between detection and registration.
    pub(crate) fn hold(&self, image: LinearImage) -> Result<PipelineFrame, AlignStackError> {
        match &self.tier {
            Tier::Resident => Ok(PipelineFrame::Resident(image)),
            Tier::Spilled { scratch, .. } => {
                let stored = StoredImage::spill(scratch, &image)?;
                self.parked.fetch_add(1, Ordering::Relaxed);
                Ok(PipelineFrame::Spilled(stored))
            }
        }
    }

    /// The calibrated lights [`Self::hold`] wrote to disk.
    pub(crate) fn parked_lights(&self) -> u64 {
        self.parked.load(Ordering::Relaxed)
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
        metadata: ImageMetadata,
        buffers: WarpBuffers,
        source_stats: FrameStats,
    ) -> Result<StoredWarp, AlignStackError> {
        let WarpBuffers {
            pixels,
            coverage,
            confidence,
            flags,
            rows,
        } = buffers;
        let quality = FrameQuality::Planes {
            coverage,
            confidence,
        };
        let image = LinearImage {
            metadata,
            pixels,
            flags,
        };
        match &self.tier {
            Tier::Resident => Ok(StoredWarp {
                frame: StoredFrame::from_memory(image, quality, source_stats),
                reusable: None,
            }),
            Tier::Spilled { scratch, .. } => {
                let frame = StoredFrame::spill(scratch, &image, &quality, source_stats)?;
                let FrameQuality::Planes {
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
                        flags: None,
                        rows,
                    }),
                })
            }
        }
    }

    /// Park a reference frame, which is stored unwarped: it carries a mask of its flags only if its
    /// source flagged pixels the warp leaves out of every other frame, and its flags only for that
    /// mask or a flag the warp carries — see [`FrameQuality::for_reference`]. One already parked on
    /// disk keeps its planes there.
    pub(crate) fn store_reference(
        &self,
        frame: PipelineFrame,
        source_stats: FrameStats,
    ) -> Result<StoredFrame, AlignStackError> {
        match (&self.tier, frame) {
            (_, PipelineFrame::Resident(image)) => {
                let quality = FrameQuality::for_reference(image.flags.as_ref());
                self.tier
                    .store(image, quality, source_stats)
                    .map_err(AlignStackError::from)
            }
            (Tier::Spilled { scratch, .. }, PipelineFrame::Spilled(stored)) => stored
                .into_reference_frame(scratch, source_stats)
                .map_err(AlignStackError::from),
            (Tier::Resident, PipelineFrame::Spilled(_)) => {
                unreachable!("the resident tier parks no frame on disk")
            }
        }
    }

    /// The tier the combine reads the stored frames through.
    pub(crate) fn into_tier(self) -> Tier {
        self.tier
    }
}

#[cfg(test)]
mod tests {
    use common::TempDir;

    use crate::combine::cache::frame_gate::FrameGate;
    use crate::frame_store::frame_stats::FrameStats;
    use crate::frame_store::run_scratch::RunScratch;
    use crate::frame_store::tier::Tier;
    use crate::internals::prelude::*;
    use crate::io::image::pixel_flags::{PixelFlags, QualityFlags};
    use crate::pipeline::frame_parking::FrameParking;
    use crate::pipeline::pipeline_frame::PipelineFrame;

    /// A reference leaves out what the warp leaves out of every other frame and keeps the flags it
    /// carries, on either tier and whether or not it was parked on disk. Four pixels, the second a
    /// cosmic ray: a mask of the excluded flags gathers every pixel but that one, and the flags the
    /// mask reads stay, the first's saturation among them when it is saturated.
    #[test]
    fn a_reference_leaves_out_the_excluded_and_keeps_the_carried() {
        let directory = TempDir::new("frame_tier_reference");
        let spill = || {
            FrameParking::new(Tier::Spilled {
                scratch: RunScratch::create(directory.path()).unwrap(),
                chunk_memory: 1 << 20,
            })
        };
        let dims = ImageDimensions::new((4, 1), 1);
        for saturated in [false, true] {
            let mut image = LinearImage::from_pixels(dims, vec![0.5, 0.25, 0.75, 1.0]);
            image.flags = PixelFlags::from_fn(dims.size(), |index| match index {
                0 if saturated => QualityFlags::SATURATED,
                1 => QualityFlags::COSMIC_RAY,
                _ => QualityFlags::default(),
            });
            for (label, tier, parked) in [
                ("ram", FrameParking::new(Tier::Resident), false),
                ("spill", spill(), false),
                ("parked", spill(), true),
            ] {
                let frame = if parked {
                    tier.hold(image.clone()).unwrap()
                } else {
                    PipelineFrame::Resident(image.clone())
                };
                let stored = tier
                    .store_reference(frame, FrameStats::measure(&image))
                    .unwrap();
                let gate = FrameGate::of(&stored, 0, 0, 4);
                assert_eq!(
                    (0..4)
                        .map(|index| gate.sample(index).map(|sample| sample.confidence))
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
                    stored.flags.as_ref().unwrap().chunk(0, 4),
                    [first, QualityFlags::COSMIC_RAY.byte(), 0, 0],
                    "{label} {saturated}"
                );
            }
        }
    }
}
