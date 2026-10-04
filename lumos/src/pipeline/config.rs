//! Configuration for registered stacking pipelines.

use crate::calibration_masters::cosmic_ray::config::CosmicRayConfig;
use crate::combine::config::{StackConfig, Weighting};
use crate::combine::error::StackConfigError;
use crate::pipeline::error::AlignStackError;
use crate::registration::registration_config::RegistrationConfig;
use crate::star_detection::config::Config as StarDetectionConfig;

/// How the reference frame (the alignment anchor) is chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Reference {
    /// The sharpest frame — the lowest median FWHM — among those with the stars registration
    /// needs, ties to the lowest index.
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
    /// Validate every stage's configuration for a run over `frame_count` lights.
    ///
    /// Each stage validates its own config where it runs, but by then the run has paid for
    /// everything upstream — and the registration stage cannot report a config problem at all,
    /// because it returns the same error type for "this config is invalid" and "these two star
    /// catalogs don't match", and the pipeline reads the latter as a frame to drop. Checking every
    /// stage here means a bad config is reported as one, before any frame is decoded. Manual
    /// weights are given one per input light, so their count is checked against the lights too.
    pub(super) fn validate(&self, frame_count: usize) -> Result<(), AlignStackError> {
        self.detection
            .validate()
            .map_err(AlignStackError::DetectionConfig)?;
        self.registration
            .validate()
            .map_err(AlignStackError::RegistrationConfig)?;
        self.stack
            .validate()
            .map_err(|source| AlignStackError::Stack(source.into()))?;
        if let Weighting::Manual(weights) = &self.stack.weighting
            && weights.len() != frame_count
        {
            return Err(AlignStackError::Stack(
                StackConfigError::ManualWeightCountMismatch {
                    expected: frame_count,
                    actual: weights.len(),
                }
                .into(),
            ));
        }
        if let Some(cosmic_ray) = &self.cosmic_ray {
            cosmic_ray
                .validate()
                .map_err(AlignStackError::CosmicRayConfig)?;
        }
        Ok(())
    }
}
