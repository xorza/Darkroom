//! [`FrameRegistrar`]: each light's registration to the reference, its warp, and where it parks.

use crate::frame_store::stored_frame::StoredFrame;
use crate::image::image_dimensions::ImageDimensions;
use crate::pipeline::align_stack_run::AlignStackRun;
use crate::pipeline::detected_frame::DetectedFrame;
use crate::pipeline::error::AlignStackError;
use crate::pipeline::frame_parking::FrameParking;
use crate::registration::error::RegistrationError;
use crate::registration::resample::source_image::SourceImage;
use crate::registration::resample::{Warp, WarpResult, WarpScratch};
use crate::registration::warp_transform::WarpTransform;
use crate::scratch_pool::ScratchPool;
use crate::star_detection::star::Star;
use crate::star_detection::star_detector::detection_result::DetectionDiagnostics;

/// Registers each light to the reference's stars, warps it, and parks the warped frame in the
/// tier. The reference parks unwarped.
#[derive(Debug)]
pub(super) struct FrameRegistrar<'a> {
    reference: usize,
    reference_stars: Vec<Star>,
    run: &'a AlignStackRun<'a>,
    parking: &'a FrameParking,
    count: usize,
    /// The sets the lights warp into, one per light in flight at most.
    warps: ScratchPool<WarpResult>,
    /// What each warp works in beside its result.
    scratch: WarpScratch,
}

/// The lights a registrar parks: how many, the dimensions every one of them has, and the reference
/// among them with the stars the others register to.
#[derive(Debug)]
pub(super) struct LightSet {
    pub(super) count: usize,
    pub(super) dimensions: ImageDimensions,
    pub(super) reference: usize,
    pub(super) reference_stars: Vec<Star>,
}

/// What became of one light at its registration.
#[derive(Debug)]
pub(super) enum Parked {
    /// The reference, parked unwarped.
    Reference(StoredFrame),
    /// Registered and warped: its warped frame, and the fit it was warped by.
    Registered {
        frame: StoredFrame,
        warp: Box<WarpTransform>,
        inliers: usize,
        rms_error: f64,
    },
    /// Left out of the stack: why registration refused it.
    Dropped(RegistrationError),
}

/// One parked light, and its detection funnel.
#[derive(Debug)]
pub(super) struct ParkedLight {
    pub(super) parked: Parked,
    pub(super) detection: DetectionDiagnostics,
}

impl<'a> FrameRegistrar<'a> {
    /// Park `lights` in `parking`, registered and warped as `run` asks.
    pub(super) fn new(
        run: &'a AlignStackRun<'a>,
        parking: &'a FrameParking,
        lights: LightSet,
    ) -> Self {
        let LightSet {
            count,
            dimensions,
            reference,
            reference_stars,
        } = lights;
        tracing::info!(
            reference,
            ref_stars = reference_stars.len(),
            lights = count,
            "Registering lights to the reference"
        );
        Self {
            reference,
            reference_stars,
            run,
            parking,
            count,
            warps: ScratchPool::new(dimensions),
            scratch: WarpScratch::default(),
        }
    }

    /// Register light `idx`, `frame`, warp it into a set leased from the registrar and park it.
    /// See [`FrameParking::store`] for where the set goes.
    ///
    /// # Errors
    ///
    /// [`AlignStackError::Cancelled`] when the run was cancelled, or the frame store.
    pub(super) fn park(&self, idx: usize, frame: DetectedFrame) -> Result<Parked, AlignStackError> {
        if self.run.cancel().is_cancelled() {
            return Err(AlignStackError::Cancelled);
        }
        let DetectedFrame {
            image,
            stars,
            admitted,
        } = frame;
        if idx == self.reference {
            // The unwarped reference has full support and unit interpolation confidence.
            return Ok(Parked::Reference(
                self.parking.store_reference(image, admitted)?,
            ));
        }

        let registration = match self.run.registrar.register(&self.reference_stars, &stars) {
            Ok(registration) => registration,
            Err(error) => {
                tracing::info!(light = idx, total = self.count, %error, "registration failed");
                return Ok(Parked::Dropped(error));
            }
        };
        tracing::info!(
            light = idx,
            total = self.count,
            inliers = registration.num_inliers(),
            rms = format!("{:.3}", registration.rms_error()),
            quality = format!("{:.3}", registration.quality_score()),
            transform = %registration.transform(),
            "registered"
        );
        let inliers = registration.num_inliers();
        let rms_error = registration.rms_error();
        let warp = registration.into_warp();
        let mut warped = self.warps.acquire();
        let image = image.into_resident();
        let job = Warp {
            transform: &warp,
            params: self.run.config.warp,
        };
        warped.warp_into(&SourceImage::of(&image), job, &self.scratch);
        drop(image);
        Ok(Parked::Registered {
            frame: self.parking.store(warped, admitted)?,
            warp: Box::new(warp),
            inliers,
            rms_error,
        })
    }
}
