//! Background estimation and refinement settings.

use crate::error::InvalidConfigField;

/// Strategy for refining background estimation.
#[derive(Debug, Clone, Copy, Default)]
pub enum BackgroundRefinement {
    /// No refinement - use single-pass background estimation.
    /// Fastest option, suitable for sparse fields with uniform background.
    #[default]
    None,

    /// Iterative refinement with source masking.
    /// Detects sources above threshold, masks them, and re-estimates background.
    /// Best for crowded fields.
    Iterative {
        /// Number of refinement iterations. Usually 1-2 is sufficient.
        iterations: usize,
    },
}

impl BackgroundRefinement {
    /// Validate the configuration.
    pub(super) fn validate(&self) -> Result<(), InvalidConfigField> {
        match self {
            Self::None => Ok(()),
            Self::Iterative { iterations } => InvalidConfigField::check(
                (1..=10).contains(iterations),
                "background refinement iterations",
                "between 1 and 10",
                *iterations as f64,
            ),
        }
    }

    /// Returns the number of iterations (0 for None).
    pub fn iterations(&self) -> usize {
        match self {
            Self::Iterative { iterations } => *iterations,
            Self::None => 0,
        }
    }
}

/// Configuration for tiled background estimation and optional refinement.
#[derive(Debug, Clone)]
pub struct BackgroundConfig {
    /// Width and height of each background-estimation tile in pixels.
    pub tile_size: usize,
    /// Maximum sigma-clipping iterations per tile.
    pub sigma_clip_iterations: usize,
    /// Optional source-masking refinement strategy.
    pub refinement: BackgroundRefinement,
    /// Radius used to dilate the source mask during refinement.
    pub mask_dilation: usize,
}

impl Default for BackgroundConfig {
    fn default() -> Self {
        Self {
            tile_size: 64,
            sigma_clip_iterations: 3,
            refinement: BackgroundRefinement::None,
            mask_dilation: 3,
        }
    }
}

impl BackgroundConfig {
    pub(crate) fn validate(&self) -> Result<(), InvalidConfigField> {
        InvalidConfigField::check(
            (16..=256).contains(&self.tile_size),
            "tile_size",
            "between 16 and 256",
            self.tile_size as f64,
        )?;
        InvalidConfigField::check(
            self.sigma_clip_iterations <= 10,
            "sigma_clip_iterations",
            "at most 10",
            self.sigma_clip_iterations as f64,
        )?;
        self.refinement.validate()?;
        InvalidConfigField::check(
            self.mask_dilation <= 50,
            "bg_mask_dilation",
            "at most 50",
            self.mask_dilation as f64,
        )?;
        Ok(())
    }
}
