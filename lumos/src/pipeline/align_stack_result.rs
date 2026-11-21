//! [`AlignStackResult`]: what a registered stack produced, and what became of each light.

use crate::pipeline::frame_registration::FrameRegistration;
use crate::stack_product::StackProduct;
use crate::star_detection::star_detector::detection_result::DetectionDiagnostics;

/// Outcome of a registered stack: the product, and what became of each light, of which exactly
/// one is the reference — the pipeline alone builds one.
#[derive(Debug)]
pub struct AlignStackResult {
    /// The combined image and its ancillary per-pixel science planes.
    pub product: StackProduct,
    lights: Vec<LightOutcome>,
}

/// What became of one light: its stars' detection funnel, and how it registered.
#[derive(Debug, Clone)]
pub struct LightOutcome {
    pub detection: DetectionDiagnostics,
    pub registration: FrameRegistration,
}

impl AlignStackResult {
    /// The result of `lights`, in input order, exactly one of them the reference.
    pub(crate) fn new(product: StackProduct, lights: Vec<LightOutcome>) -> Self {
        let references = lights
            .iter()
            .filter(|light| matches!(light.registration, FrameRegistration::Reference))
            .count();
        assert_eq!(references, 1, "a registered stack has one reference");
        Self { product, lights }
    }

    /// What became of each light, in input order — every light detected on, those registration
    /// went on to drop among them.
    pub fn lights(&self) -> &[LightOutcome] {
        &self.lights
    }

    /// The input index of the alignment reference.
    pub fn reference(&self) -> usize {
        self.lights
            .iter()
            .position(|light| matches!(light.registration, FrameRegistration::Reference))
            .expect("a registered stack has one reference")
    }

    /// The number of lights combined into the stack, the reference included.
    pub fn stacked_count(&self) -> usize {
        self.lights
            .iter()
            .filter(|light| light.registration.is_stacked())
            .count()
    }

    /// The input indices dropped because registration failed, ascending.
    pub fn dropped(&self) -> impl Iterator<Item = usize> + '_ {
        self.lights
            .iter()
            .enumerate()
            .filter(|(_, light)| !light.registration.is_stacked())
            .map(|(idx, _)| idx)
    }
}
