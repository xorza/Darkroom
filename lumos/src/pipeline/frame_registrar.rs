//! [`FrameRegistrar`]: each frame's registration to the reference, its warp, and where it parks.

use std::sync::atomic::{AtomicUsize, Ordering};

use common::CancelToken;

use crate::frame_store::stored_frame::StoredFrame;
use crate::image::image_dimensions::ImageDimensions;
use crate::pipeline::align_stack_config::AlignStackConfig;
use crate::pipeline::detected_frame::DetectedFrame;
use crate::pipeline::error::AlignStackError;
use crate::pipeline::frame_parking::FrameParking;
use crate::pipeline::frame_registration::FrameRegistration;
use crate::progress::progress_callback::ProgressCallback;
use crate::progress::stacking_progress::StackingStage;
use crate::progress::stage_counter::StageCounter;
use crate::registration::error::RegistrationError;
use crate::registration::resample::source_image::SourceImage;
use crate::registration::resample::{RowScratch, WarpBuffers};
use crate::scratch_pool::ScratchPool;
use crate::star_detection::star::Star;

/// Registers each frame to the reference's stars, warps it, and parks the warped frame in the
/// tier. The reference parks unwarped.
#[derive(Debug)]
pub(crate) struct FrameRegistrar<'a> {
    reference: usize,
    reference_stars: Vec<Star>,
    config: &'a AlignStackConfig,
    parking: &'a FrameParking,
    cancel: &'a CancelToken,
    /// The frames registered to the reference: every frame but the reference.
    others: usize,
    attempted: AtomicUsize,
    /// Counted where the work ends: a dropped frame counts as much as a registered one, since the
    /// bar tracks attempts resolved, not survivors.
    resolved: StageCounter<'a>,
    /// The sets the frames warp into, one per frame in flight at most.
    warps: ScratchPool<WarpBuffers>,
    /// The warp's row scratch, one per rayon job.
    rows: ScratchPool<RowScratch>,
}

/// The lights a registrar parks: how many, and the dimensions every one of them has.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LightSet {
    pub(crate) count: usize,
    pub(crate) dimensions: ImageDimensions,
}

/// One parked light: its stored frame when it registered, and how it registered.
#[derive(Debug)]
pub(crate) struct ParkedFrame {
    pub(crate) stored: Option<StoredFrame>,
    pub(crate) registration: FrameRegistration,
}

impl<'a> FrameRegistrar<'a> {
    pub(crate) fn new(
        reference: usize,
        reference_stars: Vec<Star>,
        config: &'a AlignStackConfig,
        parking: &'a FrameParking,
        lights: LightSet,
        progress: &'a ProgressCallback,
        cancel: &'a CancelToken,
    ) -> Self {
        let others = lights.count - 1;
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
            parking,
            cancel,
            others,
            attempted: AtomicUsize::new(0),
            resolved: StageCounter::new(progress, StackingStage::Registering, others),
            warps: ScratchPool::new(lights.dimensions),
            rows: ScratchPool::default(),
        }
    }

    /// Register light `index`, `frame`, warp it into a set leased from the registrar and park it,
    /// with how it registered; `None` when the run was cancelled, which the caller turns into
    /// `Cancelled` once every worker stops. See [`FrameParking::store`] for where the set goes.
    pub(crate) fn park(
        &self,
        index: usize,
        frame: DetectedFrame,
    ) -> Result<Option<ParkedFrame>, AlignStackError> {
        if self.cancel.is_cancelled() {
            return Ok(None);
        }
        let DetectedFrame {
            image,
            stars,
            stats,
            ..
        } = frame;
        if index == self.reference {
            // The unwarped reference has full support and unit interpolation confidence.
            return self.parking.store_reference(image, stats).map(|stored| {
                Some(ParkedFrame {
                    stored: Some(stored),
                    registration: FrameRegistration::Reference,
                })
            });
        }

        let n = self.attempted.fetch_add(1, Ordering::Relaxed) + 1;
        let registration = match self
            .config
            .registration
            .register(&self.reference_stars, &stars)
        {
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
        let mut warped = self.warps.acquire();
        let image = image.into_resident();
        warped.warp_into(
            &SourceImage::of(&image),
            &warp,
            self.config.registration.warp,
            &self.rows,
        );
        drop(image);
        self.resolved.complete_one();
        let stored = self.parking.store(warped, stats)?;
        Ok(Some(ParkedFrame {
            stored: Some(stored),
            registration: FrameRegistration::Registered {
                warp: Box::new(warp),
                inliers: registration.num_inliers(),
                rms_error: registration.rms_error(),
            },
        }))
    }
}
