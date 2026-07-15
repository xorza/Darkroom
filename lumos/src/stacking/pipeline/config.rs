//! Configuration for registered stacking pipelines.

use crate::stacking::calibration_masters::cosmic_ray::CosmicRayConfig;
use crate::stacking::combine::config::StackConfig;
use crate::stacking::registration::config::Config as RegistrationConfig;
use crate::stacking::star_detection::config::Config as StarDetectionConfig;

/// How the reference frame (the alignment anchor) is chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Reference {
    /// The frame with the most detected stars — the strongest registration anchor.
    #[default]
    Auto,
    /// A specific frame, by index into the input slice.
    Index(usize),
}

/// One configuration per pipeline stage plus the reference choice.
#[derive(Debug, Clone, Default)]
pub struct AlignStackConfig {
    pub detection: StarDetectionConfig,
    pub registration: RegistrationConfig,
    pub stack: StackConfig,
    pub reference: Reference,
    /// Optional single-frame cosmic-ray rejection after calibration and before demosaic.
    pub cosmic_ray: Option<CosmicRayConfig>,
}
