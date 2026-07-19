//! Configuration types for star detection.
//!
//! This module defines the composed [`Config`] and stage-specific configuration types used by
//! the star detection pipeline.

use crate::stacking::star_detection::error::StarDetectionConfigError;

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
    pub fn validate(&self) -> Result<(), StarDetectionConfigError> {
        if let CentroidMethod::MoffatFit { beta } = self
            && (!beta.is_finite() || *beta <= 0.0 || *beta > 10.0)
        {
            return Err(StarDetectionConfigError::InvalidMoffatBeta { value: *beta });
        }
        Ok(())
    }
}

/// Camera noise model for accurate SNR calculation.
#[derive(Debug, Clone, Copy)]
pub struct NoiseModel {
    /// Camera gain in electrons per ADU (e-/ADU).
    /// Typical values: 0.5-4.0 e-/ADU for modern CMOS sensors.
    pub gain: f32,
    /// Read noise in electrons (e-).
    /// Typical values: 1-10 e- for modern CMOS sensors.
    pub read_noise: f32,
}

impl NoiseModel {
    /// Create a new noise model.
    pub fn new(gain: f32, read_noise: f32) -> Self {
        Self { gain, read_noise }
    }

    /// Validate the noise model.
    pub fn validate(&self) -> Result<(), StarDetectionConfigError> {
        if !self.gain.is_finite() || self.gain <= 0.0 {
            return Err(StarDetectionConfigError::InvalidGain { value: self.gain });
        }
        if !self.read_noise.is_finite() || self.read_noise < 0.0 {
            return Err(StarDetectionConfigError::InvalidReadNoise {
                value: self.read_noise,
            });
        }
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
    pub fn validate(&self) -> Result<(), StarDetectionConfigError> {
        match self {
            Self::None => Ok(()),
            Self::Iterative { iterations: 0 } => {
                Err(StarDetectionConfigError::ZeroBackgroundRefinementIterations)
            }
            Self::Iterative { iterations } if *iterations > 10 => Err(
                StarDetectionConfigError::ExcessiveBackgroundRefinementIterations {
                    value: *iterations,
                },
            ),
            Self::Iterative { .. } => Ok(()),
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
    pub(crate) fn validate(&self) -> Result<(), StarDetectionConfigError> {
        if !(16..=256).contains(&self.tile_size) {
            return Err(StarDetectionConfigError::InvalidTileSize {
                value: self.tile_size,
            });
        }
        if self.sigma_clip_iterations > 10 {
            return Err(StarDetectionConfigError::ExcessiveSigmaClipIterations {
                value: self.sigma_clip_iterations,
            });
        }
        self.refinement.validate()?;
        if self.mask_dilation > 50 {
            return Err(StarDetectionConfigError::ExcessiveBackgroundMaskDilation {
                value: self.mask_dilation,
            });
        }
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
    fn validate(&self) -> Result<(), StarDetectionConfigError> {
        if !self.sigma_threshold.is_finite() || self.sigma_threshold <= 0.0 {
            return Err(StarDetectionConfigError::InvalidSigmaThreshold {
                value: self.sigma_threshold,
            });
        }
        if !self.psf_axis_ratio.is_finite()
            || self.psf_axis_ratio <= 0.0
            || self.psf_axis_ratio > 1.0
        {
            return Err(StarDetectionConfigError::InvalidPsfAxisRatio {
                value: self.psf_axis_ratio,
            });
        }
        if !self.psf_angle.is_finite() {
            return Err(StarDetectionConfigError::InvalidPsfAngle {
                value: self.psf_angle,
            });
        }
        if self.deblend_min_separation == 0 {
            return Err(StarDetectionConfigError::InvalidDeblendMinSeparation {
                value: self.deblend_min_separation,
            });
        }
        if !self.deblend_min_prominence.is_finite()
            || !(0.0..=1.0).contains(&self.deblend_min_prominence)
        {
            return Err(StarDetectionConfigError::InvalidDeblendMinProminence {
                value: self.deblend_min_prominence,
            });
        }
        if self.deblend_n_thresholds != 0
            && !(2..=MAX_DEBLEND_N_THRESHOLDS).contains(&self.deblend_n_thresholds)
        {
            return Err(StarDetectionConfigError::InvalidDeblendThresholdCount {
                value: self.deblend_n_thresholds,
                maximum: MAX_DEBLEND_N_THRESHOLDS,
            });
        }
        if !self.deblend_min_contrast.is_finite()
            || !(0.0..=1.0).contains(&self.deblend_min_contrast)
        {
            return Err(StarDetectionConfigError::InvalidDeblendMinContrast {
                value: self.deblend_min_contrast,
            });
        }
        if self.min_area == 0 {
            return Err(StarDetectionConfigError::ZeroMinArea);
        }
        if self.max_area < self.min_area {
            return Err(StarDetectionConfigError::MaxAreaBelowMin {
                min_area: self.min_area,
                max_area: self.max_area,
            });
        }
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
    fn validate(&self) -> Result<(), StarDetectionConfigError> {
        if !self.expected.is_finite() || self.expected < 0.0 {
            return Err(StarDetectionConfigError::InvalidExpectedFwhm {
                value: self.expected,
            });
        }
        if self.min_stars < 5 {
            return Err(StarDetectionConfigError::TooFewStarsForFwhm {
                value: self.min_stars,
            });
        }
        if !self.estimation_sigma_factor.is_finite() || self.estimation_sigma_factor < 1.0 {
            return Err(StarDetectionConfigError::InvalidFwhmEstimationSigmaFactor {
                value: self.estimation_sigma_factor,
            });
        }
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
    fn validate(&self) -> Result<(), StarDetectionConfigError> {
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
    fn validate(&self) -> Result<(), StarDetectionConfigError> {
        if !self.min_snr.is_finite() || self.min_snr <= 0.0 {
            return Err(StarDetectionConfigError::InvalidMinSnr {
                value: self.min_snr,
            });
        }
        if !self.max_eccentricity.is_finite() || !(0.0..=1.0).contains(&self.max_eccentricity) {
            return Err(StarDetectionConfigError::InvalidMaxEccentricity {
                value: self.max_eccentricity,
            });
        }
        if !self.max_sharpness.is_finite() || self.max_sharpness <= 0.0 || self.max_sharpness > 1.0
        {
            return Err(StarDetectionConfigError::InvalidMaxSharpness {
                value: self.max_sharpness,
            });
        }
        if !self.max_roundness.is_finite() || self.max_roundness <= 0.0 || self.max_roundness > 1.0
        {
            return Err(StarDetectionConfigError::InvalidMaxRoundness {
                value: self.max_roundness,
            });
        }
        if !self.max_fwhm_deviation.is_finite() || self.max_fwhm_deviation < 0.0 {
            return Err(StarDetectionConfigError::InvalidMaxFwhmDeviation {
                value: self.max_fwhm_deviation,
            });
        }
        if !self.duplicate_min_separation.is_finite() || self.duplicate_min_separation < 0.0 {
            return Err(StarDetectionConfigError::InvalidDuplicateMinSeparation {
                value: self.duplicate_min_separation,
            });
        }
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
    pub fn validate(&self) -> Result<(), StarDetectionConfigError> {
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
mod tests {
    use crate::stacking::star_detection::config::*;

    fn configured(update: impl FnOnce(&mut Config)) -> Config {
        let mut config = Config::default();
        update(&mut config);
        config
    }

    #[test]
    fn test_noise_model() {
        let model = NoiseModel::new(1.5, 5.0);
        assert!((model.gain - 1.5).abs() < 1e-6);
        assert!((model.read_noise - 5.0).abs() < 1e-6);
        assert_eq!(model.validate(), Ok(()));
    }

    #[test]
    fn test_noise_model_invalid_parameters_return_exact_errors() {
        let cases = [
            (
                NoiseModel::new(0.0, 5.0),
                StarDetectionConfigError::InvalidGain { value: 0.0 },
            ),
            (
                NoiseModel::new(f32::INFINITY, 5.0),
                StarDetectionConfigError::InvalidGain {
                    value: f32::INFINITY,
                },
            ),
            (
                NoiseModel::new(1.0, -1.0),
                StarDetectionConfigError::InvalidReadNoise { value: -1.0 },
            ),
            (
                NoiseModel::new(1.0, f32::INFINITY),
                StarDetectionConfigError::InvalidReadNoise {
                    value: f32::INFINITY,
                },
            ),
        ];
        for (model, expected) in cases {
            assert_eq!(model.validate(), Err(expected));
        }
    }

    #[test]
    fn test_centroid_method_validate() {
        assert_eq!(CentroidMethod::WeightedMoments.validate(), Ok(()));
        assert_eq!(CentroidMethod::GaussianFit.validate(), Ok(()));
        assert_eq!(CentroidMethod::MoffatFit { beta: 2.5 }.validate(), Ok(()));
    }

    #[test]
    fn test_centroid_method_invalid_beta_returns_exact_error() {
        for beta in [0.0, 15.0, f32::INFINITY] {
            assert_eq!(
                CentroidMethod::MoffatFit { beta }.validate(),
                Err(StarDetectionConfigError::InvalidMoffatBeta { value: beta })
            );
        }
    }

    #[test]
    fn test_config_default() {
        let config = Config::default();
        assert!(config.measurement.noise_model.is_none());
        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn test_config_presets() {
        assert_eq!(Config::wide_field().validate(), Ok(()));
        assert_eq!(Config::high_resolution().validate(), Ok(()));
        assert_eq!(Config::crowded_field().validate(), Ok(()));
        assert_eq!(Config::precise_ground().validate(), Ok(()));
    }

    #[test]
    fn test_config_custom() {
        let config = configured(|config| {
            config.fwhm.expected = 5.0;
            config.filter.min_snr = 15.0;
            config.detection.edge_margin = 20;
            config.measurement.noise_model = Some(NoiseModel::new(1.5, 5.0));
        });

        assert!((config.fwhm.expected - 5.0).abs() < 1e-6);
        assert!((config.filter.min_snr - 15.0).abs() < 1e-6);
        assert_eq!(config.detection.edge_margin, 20);
        assert!(config.measurement.noise_model.is_some());
        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn test_config_with_auto_fwhm() {
        let config = configured(|config| {
            config.fwhm.auto_estimate = true;
            config.fwhm.expected = 0.0;
        });
        assert!(config.fwhm.auto_estimate);
        assert!((config.fwhm.expected - 0.0).abs() < 1e-6);
        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn test_config_validates_centroid() {
        let config = configured(|config| {
            config.measurement.centroid_method = CentroidMethod::MoffatFit { beta: 2.5 };
        });
        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn test_config_invalid_parameters_return_exact_errors() {
        let cases = [
            (
                configured(|config| config.background.tile_size = 10),
                StarDetectionConfigError::InvalidTileSize { value: 10 },
            ),
            (
                configured(|config| config.background.sigma_clip_iterations = 11),
                StarDetectionConfigError::ExcessiveSigmaClipIterations { value: 11 },
            ),
            (
                configured(|config| {
                    config.background.refinement =
                        BackgroundRefinement::Iterative { iterations: 0 };
                }),
                StarDetectionConfigError::ZeroBackgroundRefinementIterations,
            ),
            (
                configured(|config| {
                    config.background.refinement =
                        BackgroundRefinement::Iterative { iterations: 11 };
                }),
                StarDetectionConfigError::ExcessiveBackgroundRefinementIterations { value: 11 },
            ),
            (
                configured(|config| config.background.mask_dilation = 51),
                StarDetectionConfigError::ExcessiveBackgroundMaskDilation { value: 51 },
            ),
            (
                configured(|config| config.detection.sigma_threshold = 0.0),
                StarDetectionConfigError::InvalidSigmaThreshold { value: 0.0 },
            ),
            (
                configured(|config| config.fwhm.expected = -1.0),
                StarDetectionConfigError::InvalidExpectedFwhm { value: -1.0 },
            ),
            (
                configured(|config| config.detection.psf_axis_ratio = 0.0),
                StarDetectionConfigError::InvalidPsfAxisRatio { value: 0.0 },
            ),
            (
                configured(|config| config.detection.psf_angle = f32::INFINITY),
                StarDetectionConfigError::InvalidPsfAngle {
                    value: f32::INFINITY,
                },
            ),
            (
                configured(|config| config.fwhm.min_stars = 4),
                StarDetectionConfigError::TooFewStarsForFwhm { value: 4 },
            ),
            (
                configured(|config| config.fwhm.estimation_sigma_factor = 0.5),
                StarDetectionConfigError::InvalidFwhmEstimationSigmaFactor { value: 0.5 },
            ),
            (
                configured(|config| config.detection.deblend_min_separation = 0),
                StarDetectionConfigError::InvalidDeblendMinSeparation { value: 0 },
            ),
            (
                configured(|config| config.detection.deblend_min_prominence = 1.5),
                StarDetectionConfigError::InvalidDeblendMinProminence { value: 1.5 },
            ),
            (
                configured(|config| config.detection.deblend_n_thresholds = 1),
                StarDetectionConfigError::InvalidDeblendThresholdCount {
                    value: 1,
                    maximum: MAX_DEBLEND_N_THRESHOLDS,
                },
            ),
            (
                configured(|config| {
                    config.detection.deblend_n_thresholds = MAX_DEBLEND_N_THRESHOLDS + 1;
                }),
                StarDetectionConfigError::InvalidDeblendThresholdCount {
                    value: MAX_DEBLEND_N_THRESHOLDS + 1,
                    maximum: MAX_DEBLEND_N_THRESHOLDS,
                },
            ),
            (
                configured(|config| config.detection.deblend_min_contrast = -0.1),
                StarDetectionConfigError::InvalidDeblendMinContrast { value: -0.1 },
            ),
            (
                configured(|config| config.detection.min_area = 0),
                StarDetectionConfigError::ZeroMinArea,
            ),
            (
                configured(|config| {
                    config.detection.min_area = 100;
                    config.detection.max_area = 50;
                }),
                StarDetectionConfigError::MaxAreaBelowMin {
                    min_area: 100,
                    max_area: 50,
                },
            ),
            (
                configured(|config| {
                    config.measurement.centroid_method = CentroidMethod::MoffatFit { beta: 0.0 };
                }),
                StarDetectionConfigError::InvalidMoffatBeta { value: 0.0 },
            ),
            (
                configured(|config| config.filter.min_snr = 0.0),
                StarDetectionConfigError::InvalidMinSnr { value: 0.0 },
            ),
            (
                configured(|config| config.filter.max_eccentricity = 1.5),
                StarDetectionConfigError::InvalidMaxEccentricity { value: 1.5 },
            ),
            (
                configured(|config| config.filter.max_sharpness = 0.0),
                StarDetectionConfigError::InvalidMaxSharpness { value: 0.0 },
            ),
            (
                configured(|config| config.filter.max_roundness = 0.0),
                StarDetectionConfigError::InvalidMaxRoundness { value: 0.0 },
            ),
            (
                configured(|config| config.filter.max_fwhm_deviation = -1.0),
                StarDetectionConfigError::InvalidMaxFwhmDeviation { value: -1.0 },
            ),
            (
                configured(|config| config.filter.duplicate_min_separation = -1.0),
                StarDetectionConfigError::InvalidDuplicateMinSeparation { value: -1.0 },
            ),
            (
                configured(|config| {
                    config.measurement.noise_model = Some(NoiseModel::new(0.0, 1.0));
                }),
                StarDetectionConfigError::InvalidGain { value: 0.0 },
            ),
            (
                configured(|config| {
                    config.measurement.noise_model = Some(NoiseModel::new(1.0, -1.0));
                }),
                StarDetectionConfigError::InvalidReadNoise { value: -1.0 },
            ),
        ];

        for (config, expected) in cases {
            assert_eq!(config.validate(), Err(expected));
        }
    }

    #[test]
    fn test_config_deblend_n_thresholds_at_max_accepted() {
        assert_eq!(
            configured(|config| {
                config.detection.deblend_n_thresholds = MAX_DEBLEND_N_THRESHOLDS;
            })
            .validate(),
            Ok(())
        );
    }

    #[test]
    fn test_config_deblend_multi_threshold() {
        let config = configured(|config| config.detection.deblend_n_thresholds = 32);
        assert!(config.detection.is_multi_threshold());
        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn test_config_wide_field_values() {
        let config = Config::wide_field();
        assert!((config.fwhm.expected - 6.0).abs() < 1e-6);
        assert!(config.fwhm.auto_estimate);
        assert_eq!(config.detection.min_area, 7);
        assert_eq!(config.detection.max_area, 1500);
        assert_eq!(config.detection.edge_margin, 20);
        assert!((config.filter.max_eccentricity - 0.7).abs() < 1e-6);
        assert_eq!(config.detection.connectivity, Connectivity::Eight);
    }

    #[test]
    fn test_config_precise_ground_values() {
        let config = Config::precise_ground();
        assert!(matches!(
            config.measurement.centroid_method,
            CentroidMethod::MoffatFit { beta } if (beta - 2.5).abs() < 1e-6
        ));
        assert_eq!(
            config.measurement.local_background,
            LocalBackgroundMethod::LocalAnnulus
        );
        assert_eq!(config.detection.deblend_n_thresholds, 32);
        assert!((config.filter.min_snr - 15.0).abs() < 1e-6);
        assert_eq!(config.background.tile_size, 128);
        assert!((config.detection.sigma_threshold - 3.0).abs() < 1e-6);
        assert!(config.fwhm.auto_estimate);
        assert_eq!(config.fwhm.min_stars, 30);
    }

    #[test]
    fn test_config_high_resolution_values() {
        let config = Config::high_resolution();
        assert!((config.fwhm.expected - 2.5).abs() < 1e-6);
        assert!(config.fwhm.auto_estimate);
        assert_eq!(config.detection.min_area, 3);
        assert_eq!(config.detection.max_area, 200);
        assert!((config.filter.min_snr - 15.0).abs() < 1e-6);
        assert!((config.filter.max_eccentricity - 0.5).abs() < 1e-6);
        assert!((config.filter.max_roundness - 0.3).abs() < 1e-6);
        assert!(matches!(
            config.measurement.centroid_method,
            CentroidMethod::GaussianFit
        ));
    }

    #[test]
    fn test_config_crowded_field_values() {
        let config = Config::crowded_field();
        assert_eq!(config.detection.deblend_n_thresholds, 32);
        assert_eq!(config.detection.deblend_min_separation, 2);
        assert!((config.detection.deblend_min_prominence - 0.15).abs() < 1e-6);
        assert!((config.detection.deblend_min_contrast - 0.005).abs() < 1e-6);
        assert!(matches!(
            config.background.refinement,
            BackgroundRefinement::Iterative { iterations: 2 }
        ));
        assert!((config.filter.duplicate_min_separation - 3.0).abs() < 1e-6);
        assert!(config.fwhm.auto_estimate);
    }

    #[test]
    fn test_config_rejects_non_finite_float_parameters() {
        let cases = [
            (
                configured(|config| config.detection.sigma_threshold = f32::INFINITY),
                StarDetectionConfigError::InvalidSigmaThreshold {
                    value: f32::INFINITY,
                },
            ),
            (
                configured(|config| config.fwhm.expected = f32::INFINITY),
                StarDetectionConfigError::InvalidExpectedFwhm {
                    value: f32::INFINITY,
                },
            ),
            (
                configured(|config| config.detection.psf_axis_ratio = f32::INFINITY),
                StarDetectionConfigError::InvalidPsfAxisRatio {
                    value: f32::INFINITY,
                },
            ),
            (
                configured(|config| config.fwhm.estimation_sigma_factor = f32::INFINITY),
                StarDetectionConfigError::InvalidFwhmEstimationSigmaFactor {
                    value: f32::INFINITY,
                },
            ),
            (
                configured(|config| {
                    config.detection.deblend_min_prominence = f32::INFINITY;
                }),
                StarDetectionConfigError::InvalidDeblendMinProminence {
                    value: f32::INFINITY,
                },
            ),
            (
                configured(|config| config.detection.deblend_min_contrast = f32::INFINITY),
                StarDetectionConfigError::InvalidDeblendMinContrast {
                    value: f32::INFINITY,
                },
            ),
            (
                configured(|config| config.filter.min_snr = f32::INFINITY),
                StarDetectionConfigError::InvalidMinSnr {
                    value: f32::INFINITY,
                },
            ),
            (
                configured(|config| config.filter.max_eccentricity = f32::INFINITY),
                StarDetectionConfigError::InvalidMaxEccentricity {
                    value: f32::INFINITY,
                },
            ),
            (
                configured(|config| config.filter.max_sharpness = f32::INFINITY),
                StarDetectionConfigError::InvalidMaxSharpness {
                    value: f32::INFINITY,
                },
            ),
            (
                configured(|config| config.filter.max_roundness = f32::INFINITY),
                StarDetectionConfigError::InvalidMaxRoundness {
                    value: f32::INFINITY,
                },
            ),
            (
                configured(|config| config.filter.max_fwhm_deviation = f32::INFINITY),
                StarDetectionConfigError::InvalidMaxFwhmDeviation {
                    value: f32::INFINITY,
                },
            ),
            (
                configured(|config| config.filter.duplicate_min_separation = f32::INFINITY),
                StarDetectionConfigError::InvalidDuplicateMinSeparation {
                    value: f32::INFINITY,
                },
            ),
        ];

        for (config, expected) in cases {
            assert_eq!(config.validate(), Err(expected));
        }
    }

    #[test]
    fn test_background_refinement_iterations() {
        assert_eq!(BackgroundRefinement::None.iterations(), 0);
        assert_eq!(
            BackgroundRefinement::Iterative { iterations: 3 }.iterations(),
            3
        );
        assert_eq!(BackgroundRefinement::None.validate(), Ok(()));
        assert_eq!(
            BackgroundRefinement::Iterative { iterations: 3 }.validate(),
            Ok(())
        );
    }

    #[test]
    fn test_background_refinement_invalid_iterations_return_exact_errors() {
        assert_eq!(
            BackgroundRefinement::Iterative { iterations: 0 }.validate(),
            Err(StarDetectionConfigError::ZeroBackgroundRefinementIterations)
        );
        assert_eq!(
            BackgroundRefinement::Iterative { iterations: 11 }.validate(),
            Err(StarDetectionConfigError::ExcessiveBackgroundRefinementIterations { value: 11 })
        );
    }

    #[test]
    fn test_is_multi_threshold() {
        // 0 = disabled → false
        let config = configured(|config| config.detection.deblend_n_thresholds = 0);
        assert!(!config.detection.is_multi_threshold());

        // >= 2 → true
        let config = configured(|config| config.detection.deblend_n_thresholds = 2);
        assert!(config.detection.is_multi_threshold());
    }
}
