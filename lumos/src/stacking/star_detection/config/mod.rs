//! Configuration types for star detection.
//!
//! This module defines the composed [`Config`] and stage-specific configuration types used by
//! the star detection pipeline.

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

/// Method for computing local background during centroid refinement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LocalBackgroundMethod {
    /// Use the global background map (default, fastest).
    #[default]
    GlobalMap,
    /// Compute local background using an annular region around the star.
    /// Inner radius is based on stamp_radius, outer radius is 1.5× that.
    /// More accurate in regions with variable nebulosity.
    LocalAnnulus,
}

/// Method for computing sub-pixel centroids.
///
/// Different methods offer tradeoffs between accuracy and speed.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum CentroidMethod {
    /// Iterative weighted centroid using Gaussian weights.
    /// Fast (~0.05 pixel accuracy). This is the default.
    #[default]
    WeightedMoments,

    /// 2D Gaussian profile fitting via Levenberg-Marquardt optimization.
    /// High precision (~0.01 pixel accuracy) but ~8x slower than WeightedMoments.
    /// Best for well-sampled, symmetric PSFs.
    GaussianFit,

    /// 2D Moffat profile fitting with configurable beta parameter.
    /// High precision (~0.01 pixel accuracy), similar speed to GaussianFit.
    /// Better model for atmospheric seeing (extended wings).
    /// Beta parameter controls wing slope: 2.5 typical for ground-based, 4.5 for space-based.
    MoffatFit {
        /// Power law slope controlling wing falloff. Typical range: 2.0-5.0.
        /// Lower values = more extended wings.
        beta: f32,
    },
}

impl CentroidMethod {
    /// Validate the centroid method configuration.
    pub fn validate(&self) -> Result<(), InvalidConfigField> {
        if let CentroidMethod::MoffatFit { beta } = self {
            InvalidConfigField::finite("Moffat beta", "finite and in (0, 10]", *beta, |value| {
                value > 0.0 && value <= 10.0
            })?;
        }
        Ok(())
    }
}

/// Sensor noise model for normalized linear pixels.
///
/// Lumos stores pixels in normalized units, so the conversion factor is electrons represented by
/// a pixel value of `1.0`, not the camera's electrons-per-ADU gain. Convert a physical gain with
/// `electrons_per_normalized_unit = electrons_per_adu * adu_per_normalized_unit`.
#[derive(Debug, Clone, Copy)]
pub struct NoiseModel {
    /// Electrons represented by one normalized pixel unit.
    pub electrons_per_normalized_unit: f32,
    /// Per-pixel read-noise standard deviation in electrons.
    pub read_noise_electrons: f32,
}

impl NoiseModel {
    /// Create a noise model whose signal scale already matches normalized Lumos pixels.
    pub fn from_normalized(electrons_per_normalized_unit: f32, read_noise_electrons: f32) -> Self {
        Self {
            electrons_per_normalized_unit,
            read_noise_electrons,
        }
    }

    /// Variance of an integrated normalized signal.
    ///
    /// `signal` is the summed background-subtracted signal, `background_noise` is the empirical
    /// per-pixel background standard deviation, and `sample_count` is the number of summed pixels.
    pub(crate) fn variance_normalized(
        &self,
        signal: f64,
        background_noise: f64,
        sample_count: usize,
    ) -> f64 {
        debug_assert!(signal.is_finite() && signal >= 0.0);
        debug_assert!(background_noise.is_finite() && background_noise >= 0.0);

        let electrons_per_unit = self.electrons_per_normalized_unit as f64;
        let read_noise_normalized = self.read_noise_electrons as f64 / electrons_per_unit;
        signal / electrons_per_unit
            + sample_count as f64
                * (background_noise * background_noise
                    + read_noise_normalized * read_noise_normalized)
    }

