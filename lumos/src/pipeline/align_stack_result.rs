//! [`AlignStackResult`]: what a registered stack produced, and what became of each light.

use crate::pipeline::frame_registration::FrameRegistration;
use crate::stack_product::StackProduct;
use crate::star_detection::star_detector::detection_result::DetectionDiagnostics;

/// Outcome of a registered stack.
#[derive(Debug)]
pub struct AlignStackResult {
    /// The combined image and its ancillary per-pixel science planes.
    pub product: StackProduct,
    /// Index into the input of the alignment reference.
    pub reference: usize,
    /// What became of each light, in input order — every light detected on, those registration
    /// went on to drop among them.
    pub lights: Vec<LightOutcome>,
}

/// What became of one light: its stars' detection funnel, and how it registered.
#[derive(Debug, Clone)]
pub struct LightOutcome {
    pub detection: DetectionDiagnostics,
    pub registration: FrameRegistration,
}

impl AlignStackResult {
    /// The number of lights combined into the stack, the reference included.
    pub fn registered(&self) -> usize {
        self.lights
            .iter()
            .filter(|light| light.registration.is_stacked())
            .count()
    }

    /// The input indices dropped because registration failed, ascending.
    pub fn dropped(&self) -> Vec<usize> {
        self.lights
            .iter()
            .enumerate()
            .filter(|(_, light)| !light.registration.is_stacked())
            .map(|(index, _)| index)
            .collect()
    }
}
