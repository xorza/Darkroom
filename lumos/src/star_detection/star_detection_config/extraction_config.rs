//! Candidate detection, deblending, and region-filtering settings.

use crate::config_field::ConfigField;
use crate::error::InvalidConfigField;

/// Pixel connectivity for connected component labeling.
///
/// Determines which pixels are considered neighbors when grouping
/// above-threshold pixels into connected components.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Connectivity {
    /// 4-connectivity: only horizontal and vertical neighbors.
    /// Pixels at (x±1, y) and (x, y±1) are connected.
    /// Diagonal pixels are NOT connected.
    Four,
    /// 8-connectivity: includes diagonal neighbors.
    /// All 8 surrounding pixels are connected.
    /// This is the default, matching SExtractor, photutils, and SEP.
    #[default]
    Eight,
}

/// How a connected component is split into stars.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Deblend {
    /// Every residual peak at least `min_relative_height` of the component's brightest, and at
    /// least `deblend_min_separation` from a brighter one, is its own star.
    LocalMaxima {
        /// A peak's height as a share of the component's brightest residual, in `[0, 1]`: a
        /// relative height, not a prominence — no saddle to the brighter peak is measured.
        min_relative_height: f32,
    },
    /// SExtractor's tree: the component is thresholded at `n_thresholds` levels spaced
    /// exponentially between the detection threshold and its peak residual, and a branch is its
    /// own star when it holds at least `min_contrast` of the component's residual flux above
    /// that threshold.
    MultiThreshold {
        /// Levels in the tree, between 2 and 256.
        n_thresholds: usize,
        /// A branch's share of the component's residual flux, in `[0, 1]`.
        min_contrast: f32,
    },
}

/// Upper bound for [`Deblend::MultiThreshold`]'s `n_thresholds`.
///
/// Each level of the multi-threshold tree is a pass over the component's pixels that stood above
/// the level below, so the cap bounds the passes one component costs. 256 levels is already eight
/// times SExtractor's default of 32.
pub(super) const MAX_DEBLEND_N_THRESHOLDS: usize = 256;

/// Configuration for candidate detection, deblending, and region filtering.
#[derive(Debug, Clone)]
pub struct ExtractionConfig {
    /// Detection threshold in local background-noise standard deviations.
    pub sigma_threshold: f32,
    /// Pixel connectivity used to form candidate regions.
    pub connectivity: Connectivity,
    /// How components are split into stars.
    pub deblend: Deblend,
    /// Minimum separation between deblended peaks in pixels.
    pub deblend_min_separation: usize,
    /// Minimum candidate-region area in pixels.
    pub min_area: usize,
    /// Maximum candidate-region area in pixels.
    pub max_area: usize,
    /// Rejected border width in pixels.
    pub edge_margin: usize,
}

impl Default for ExtractionConfig {
    fn default() -> Self {
        Self {
            sigma_threshold: 4.0,
            connectivity: Connectivity::Eight,
            deblend: Deblend::LocalMaxima {
                min_relative_height: 0.3,
            },
            deblend_min_separation: 3,
            min_area: 5,
            max_area: 500,
            edge_margin: 10,
        }
    }
}

impl ExtractionConfig {
    pub(super) fn validate(&self) -> Result<(), InvalidConfigField> {
        ConfigField::new("sigma_threshold", self.sigma_threshold)
            .finite_and("finite and positive", |value| value > 0.0)?;
        ConfigField::count("deblend_min_separation", self.deblend_min_separation)
            .must_be("at least 1", self.deblend_min_separation >= 1)?;
        match self.deblend {
            Deblend::LocalMaxima {
                min_relative_height,
            } => {
                ConfigField::new("deblend min_relative_height", min_relative_height)
                    .finite_and("finite and in [0, 1]", |value| (0.0..=1.0).contains(&value))?;
            }
            Deblend::MultiThreshold {
                n_thresholds,
                min_contrast,
            } => {
                ConfigField::count("deblend n_thresholds", n_thresholds).must_be_against_count(
                    "between 2 and the deblend level cap",
                    MAX_DEBLEND_N_THRESHOLDS,
                    (2..=MAX_DEBLEND_N_THRESHOLDS).contains(&n_thresholds),
                )?;
                ConfigField::new("deblend min_contrast", min_contrast)
                    .finite_and("finite and in [0, 1]", |value| (0.0..=1.0).contains(&value))?;
            }
        }
        ConfigField::count("min_area", self.min_area).must_be("at least 1", self.min_area >= 1)?;
        ConfigField::count("max_area", self.max_area).must_be_against_count(
            "at least min_area",
            self.min_area,
            self.max_area >= self.min_area,
        )?;
        Ok(())
    }
}
