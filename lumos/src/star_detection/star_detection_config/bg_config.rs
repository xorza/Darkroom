//! Background estimation and refinement settings.

use std::num::NonZeroUsize;

use crate::bg_mesh::mesh_workspace::{DEFAULT_SIGMA_CLIP_MAX_ITERATIONS, MedianFilter, MeshParams};
use crate::config_field::ConfigField;
use crate::error::InvalidConfigField;

/// The smallest tile a mesh may take, in px: the finest mesh, and the most tiles it holds.
pub(crate) const MIN_TILE_SIZE: usize = 16;

/// A background refined by masking the sources and estimating again: what a crowded field needs,
/// whose sources lift the single pass's sky.
#[derive(Debug, Clone, Copy)]
pub struct IterativeRefinement {
    /// Number of refinement iterations, at most 10. Usually 1-2 is sufficient.
    pub iterations: NonZeroUsize,
    /// Radius of the disk the source mask is dilated by before the sky is re-estimated around it.
    pub mask_dilation: usize,
    /// The level, in the detection plane's own σ, above which a pixel is masked as a source. Lower
    /// than the detection threshold, so a source's faint wings are masked too: photutils masks at
    /// 2σ on the convolved data.
    pub mask_sigma: f32,
}

impl IterativeRefinement {
    /// The widest [`Self::mask_dilation`] a configuration takes.
    pub(crate) const MAX_MASK_DILATION: usize = 50;
}

impl IterativeRefinement {
    /// Validate the configuration.
    pub(super) fn validate(&self) -> Result<(), InvalidConfigField> {
        let Self {
            iterations,
            mask_dilation,
            mask_sigma,
        } = *self;
        ConfigField::count("background refinement iterations", iterations.get())
            .must_be("at most 10", iterations.get() <= 10)?;
        ConfigField::count("background refinement mask_dilation", mask_dilation)
            .must_be("at most 50", mask_dilation <= Self::MAX_MASK_DILATION)?;
        ConfigField::new("background refinement mask_sigma", mask_sigma)
            .finite_and("finite and positive", |value| value > 0.0)
    }
}

/// Configuration for tiled background estimation and optional refinement.
#[derive(Debug, Clone)]
pub struct BgConfig {
    /// Width and height of each background-estimation tile in pixels.
    pub tile_size: usize,
    /// Maximum sigma-clipping iterations per tile.
    pub sigma_clip_max_iterations: usize,
    /// The source-masking refinement; `None` estimates once, the fastest, for a sparse field of
    /// uniform sky.
    pub refinement: Option<IterativeRefinement>,
}

impl Default for BgConfig {
    fn default() -> Self {
        Self {
            tile_size: 64,
            sigma_clip_max_iterations: DEFAULT_SIGMA_CLIP_MAX_ITERATIONS,
            refinement: None,
        }
    }
}

impl BgConfig {
    /// The mesh the detector's sky and noise are measured on: these tiles and clip passes, each
    /// tile the median of its neighbourhood.
    pub(crate) const fn mesh_params(&self) -> MeshParams {
        MeshParams {
            tile_size: self.tile_size,
            sigma_clip_max_iterations: self.sigma_clip_max_iterations,
            median_filter: MedianFilter::Apply,
        }
    }

    pub(crate) fn validate(&self) -> Result<(), InvalidConfigField> {
        ConfigField::count("tile_size", self.tile_size).must_be(
            "between 16 and 256",
            (MIN_TILE_SIZE..=256).contains(&self.tile_size),
        )?;
        ConfigField::count("sigma_clip_max_iterations", self.sigma_clip_max_iterations)
            .must_be("at most 10", self.sigma_clip_max_iterations <= 10)?;
        self.refinement
            .as_ref()
            .map_or(Ok(()), IterativeRefinement::validate)
    }
}
