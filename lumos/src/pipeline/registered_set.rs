//! [`RegisteredSet`]: every light's registration outcome, and the combine of the survivors.

use common::CancelToken;

use crate::combine::stack::stack_stored_frames;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::image_metadata::ImageMetadata;
use crate::pipeline::config::AlignStackConfig;
use crate::pipeline::error::AlignStackError;
use crate::pipeline::frame_registrar::ParkedFrame;
use crate::pipeline::result::AlignStackResult;
use crate::pipeline::tier::FrameTier;
use crate::progress::ProgressCallback;
use crate::star_detection::detector::Diagnostics;

/// Every light's parked frame in input order — `None` only where the run was cancelled — with
/// what the combine of the survivors and the result need.
#[derive(Debug)]
pub(crate) struct RegisteredSet {
    pub(crate) outcomes: Vec<Option<ParkedFrame>>,
    pub(crate) reference: usize,
    /// The reference's: the master follows the alignment anchor, not whichever frame reaches the
    /// combine first.
    pub(crate) metadata: ImageMetadata,
    pub(crate) dimensions: ImageDimensions,
    /// Each light's detection funnel, in input order, including the lights that were dropped.
    pub(crate) detection: Vec<Diagnostics>,
}

impl RegisteredSet {
    /// Combine the frames that registered.
    pub(crate) fn combine(
        self,
        tier: &FrameTier,
        config: &AlignStackConfig,
        progress: ProgressCallback,
        cancel: CancelToken,
    ) -> Result<AlignStackResult, AlignStackError> {
        if cancel.is_cancelled() {
            return Err(AlignStackError::Cancelled);
        }
        let total = self.outcomes.len();
        let mut frames = Vec::with_capacity(total);
        let mut registrations = Vec::with_capacity(total);
        let mut dropped = Vec::new();
        // Ascending without a sort: the outcomes are in input order.
        for (index, outcome) in self.outcomes.into_iter().enumerate() {
            let parked = outcome.expect("only a cancelled run leaves a light unparked");
            match parked.stored {
                Some(frame) => frames.push(frame),
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
        let mut stacked = stack_stored_frames(
            frames,
            tier.cache_tier(),
            self.dimensions,
            self.metadata,
            &config.stack.for_survivors(&dropped),
            progress,
            cancel,
        )?;
        tracing::info!("Stack complete");
        stacked.report.parked_lights = tier.parked_lights();

        Ok(AlignStackResult::from_product(
            stacked,
            self.reference,
            registrations,
            self.detection,
        ))
    }
}
