//! [`FrameRegistrar`]: each frame's registration to the reference, its warp, and where it parks.

use std::sync::atomic::{AtomicUsize, Ordering};

use common::CancelToken;

use crate::frame_store::frame_stats::FrameStats;
use crate::frame_store::stored_frame::StoredFrame;
use crate::pipeline::config::AlignStackConfig;
use crate::pipeline::error::AlignStackError;
use crate::pipeline::frame_registration::FrameRegistration;
use crate::pipeline::frame_tier::FrameTier;
use crate::pipeline::pipeline_frame::PipelineFrame;
use crate::progress::progress_callback::ProgressCallback;
use crate::progress::stacking_progress::StackingStage;
use crate::progress::stage_counter::StageCounter;
use crate::registration::register;
use crate::registration::resample::WarpBuffers;
use crate::registration::result::RegistrationError;
use crate::star_detection::star::Star;

/// Registers each frame to the reference's stars, warps it, and parks the warped frame in the
/// tier. The reference parks unwarped.
#[derive(Debug)]
pub(crate) struct FrameRegistrar<'a> {
    reference: usize,
    reference_stars: Vec<Star>,
    config: &'a AlignStackConfig,
    tier: &'a FrameTier,
    cancel: &'a CancelToken,
    /// The frames registered to the reference: every frame but the reference.
    others: usize,
    attempted: AtomicUsize,
    /// Counted where the work ends: a dropped frame counts as much as a registered one, since the
    /// bar tracks attempts resolved, not survivors.
    resolved: StageCounter<'a>,
}

/// One parked light: its stored frame when it registered, and how it registered.
#[derive(Debug)]
pub(crate) struct ParkedFrame {
    pub(crate) stored: Option<StoredFrame>,
    pub(crate) registration: FrameRegistration,
}

/// What parking one frame needs: its pixels, its stars and its statistics.
#[derive(Debug)]
pub(crate) struct FrameToPark<'a> {
    pub(crate) index: usize,
    pub(crate) image: PipelineFrame,
    pub(crate) stars: &'a [Star],
    pub(crate) stats: FrameStats,
}

impl<'a> FrameRegistrar<'a> {
    pub(crate) fn new(
        reference: usize,
        reference_stars: Vec<Star>,
        config: &'a AlignStackConfig,
        tier: &'a FrameTier,
        frame_count: usize,
        progress: &'a ProgressCallback,
        cancel: &'a CancelToken,
    ) -> Self {
        let others = frame_count - 1;
        tracing::info!(
            reference,
            ref_stars = reference_stars.len(),
            frames = others,
            "Registering frames to the reference"
        );
        Self {
            reference,
            reference_stars,
            config,
            tier,
            cancel,
            others,
            attempted: AtomicUsize::new(0),
            resolved: StageCounter::new(progress, StackingStage::Registering, others),
        }
    }

    /// Register `frame`, warp it into `buffers` and park it, with how it registered; `None` when
    /// the run was cancelled, which the caller turns into `Cancelled` once every worker stops.
    ///
    /// The spill tier hands its buffers back once the frame is on disk, so a worker warps into
    /// pages it already faulted in. The RAM tier keeps them, and the slot refills from a fresh
    /// allocation it would make anyway.
    pub(crate) fn park(
        &self,
        buffers: &mut Option<WarpBuffers>,
        frame: FrameToPark<'_>,
    ) -> Result<Option<ParkedFrame>, AlignStackError> {
        if self.cancel.is_cancelled() {
            return Ok(None);
        }
        let FrameToPark {
            index,
            image,
            stars,
            stats,
        } = frame;
        if index == self.reference {
            // The unwarped reference has full support and unit interpolation confidence.
            return self.tier.store_reference(image, stats).map(|stored| {
                Some(ParkedFrame {
                    stored: Some(stored),
                    registration: FrameRegistration::Reference,
                })
            });
        }

        let n = self.attempted.fetch_add(1, Ordering::Relaxed) + 1;
        let registration = match register(&self.reference_stars, stars, &self.config.registration) {
            Ok(registration) => registration,
            // A pair that did not match is a frame to drop. An invalid config is not: it fails
            // identically for every pair, so dropping it would spend the whole run to report
            // `AllFramesDropped` and blame the data.
            Err(RegistrationError::InvalidConfig(invalid)) => {
                return Err(AlignStackError::RegistrationConfig(invalid));
            }
            Err(error) => {
                tracing::info!(frame = n, total = self.others, %error, "registration failed");
                self.resolved.complete_one();
                return Ok(Some(ParkedFrame {
                    stored: None,
                    registration: FrameRegistration::Dropped(error),
                }));
            }
        };
        tracing::info!(
            frame = n,
            total = self.others,
            inliers = registration.num_inliers(),
            rms = format!("{:.3}", registration.rms_error()),
            quality = format!("{:.3}", registration.quality_score()),
            transform = %registration.transform(),
            "registered"
        );
        let warp = registration.warp_transform();
        let mut warped = buffers
            .take()
            .unwrap_or_else(|| WarpBuffers::new(image.dimensions()));
        warped.warp_into(&image.source(), &warp, self.config.registration.warp);
        let metadata = image
            .metadata()
            .clone()
            .warped(&warp, image.dimensions().size());
        drop(image);
        self.resolved.complete_one();
        let stored = self.tier.store(metadata, warped, stats)?;
        *buffers = stored.reusable;
        Ok(Some(ParkedFrame {
            stored: Some(stored.frame),
            registration: FrameRegistration::Registered {
                warp: Box::new(warp),
                inliers: registration.num_inliers(),
                rms_error: registration.rms_error(),
            },
        }))
    }
}
