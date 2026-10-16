//! [`DetectedFrame`]: a light and the stars detected on it, moving through the pipeline together.

use crate::frame::admitted::Admitted;
use crate::image::linear_image::LinearImage;
use crate::pipeline::error::AlignStackError;
use crate::pipeline::frame_parking::FrameParking;
use crate::pipeline::pipeline_frame::PipelineFrame;
use crate::star_detection::star::Star;
use crate::star_detection::star_detector::detection_result::DetectionDiagnostics;

/// One light whose pixels and detected stars advance through the pipeline together: `I` holds its
/// pixels, in hand as they are detected and then wherever the tier parks them.
#[derive(Debug)]
pub(super) struct DetectedFrame<I = PipelineFrame> {
    pub(super) image: I,
    pub(super) stars: Vec<Star>,
    /// The light's statistics, measured on its pixels as decoded.
    pub(super) admitted: Admitted,
}

/// A detected light, and its detection funnel, which goes to the caller beside it.
#[derive(Debug)]
pub(super) struct DetectedLight<I = PipelineFrame> {
    pub(super) frame: DetectedFrame<I>,
    pub(super) diagnostics: DetectionDiagnostics,
}

impl DetectedFrame<LinearImage> {
    /// The light kept in RAM.
    pub(super) fn resident(self) -> DetectedFrame {
        let Self {
            image,
            stars,
            admitted,
        } = self;
        DetectedFrame {
            image: PipelineFrame::Resident(image),
            stars,
            admitted,
        }
    }

    /// The light parked where `parking` says, until its registration.
    pub(super) fn parked(self, parking: &FrameParking) -> Result<DetectedFrame, AlignStackError> {
        let Self {
            image,
            stars,
            admitted,
        } = self;
        Ok(DetectedFrame {
            image: parking.hold(image)?,
            stars,
            admitted,
        })
    }
}
