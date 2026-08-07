//! Configuration for registered stacking pipelines.

use crate::stacking::calibration_masters::cosmic_ray::CosmicRayConfig;
use crate::stacking::combine::config::StackConfig;
use crate::stacking::pipeline::result::Error;
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

impl AlignStackConfig {
    /// Validate every stage's configuration.
    ///
    /// Each stage validates its own config where it runs, but by then the run has paid for
    /// everything upstream — and the registration stage cannot report a config problem at all,
    /// because it returns the same error type for "this config is invalid" and "these two star
    /// catalogs don't match", and the pipeline reads the latter as a frame to drop. Checking all
    /// three here means a bad config is reported as one, before any frame is decoded.
    pub fn validate(&self) -> Result<(), Error> {
        self.detection.validate().map_err(Error::DetectionConfig)?;
        self.registration
            .validate()
            .map_err(Error::RegistrationConfig)?;
        self.stack
            .validate()
            .map_err(|source| Error::Stack(source.into()))?;
        Ok(())
    }
}
