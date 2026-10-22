//! [`RegisteredSet`]: every light's registration outcome, and the combine of the survivors.

use crate::combine::frame_cache::core::CacheCore;
use crate::image::image_dimensions::ImageDimensions;
use crate::image::image_metadata::ImageMetadata;
use crate::pipeline::align_stack_result::{AlignStackResult, LightOutcome};
use crate::pipeline::align_stack_run::AlignStackRun;
use crate::pipeline::error::AlignStackError;
use crate::pipeline::frame_parking::FrameParking;
use crate::pipeline::frame_registrar::{Parked, ParkedLight};
use crate::pipeline::frame_registration::FrameRegistration;

/// Every light parked, in input order, with what the combine of the survivors and the result need,
/// and the tier the frames are parked in.
#[derive(Debug)]
pub(super) struct RegisteredSet {
    pub(super) lights: Vec<ParkedLight>,
    pub(super) reference: usize,
    /// The reference's: the master follows the alignment anchor, not whichever frame reaches the
    /// combine first.
    pub(super) metadata: ImageMetadata,
    pub(super) dimensions: ImageDimensions,
    pub(super) parking: FrameParking,
}

impl RegisteredSet {
    /// Combine the lights that registered, under `run`.
    pub(super) fn combine(
        self,
        run: &AlignStackRun<'_>,
    ) -> Result<AlignStackResult, AlignStackError> {
        let cancel = run.cancel();
        if cancel.is_cancelled() {
            return Err(AlignStackError::Cancelled);
        }
        let total = self.lights.len();
        let mut frames = Vec::with_capacity(total);
        let mut survivors = Vec::with_capacity(total);
        let mut lights = Vec::with_capacity(total);
        let mut dropped = Vec::new();
        // Ascending without a sort: the lights are in input order.
        for (idx, ParkedLight { parked, detection }) in self.lights.into_iter().enumerate() {
            let (frame, registration) = match parked {
                Parked::Reference(frame) => (Some(frame), FrameRegistration::Reference),
                Parked::Registered {
                    frame,
                    warp,
                    inliers,
                    rms_error,
                } => (
                    Some(frame),
                    FrameRegistration::Registered {
                        warp,
                        inliers,
                        rms_error,
                    },
                ),
                Parked::Dropped(error) => (None, FrameRegistration::Dropped(error)),
            };
            match frame {
                Some(frame) => {
                    frames.push(frame);
                    survivors.push(idx);
                }
                None => dropped.push(idx),
            }
            lights.push(LightOutcome {
                detection,
                registration,
            });
        }
        tracing::info!(
            aligned = frames.len(),
            dropped = dropped.len(),
            "Registration complete"
        );

        // Only the reference survived: every other light dropped. A lone reference input is fine;
        // nothing aligned out of more than one input is an error.
        if frames.len() <= 1 && total > 1 {
            return Err(AlignStackError::AllFramesDropped { count: total - 1 });
        }

        tracing::info!(frames = frames.len(), "Stacking aligned frames");
        let parked_lights = self.parking.parked_lights();
        let mut stacked = run
            .config
            .stack
            .for_survivors(&dropped)
            .stack_stored_frames(
                frames,
                CacheCore {
                    tier: self.parking.into_tier(),
                    dimensions: self.dimensions,
                    metadata: self.metadata,
                    monitor: run.ingest.monitor(),
                },
            )
            // The combine numbers the survivors by position; the caller numbers its lights.
            .map_err(|error| error.renumbered(&survivors))?;
        tracing::info!("Stack complete");
        stacked.report.parked_lights = parked_lights;

        Ok(AlignStackResult {
            product: stacked,
            reference: self.reference,
            lights,
        })
    }
}
