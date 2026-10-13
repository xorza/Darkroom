//! [`RegisteredSet`]: every light's registration outcome, and the combine of the survivors.

use crate::combine::frame_cache::core::CacheCore;
use crate::combine::stack::stack_stored_frames;
use crate::image::image_dimensions::ImageDimensions;
use crate::image::image_metadata::ImageMetadata;
use crate::pipeline::align_stack_result::{AlignStackResult, AlignmentSummary};
use crate::pipeline::align_stack_run::AlignStackRun;
use crate::pipeline::error::AlignStackError;
use crate::pipeline::frame_parking::FrameParking;
use crate::pipeline::frame_registrar::ParkedFrame;
use crate::star_detection::star_detector::detection_result::DetectionDiagnostics;

/// Every light's parked frame in input order — `None` only where the run was cancelled — with
/// what the combine of the survivors and the result need, and the tier the frames are parked in.
#[derive(Debug)]
pub(crate) struct RegisteredSet {
    pub(crate) outcomes: Vec<Option<ParkedFrame>>,
    pub(crate) reference: usize,
    /// The reference's: the master follows the alignment anchor, not whichever frame reaches the
    /// combine first.
    pub(crate) metadata: ImageMetadata,
    pub(crate) dimensions: ImageDimensions,
    /// Each light's detection funnel, in input order, including the lights that were dropped.
    pub(crate) detection: Vec<DetectionDiagnostics>,
    pub(crate) parking: FrameParking,
}

impl RegisteredSet {
    /// Combine the frames that registered, under `run`.
    pub(super) fn combine(
        self,
        run: &AlignStackRun<'_>,
    ) -> Result<AlignStackResult, AlignStackError> {
        let cancel = run.cancel();
        if cancel.is_cancelled() {
            return Err(AlignStackError::Cancelled);
        }
        let total = self.outcomes.len();
        let mut frames = Vec::with_capacity(total);
        let mut survivors = Vec::with_capacity(total);
        let mut registrations = Vec::with_capacity(total);
        let mut dropped = Vec::new();
        // Ascending without a sort: the outcomes are in input order.
        for (index, outcome) in self.outcomes.into_iter().enumerate() {
            let parked = outcome.expect("only a cancelled run leaves a light unparked");
            match parked.stored {
                Some(frame) => {
                    frames.push(frame);
                    survivors.push(index);
                }
                None => dropped.push(index),
            }
            registrations.push(parked.registration);
        }
        tracing::info!(
            aligned = frames.len(),
            dropped = dropped.len(),
            "Registration complete"
        );

        // Only the reference survived: every other frame dropped. A lone reference input is fine;
        // nothing aligned out of more than one input is an error.
        if frames.len() <= 1 && total > 1 {
            return Err(AlignStackError::AllFramesDropped { count: total - 1 });
        }

        let registered = frames.len();
        tracing::info!(frames = registered, "Stacking aligned frames");
        let parked_lights = self.parking.parked_lights();
        let mut stacked = stack_stored_frames(
            frames,
            CacheCore {
                tier: self.parking.into_tier(),
                dimensions: self.dimensions,
                metadata: self.metadata,
                monitor: run.ingest.monitor(),
            },
            &run.config.stack.for_survivors(&dropped),
        )
        // The combine numbers the survivors by position; the caller numbers its lights.
        .map_err(|error| error.renumbered(&survivors))?;
        tracing::info!("Stack complete");
        stacked.report.parked_lights = parked_lights;

        Ok(AlignStackResult {
            product: stacked,
            alignment: AlignmentSummary {
                reference: self.reference,
                frames: registrations,
            },
            detection: self.detection,
        })
    }
}
