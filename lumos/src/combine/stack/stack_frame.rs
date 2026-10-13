//! [`StackFrame`]: one input frame for [`StackConfig::stack_images`], with the frame quality a
//! registered frame carries.
//!
//! [`StackConfig::stack_images`]: crate::combine::stack_config::StackConfig::stack_images

use imaginarium::Buffer2;

use crate::frame::frame_quality::FrameQuality;
use crate::frame::frame_stats::FrameStats;
use crate::image::linear_image::LinearImage;
use crate::registration::resample::WarpResult;

/// One input frame for
/// [`StackConfig::stack_images`](crate::combine::stack_config::StackConfig::stack_images), with
/// the per-pixel frame quality a registered frame carries.
///
/// Its coverage plane gates whether a warped sample has meaningful source support; its confidence
/// plane is an independent inverse-variance multiplier. A frame with no frame quality has full
/// support at unit confidence throughout. Plain `LinearImage`s convert with `.into()`; registered
/// frames must use [`StackFrame::registered`] so source-domain noise is captured before
/// interpolation.
#[derive(Debug, Clone)]
pub struct StackFrame {
    pub(crate) image: LinearImage,
    pub(crate) quality: FrameQuality<Buffer2<f32>>,
    pub(crate) source_stats: FrameStats,
}

impl StackFrame {
    /// Build a registered stack frame while preserving statistics from before interpolation.
    pub fn registered(source: &LinearImage, warped: WarpResult) -> Self {
        Self {
            source_stats: FrameStats::measure(source),
            image: warped.image,
            quality: FrameQuality::Planes {
                coverage: warped.coverage,
                confidence: warped.confidence,
            },
        }
    }
}

impl From<LinearImage> for StackFrame {
    fn from(image: LinearImage) -> Self {
        let source_stats = FrameStats::measure(&image);
        let quality = FrameQuality::for_unwarped(&image);
        Self {
            image,
            quality,
            source_stats,
        }
    }
}
