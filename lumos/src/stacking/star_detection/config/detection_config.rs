//! Candidate detection, deblending, and region-filtering settings.

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
    /// Every residual peak at least `min_prominence` of the component's brightest, and at least
    /// `deblend_min_separation` from a brighter one, is its own star.
    LocalMaxima {
        /// A peak's share of the component's brightest residual, in `[0, 1]`.
        min_prominence: f32,
    },
    /// SExtractor's tree: the component is thresholded at `n_thresholds` levels spaced
    /// exponentially between the detection threshold and its peak residual, and a branch is its
    /// own star when it holds at least `min_contrast` of the component's residual flux above
    /// that threshold.
    MultiThreshold {
        /// Levels in the tree, between 2 and [`MAX_DEBLEND_N_THRESHOLDS`].
        n_thresholds: usize,
        /// A branch's share of the component's residual flux, in `[0, 1]`.
        min_contrast: f32,
    },
}

/// Upper bound for [`Deblend::MultiThreshold`]'s `n_thresholds`.
///
/// The multi-threshold deblend tree's max depth is `n_thresholds + 1`
/// (`build_deblend_tree`'s level loop), and `collect_significant_leaves` recurses
/// along that depth with no independent cutoff — an unbounded `n_thresholds` risks
/// stack overflow on a component with enough real structure to keep splitting.
/// 256 levels is already far beyond the documented useful range ("32+ = SExtractor-style").
pub(super) const MAX_DEBLEND_N_THRESHOLDS: usize = 256;

/// Configuration for candidate detection, deblending, and region filtering.
#[derive(Debug, Clone)]
pub struct DetectionConfig {
    /// Detection threshold in local background-noise standard deviations.
    pub sigma_threshold: f32,
    /// Pixel connectivity used to form candidate regions.
    pub connectivity: Connectivity,
    /// Minor-to-major axis ratio for the matched-filter PSF.
    pub psf_axis_ratio: f32,
    /// Matched-filter PSF angle in radians.
    pub psf_angle: f32,
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

impl Default for DetectionConfig {
    fn default() -> Self {
        Self {
            sigma_threshold: 4.0,
            connectivity: Connectivity::Eight,
            psf_axis_ratio: 1.0,
            psf_angle: 0.0,
            deblend: Deblend::LocalMaxima {
                min_prominence: 0.3,
            },
            deblend_min_separation: 3,
            min_area: 5,
            max_area: 500,
            edge_margin: 10,
        }
    }
}

impl DetectionConfig {
    pub(super) fn validate(&self) -> Result<(), InvalidConfigField> {
        InvalidConfigField::finite(
            "sigma_threshold",
            "finite and positive",
            self.sigma_threshold,
            |value| value > 0.0,
        )?;
        InvalidConfigField::finite(
            "psf_axis_ratio",
            "finite and in (0, 1]",
            self.psf_axis_ratio,
            |value| value > 0.0 && value <= 1.0,
        )?;
        InvalidConfigField::finite_only("psf_angle", self.psf_angle)?;
        InvalidConfigField::check(
            self.deblend_min_separation >= 1,
            "deblend_min_separation",
            "at least 1",
            self.deblend_min_separation as f64,
        )?;
        match self.deblend {
            Deblend::LocalMaxima { min_prominence } => InvalidConfigField::finite(
                "deblend min_prominence",
                "finite and in [0, 1]",
                min_prominence,
                |value| (0.0..=1.0).contains(&value),
            )?,
            Deblend::MultiThreshold {
                n_thresholds,
                min_contrast,
            } => {
                InvalidConfigField::check_against(
                    (2..=MAX_DEBLEND_N_THRESHOLDS).contains(&n_thresholds),
                    "deblend n_thresholds",
                    "between 2 and the deblend level cap",
                    n_thresholds as f64,
                    MAX_DEBLEND_N_THRESHOLDS as f64,
                )?;
                InvalidConfigField::finite(
                    "deblend min_contrast",
                    "finite and in [0, 1]",
                    min_contrast,
                    |value| (0.0..=1.0).contains(&value),
                )?;
            }
        }
        InvalidConfigField::check(
            self.min_area >= 1,
            "min_area",
            "at least 1",
            self.min_area as f64,
        )?;
        InvalidConfigField::check_against(
            self.max_area >= self.min_area,
            "max_area",
            "at least min_area",
            self.max_area as f64,
            self.min_area as f64,
        )?;
        Ok(())
    }
}
