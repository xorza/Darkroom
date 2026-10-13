//! [`AlignStackResult`]: what a registered stack produced, and how each light registered.

use crate::pipeline::frame_registration::FrameRegistration;
use crate::stack_product::StackProduct;
use crate::star_detection::star_detector::detection_result::DetectionDiagnostics;

/// Registration bookkeeping for an aligned stack.
#[derive(Debug, Clone)]
pub struct AlignmentSummary {
    /// Index into the input of the alignment reference frame.
    pub reference: usize,
    /// How each light registered, in input order: the reference, a warp with its fit, or why it
    /// was dropped.
    pub frames: Vec<FrameRegistration>,
}

impl AlignmentSummary {
    /// The number of frames combined into the stack, the reference included.
    pub fn registered(&self) -> usize {
        self.frames
            .iter()
            .filter(|frame| frame.is_stacked())
            .count()
    }

    /// The input indices dropped because registration failed, ascending.
    pub fn dropped(&self) -> Vec<usize> {
        self.frames
            .iter()
            .enumerate()
            .filter(|(_, frame)| !frame.is_stacked())
            .map(|(index, _)| index)
            .collect()
    }
}

/// Outcome of a registered stack.
#[derive(Debug)]
pub struct AlignStackResult {
    /// The combined image and its ancillary per-pixel science planes.
    pub product: StackProduct,
    /// Reference selection and frame registration outcome.
    pub alignment: AlignmentSummary,
    /// Per-frame star-detection funnel, in input order — every frame the pipeline detected on,
    /// including those registration later dropped, so an index here matches an input index.
    pub detection: Vec<DetectionDiagnostics>,
}
