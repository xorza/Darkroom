//! [`AlignStackRun`]: one registered stack under way, and the register-warp-combine body every
//! entry ends in.

use std::mem;

use common::CancelToken;

use crate::concurrency;
use crate::ingest::ingest_run::IngestRun;
use crate::pipeline::align_stack_config::{AlignStackConfig, ReferenceCandidate};
use crate::pipeline::align_stack_result::AlignStackResult;
use crate::pipeline::error::AlignStackError;
use crate::pipeline::frame_parking::StagePlan;
use crate::pipeline::frame_registrar::{FrameRegistrar, LightSet};
use crate::pipeline::light_source::DetectedLights;
use crate::pipeline::registered_set::RegisteredSet;
use crate::star_detection::star_detector::detection_result::DetectionDiagnostics;

/// One registered stack under way: what it was asked to do, and the memory reading, decode policy,
/// cancel token and progress it took at its entry.
#[derive(Debug)]
pub(super) struct AlignStackRun<'a> {
    pub(super) config: &'a AlignStackConfig,
    pub(super) ingest: IngestRun,
}

impl AlignStackRun<'_> {
    pub(super) const fn cancel(&self) -> &CancelToken {
        &self.ingest.context.cancel
    }

    /// Register every frame to the chosen reference, warp it, and combine the survivors.
    ///
    /// The single body behind every entry with [`Reference::Auto`](crate::Reference::Auto), and
    /// behind the entry of frames already decoded. The entries differ only in how a frame becomes
    /// a [`DetectedFrame`](crate::pipeline::detected_frame::DetectedFrame) — already decoded, or
    /// decoded and calibrated from a path — and the stage plan decides whether a warped output
    /// stays resident or goes to the frame store. Everything from reference selection onward is
    /// the same work either way.
    ///
    /// Every entry checked each frame's dimensions against the first before detection: `warp`
    /// reprojects into the *source* frame's grid, so a mismatch here would reach the combine as
    /// differently-sized planes.
    pub(super) fn register_warp_and_stack(
        &self,
        detected: DetectedLights,
    ) -> Result<AlignStackResult, AlignStackError> {
        let DetectedLights {
            frames: mut detected,
            stage: StagePlan {
                parking,
                warp_width,
            },
        } = detected;
        let total = detected.len();
        if self.cancel().is_cancelled() {
            return Err(AlignStackError::Cancelled);
        }
        debug_assert!(
            detected
                .iter()
                .all(|frame| frame.image.dimensions() == detected[0].image.dimensions()),
            "every entry checks the dimensions before detection"
        );

        // Taken before the frames are consumed below, so the funnel survives into the result in
        // input order — including for frames registration goes on to drop.
        let detection: Vec<DetectionDiagnostics> = detected
            .iter_mut()
            .map(|frame| mem::take(&mut frame.diagnostics))
            .collect();

        let candidates: Vec<ReferenceCandidate> = detected
            .iter()
            .zip(&detection)
            .map(|(frame, funnel)| ReferenceCandidate {
                stars: frame.stars.len(),
                median_fwhm: funnel.median_fwhm,
            })
            .collect();
        let registration = &self.config.registration;
        let reference = self.config.reference.select(
            &candidates,
            registration
                .matching
                .required_stars(registration.transform_type),
        )?;
        let metadata = detected[reference].image.metadata().clone();
        let dimensions = detected[reference].image.dimensions();
        let registrar = FrameRegistrar::new(
            self,
            &parking,
            LightSet {
                count: total,
                dimensions,
                reference,
                reference_stars: mem::take(&mut detected[reference].stars),
            },
        );
        // Taking each detected record by value frees its input image as soon as the warped output
        // exists, so this stage never holds the complete input and warped sets simultaneously.
        let outcomes = concurrency::try_par_map_bounded_owned(
            detected,
            &mut vec![(); warp_width],
            |(), index, detected| registrar.park(index, detected),
        )?;
        // The registrar's warp sets are a warped frame's worth each, and the combine sizes its
        // chunks against the memory without them.
        drop(registrar);

        RegisteredSet {
            outcomes,
            reference,
            metadata,
            dimensions,
            detection,
            parking,
        }
        .combine(self)
    }
}