    /// Validate the noise model.
    pub fn validate(&self) -> Result<(), InvalidConfigField> {
        InvalidConfigField::finite(
            "electrons_per_normalized_unit",
            "finite and positive",
            self.electrons_per_normalized_unit,
            |value| value > 0.0,
        )?;
        InvalidConfigField::finite(
            "read_noise_electrons",
            "finite and non-negative",
            self.read_noise_electrons,
            |value| value >= 0.0,
        )?;
        Ok(())
    }
}

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
    pub fn validate(&self) -> Result<(), InvalidConfigField> {
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

/// Upper bound for `Config::deblend_n_thresholds`.
///
/// The multi-threshold deblend tree's max depth is `n_thresholds + 1`
/// (`build_deblend_tree`'s level loop), and `collect_significant_leaves` recurses
/// along that depth with no independent cutoff — an unbounded `n_thresholds` risks
/// stack overflow on a component with enough real structure to keep splitting.
/// 256 levels is already far beyond the documented useful range ("32+ = SExtractor-style").
const MAX_DEBLEND_N_THRESHOLDS: usize = 256;

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
    pub(super) fn validate(&self) -> Result<(), InvalidConfigField> {
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
    fn validate(&self) -> Result<(), InvalidConfigField> {
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

/// Configuration for selecting or estimating the matched-filter FWHM.
#[derive(Debug, Clone)]
pub struct FwhmConfig {
    /// Fixed matched-filter FWHM, or the fallback for auto-estimation; zero disables it.
    pub expected: f32,
    /// Whether to estimate FWHM from a first-pass star catalog.
    pub auto_estimate: bool,
    /// Minimum first-pass stars required to accept an estimate.
    pub min_stars: usize,
    /// Multiplier applied to the detection threshold during the first pass.
    pub estimation_sigma_factor: f32,
}

impl Default for FwhmConfig {
    fn default() -> Self {
        Self {
            expected: 4.0,
            auto_estimate: false,
            min_stars: 10,
            estimation_sigma_factor: 2.0,
        }
    }
}

impl FwhmConfig {
    fn validate(&self) -> Result<(), InvalidConfigField> {
        InvalidConfigField::finite(
            "expected_fwhm",
            "finite and non-negative",
            self.expected,
            |value| value >= 0.0,
        )?;
        InvalidConfigField::check(
            self.min_stars >= 5,
            "min_stars_for_fwhm",
            "at least 5",
            self.min_stars as f64,
        )?;
        InvalidConfigField::finite(
            "fwhm_estimation_sigma_factor",
            "finite and at least 1",
            self.estimation_sigma_factor,
            |value| value >= 1.0,
        )?;
        Ok(())
    }
}

/// Configuration for centroid refinement and metric measurement.
#[derive(Debug, Clone)]
pub struct MeasurementConfig {
    /// Centroid refinement algorithm.
    pub centroid_method: CentroidMethod,
    /// Background source used for per-star measurement.
    pub local_background: LocalBackgroundMethod,
    /// Optional sensor model for variance-weighted fitting and SNR.
    pub noise_model: Option<NoiseModel>,
}

impl Default for MeasurementConfig {
    fn default() -> Self {
        Self {
            centroid_method: CentroidMethod::WeightedMoments,
            local_background: LocalBackgroundMethod::GlobalMap,
            noise_model: None,
        }
    }
}

impl MeasurementConfig {
    fn validate(&self) -> Result<(), InvalidConfigField> {
        self.centroid_method.validate()?;
        if let Some(noise) = &self.noise_model {
            noise.validate()?;
        }
        Ok(())
    }
}

/// Configuration for final star-quality filtering and duplicate removal.
#[derive(Debug, Clone)]
pub struct FilterConfig {
    /// Minimum accepted signal-to-noise ratio.
    pub min_snr: f32,
    /// Maximum accepted eccentricity.
    pub max_eccentricity: f32,
    /// Maximum accepted sharpness.
    pub max_sharpness: f32,
    /// Maximum accepted absolute roundness.
    pub max_roundness: f32,
    /// Maximum robust FWHM deviation in MAD-scaled units.
    pub max_fwhm_deviation: f32,
    /// Minimum retained separation between duplicate stars in pixels.
    pub duplicate_min_separation: f32,
}

impl Default for FilterConfig {
    fn default() -> Self {
        Self {
            min_snr: 10.0,
            max_eccentricity: 0.6,
            max_sharpness: 0.7,
            max_roundness: 0.5,
            max_fwhm_deviation: 3.0,
            duplicate_min_separation: 8.0,
        }
    }
}

impl FilterConfig {
    fn validate(&self) -> Result<(), InvalidConfigField> {
        InvalidConfigField::finite("min_snr", "finite and positive", self.min_snr, |value| {
            value > 0.0
        })?;
        InvalidConfigField::finite(
            "max_eccentricity",
            "finite and in [0, 1]",
            self.max_eccentricity,
            |value| (0.0..=1.0).contains(&value),
        )?;
        InvalidConfigField::finite(
            "max_sharpness",
            "finite and in (0, 1]",
            self.max_sharpness,
            |value| value > 0.0 && value <= 1.0,
        )?;
        InvalidConfigField::finite(
            "max_roundness",
            "finite and in (0, 1]",
            self.max_roundness,
            |value| value > 0.0 && value <= 1.0,
        )?;
        InvalidConfigField::finite(
            "max_fwhm_deviation",
            "finite and non-negative",
            self.max_fwhm_deviation,
            |value| value >= 0.0,
        )?;
        InvalidConfigField::finite(
            "duplicate_min_separation",
            "finite and non-negative",
            self.duplicate_min_separation,
            |value| value >= 0.0,
        )?;
        Ok(())
    }
}

/// Configuration for the star detection pipeline, composed by processing stage.
///
/// # Example
///
/// ```rust,ignore
/// use lumos::StarDetectionConfig;
///
/// // Use a preset
/// let config = StarDetectionConfig::wide_field();
///
/// // Customize from a preset
/// let mut config = StarDetectionConfig::crowded_field();
/// config.filter.min_snr = 20.0;
/// ```
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Background estimation and refinement settings.
    pub background: BackgroundConfig,
    /// Candidate detection, deblending, and region-filtering settings.
    pub detection: DetectionConfig,
    /// Matched-filter FWHM selection and estimation settings.
    pub fwhm: FwhmConfig,
    /// Centroid and metric measurement settings.
    pub measurement: MeasurementConfig,
    /// Final quality and duplicate-filtering settings.
    pub filter: FilterConfig,
}

impl Config {
    /// Validate every parameter before constructing a detector.
    pub fn validate(&self) -> Result<(), InvalidConfigField> {
        self.background.validate()?;
        self.detection.validate()?;
        self.fwhm.validate()?;
        self.measurement.validate()?;
        self.filter.validate()?;
        Ok(())
    }

    /// Wide-field imaging settings (short focal length, large pixel scale).
    ///
    /// Wide-field setups produce larger stars (FWHM 5-8px) that may be slightly
    /// elongated at field edges due to coma and field curvature. Uses relaxed
    /// eccentricity filtering, auto FWHM estimation, and 8-connectivity for
    /// undersampled PSFs that may not connect well with 4-connectivity.
    pub fn wide_field() -> Self {
        Self {
            fwhm: FwhmConfig {
                expected: 6.0,
                auto_estimate: true,
                min_stars: 15,
                ..Default::default()
            },
            detection: DetectionConfig {
                min_area: 7,
                max_area: 1500,
                edge_margin: 20,
                connectivity: Connectivity::Eight,
                ..Default::default()
            },
            filter: FilterConfig {
                max_eccentricity: 0.7,
                ..Default::default()
            },
            ..Self::default()
        }
    }

    /// High-resolution imaging settings (long focal length, small pixel scale).
    ///
    /// Well-sampled Nyquist PSFs (FWHM 2-4px) with symmetric profiles. Uses
    /// Gaussian centroid fitting for maximum precision on well-sampled stars,
    /// stricter eccentricity and roundness filtering, and higher SNR threshold
    /// to build a clean, high-quality star catalog.
    pub fn high_resolution() -> Self {
        Self {
            fwhm: FwhmConfig {
                expected: 2.5,
                auto_estimate: true,
                min_stars: 15,
                ..Default::default()
            },
            detection: DetectionConfig {
                min_area: 3,
                max_area: 200,
                ..Default::default()
            },
            measurement: MeasurementConfig {
                centroid_method: CentroidMethod::GaussianFit,
                ..Default::default()
            },
            filter: FilterConfig {
                min_snr: 15.0,
                max_eccentricity: 0.5,
                max_roundness: 0.3,
                ..Default::default()
            },
            ..Self::default()
        }
    }

    /// Crowded field settings (globular clusters, dense star fields).
    ///
    /// Enables SExtractor-style multi-threshold deblending (32 sub-thresholds)
    /// with low contrast threshold to separate close blends. Uses iterative
    /// background refinement to re-estimate background after masking sources.
    pub fn crowded_field() -> Self {
        Self {
            background: BackgroundConfig {
                refinement: BackgroundRefinement::Iterative { iterations: 2 },
                ..Default::default()
            },
            detection: DetectionConfig {
                deblend_n_thresholds: 32,
                deblend_min_separation: 2,
                deblend_min_prominence: 0.15,
                deblend_min_contrast: 0.005,
                connectivity: Connectivity::Eight,
                ..Default::default()
            },
            fwhm: FwhmConfig {
                auto_estimate: true,
                ..Default::default()
            },
            filter: FilterConfig {
                duplicate_min_separation: 3.0,
                ..Default::default()
            },
            ..Self::default()
        }
    }

    /// Maximum centroid precision settings for ground-based astrophotography.
    ///
    /// Optimized for sub-pixel astrometric accuracy. Uses Moffat PSF fitting
    /// (beta=2.5) which models atmospheric seeing wings better than Gaussian.
    /// Local annulus background subtraction handles nebulosity near stars.
    pub fn precise_ground() -> Self {
        Self {
            background: BackgroundConfig {
                mask_dilation: 5,
                tile_size: 128,
                sigma_clip_iterations: 3,
                refinement: BackgroundRefinement::Iterative { iterations: 3 },
            },
            detection: DetectionConfig {
                sigma_threshold: 3.0,
                min_area: 7,
                max_area: 2000,
                edge_margin: 15,
                connectivity: Connectivity::Eight,
                deblend_min_separation: 2,
                deblend_min_prominence: 0.15,
                deblend_n_thresholds: 32,
                deblend_min_contrast: 0.003,
                ..Default::default()
            },
            fwhm: FwhmConfig {
                expected: 3.0,
                auto_estimate: true,
                min_stars: 30,
                estimation_sigma_factor: 2.5,
            },
            measurement: MeasurementConfig {
                centroid_method: CentroidMethod::MoffatFit { beta: 2.5 },
                local_background: LocalBackgroundMethod::LocalAnnulus,
                ..Default::default()
            },
            filter: FilterConfig {
                min_snr: 15.0,
                max_fwhm_deviation: 4.0,
                duplicate_min_separation: 5.0,
                ..Default::default()
            },
        }
    }
}

#[cfg(test)]
mod tests;
