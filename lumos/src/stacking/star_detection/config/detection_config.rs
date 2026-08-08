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

/// Upper bound for `Config::deblend_n_thresholds`.
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
    /// Minimum separation between deblended peaks in pixels.
    pub deblend_min_separation: usize,
    /// Minimum local prominence for local-maxima deblending.
    pub deblend_min_prominence: f32,
    /// Number of multi-threshold deblending levels; zero selects local maxima.
    pub deblend_n_thresholds: usize,
    /// Minimum branch-to-component flux contrast for multi-threshold deblending.
    pub deblend_min_contrast: f32,
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
            deblend_min_separation: 3,
            deblend_min_prominence: 0.3,
            deblend_n_thresholds: 0,
            deblend_min_contrast: 0.005,
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
        InvalidConfigField::finite(
            "deblend_min_prominence",
            "finite and in [0, 1]",
            self.deblend_min_prominence,
            |value| (0.0..=1.0).contains(&value),
        )?;
        InvalidConfigField::check_against(
            self.deblend_n_thresholds == 0
                || (2..=MAX_DEBLEND_N_THRESHOLDS).contains(&self.deblend_n_thresholds),
            "deblend_n_thresholds",
            "0, or between 2 and the deblend level cap",
            self.deblend_n_thresholds as f64,
            MAX_DEBLEND_N_THRESHOLDS as f64,
        )?;
        InvalidConfigField::finite(
            "deblend_min_contrast",
            "finite and in [0, 1]",
            self.deblend_min_contrast,
            |value| (0.0..=1.0).contains(&value),
        )?;
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

    #[inline]
    pub const fn is_multi_threshold(&self) -> bool {
        self.deblend_n_thresholds > 0
    }
}
