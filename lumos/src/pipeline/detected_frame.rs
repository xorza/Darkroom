//! [`DetectedFrame`]: a frame and the stars detected on it, moving through the pipeline together.

use crate::frame_store::frame_stats::FrameStats;
use crate::pipeline::pipeline_frame::PipelineFrame;
use crate::star_detection::detector::Diagnostics;
use crate::star_detection::star::Star;

/// One frame whose pixels and detected stars advance through the pipeline together.
#[derive(Debug)]
pub(crate) struct DetectedFrame {
    pub(crate) image: PipelineFrame,
    pub(crate) stars: Vec<Star>,
    /// The detection funnel for this frame, carried through to the caller rather than only logged.
    pub(crate) diagnostics: Diagnostics,
    /// The frame's statistics, measured on its pixels as decoded.
    pub(crate) stats: FrameStats,
}
