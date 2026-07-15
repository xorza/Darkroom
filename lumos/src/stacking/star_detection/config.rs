//! Configuration types for star detection.
//!
//! This module defines the flat [`Config`] struct and associated enums used by
//! the star detection pipeline. All parameters are grouped by comments into
//! logical sections.

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

/// Configuration for the star detection pipeline.
///
/// Single flat struct with all parameters grouped by pipeline stage.
/// Use constructor presets for common scenarios, then customize individual
/// fields as needed.
///
/// # Example
///
/// ```rust,ignore
/// use lumos::star_detection::Config;
///
/// // Use a preset
/// let config = Config::wide_field();
///
/// // Customize from a preset
/// let mut config = Config::crowded_field();
/// config.min_snr = 20.0;
/// ```
#[derive(Debug, Clone)]
pub struct Config {
    // -- Background estimation --
    /// Tile size for background estimation in pixels. Range: 16-256.
    pub tile_size: usize,
    /// Number of sigma-clipping iterations for tile statistics.
    pub sigma_clip_iterations: usize,
    /// Background refinement strategy.
    pub refinement: BackgroundRefinement,
    /// Dilation radius for background refinement object masks (pixels).
    pub bg_mask_dilation: usize,

    // -- Detection --
    /// Detection threshold in sigma above background.
    pub sigma_threshold: f32,
    /// Pixel connectivity for connected component labeling.
    pub connectivity: Connectivity,

    // -- PSF / matched filter --
    /// Expected FWHM of stars in pixels. 0 = no matched filter.
    pub expected_fwhm: f32,
    /// Enable automatic FWHM estimation from bright stars.
    pub auto_estimate_fwhm: bool,
    /// Minimum number of stars required for valid FWHM estimation.
    pub min_stars_for_fwhm: usize,
    /// Sigma threshold multiplier for first-pass bright star detection.
    pub fwhm_estimation_sigma_factor: f32,
    /// PSF axis ratio (minor/major). 1.0 = circular.
    pub psf_axis_ratio: f32,
    /// PSF position angle in radians.
    pub psf_angle: f32,

    // -- Deblending --
    /// Minimum separation between peaks for deblending (pixels).
    pub deblend_min_separation: usize,
    /// Minimum peak prominence as fraction of primary peak.
    pub deblend_min_prominence: f32,
    /// Number of sub-thresholds for multi-threshold deblending.
    /// 0 = local maxima only, 32+ = SExtractor-style.
    pub deblend_n_thresholds: usize,
    /// Minimum contrast for multi-threshold deblending.
    pub deblend_min_contrast: f32,

    // -- Region filtering (applied during detection) --
    /// Minimum star area in pixels.
    pub min_area: usize,
    /// Maximum star area in pixels.
    pub max_area: usize,
    /// Edge margin in pixels (stars too close to edge are rejected).
    pub edge_margin: usize,

    // -- Centroid --
    /// Method for computing sub-pixel centroids.
    pub centroid_method: CentroidMethod,
    /// Method for computing local background during centroid refinement.
    pub local_background: LocalBackgroundMethod,

    // -- Star quality filtering --
    /// Minimum SNR for a star to be considered valid.
    pub min_snr: f32,
    /// Maximum eccentricity (0-1, higher = more elongated allowed).
    pub max_eccentricity: f32,
    /// Maximum sharpness. Cosmic rays have sharpness > 0.7.
    pub max_sharpness: f32,
    /// Maximum roundness for shape filtering.
    pub max_roundness: f32,
    /// Maximum FWHM deviation from median in MAD units. 0 = disabled.
    pub max_fwhm_deviation: f32,
    /// Minimum separation between stars for duplicate removal (pixels).
    pub duplicate_min_separation: f32,

    // -- Noise model --
    /// Optional camera noise model for accurate SNR calculation.
    pub noise_model: Option<NoiseModel>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            // Background estimation
            tile_size: 64,
            sigma_clip_iterations: 3,
            refinement: BackgroundRefinement::None,
            bg_mask_dilation: 3,

            // Detection
            sigma_threshold: 4.0,
            connectivity: Connectivity::Eight,

            // PSF / matched filter
            expected_fwhm: 4.0,
            auto_estimate_fwhm: false,
            min_stars_for_fwhm: 10,
            fwhm_estimation_sigma_factor: 2.0,
            psf_axis_ratio: 1.0,
            psf_angle: 0.0,

            // Deblending
            deblend_min_separation: 3,
            deblend_min_prominence: 0.3,
            deblend_n_thresholds: 0,
            deblend_min_contrast: 0.005,

            // Region filtering
            min_area: 5,
            max_area: 500,
            edge_margin: 10,

            // Centroid
            centroid_method: CentroidMethod::WeightedMoments,
            local_background: LocalBackgroundMethod::GlobalMap,

            // Star quality filtering
            min_snr: 10.0,
            max_eccentricity: 0.6,
            max_sharpness: 0.7,
            max_roundness: 0.5,
            max_fwhm_deviation: 3.0,
            duplicate_min_separation: 8.0,

            // Noise model
            noise_model: None,
        }
    }
}

impl Config {
    /// Validate every parameter before constructing a detector.
    pub fn validate(&self) -> Result<(), StarDetectionConfigError> {
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
        if self.bg_mask_dilation > 50 {
            return Err(StarDetectionConfigError::ExcessiveBackgroundMaskDilation {
                value: self.bg_mask_dilation,
            });
        }
        if !self.sigma_threshold.is_finite() || self.sigma_threshold <= 0.0 {
            return Err(StarDetectionConfigError::InvalidSigmaThreshold {
                value: self.sigma_threshold,
            });
        }
        if !self.expected_fwhm.is_finite() || self.expected_fwhm < 0.0 {
            return Err(StarDetectionConfigError::InvalidExpectedFwhm {
                value: self.expected_fwhm,
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
        if self.min_stars_for_fwhm < 5 {
            return Err(StarDetectionConfigError::TooFewStarsForFwhm {
                value: self.min_stars_for_fwhm,
            });
        }
        if !self.fwhm_estimation_sigma_factor.is_finite() || self.fwhm_estimation_sigma_factor < 1.0
        {
            return Err(StarDetectionConfigError::InvalidFwhmEstimationSigmaFactor {
                value: self.fwhm_estimation_sigma_factor,
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
        self.centroid_method.validate()?;
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
        if let Some(noise) = &self.noise_model {
            noise.validate()?;
        }
        Ok(())
    }

    // =========================================================================
    // Preset Constructors
    // =========================================================================

    /// Wide-field imaging settings (short focal length, large pixel scale).
    ///
    /// Wide-field setups produce larger stars (FWHM 5-8px) that may be slightly
    /// elongated at field edges due to coma and field curvature. Uses relaxed
    /// eccentricity filtering, auto FWHM estimation, and 8-connectivity for
    /// undersampled PSFs that may not connect well with 4-connectivity.
    pub fn wide_field() -> Self {
        Self {
            expected_fwhm: 6.0,
            auto_estimate_fwhm: true,
            min_stars_for_fwhm: 15,
            min_area: 7,
            max_area: 1500,
            edge_margin: 20,
            max_eccentricity: 0.7,
            connectivity: Connectivity::Eight,
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
            expected_fwhm: 2.5,
            auto_estimate_fwhm: true,
            min_stars_for_fwhm: 15,
            min_area: 3,
            max_area: 200,
            min_snr: 15.0,
            max_eccentricity: 0.5,
            max_roundness: 0.3,
            centroid_method: CentroidMethod::GaussianFit,
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
            deblend_n_thresholds: 32,
            deblend_min_separation: 2,
            deblend_min_prominence: 0.15,
            deblend_min_contrast: 0.005,
            refinement: BackgroundRefinement::Iterative { iterations: 2 },
            duplicate_min_separation: 3.0,
            connectivity: Connectivity::Eight,
            auto_estimate_fwhm: true,
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
            // Background
            sigma_threshold: 3.0,
            bg_mask_dilation: 5,
            tile_size: 128,
            sigma_clip_iterations: 3,
            refinement: BackgroundRefinement::Iterative { iterations: 3 },

            // Region filtering
            min_area: 7,
            max_area: 2000,
            edge_margin: 15,
            connectivity: Connectivity::Eight,

            // Deblending
            deblend_min_separation: 2,
            deblend_min_prominence: 0.15,
            deblend_n_thresholds: 32,
            deblend_min_contrast: 0.003,

            // Centroid
            centroid_method: CentroidMethod::MoffatFit { beta: 2.5 },
            local_background: LocalBackgroundMethod::LocalAnnulus,

            // PSF
            expected_fwhm: 3.0,
            auto_estimate_fwhm: true,
            min_stars_for_fwhm: 30,
            fwhm_estimation_sigma_factor: 2.5,

            // Star quality filtering
            min_snr: 15.0,
            max_fwhm_deviation: 4.0,
            duplicate_min_separation: 5.0,

            ..Self::default()
        }
    }

    /// Returns true if multi-threshold deblending is enabled.
    #[inline]
    pub const fn is_multi_threshold(&self) -> bool {
        self.deblend_n_thresholds > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // NoiseModel tests
    // -------------------------------------------------------------------------

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

    // -------------------------------------------------------------------------
    // CentroidMethod tests
    // -------------------------------------------------------------------------

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

    // -------------------------------------------------------------------------
    // Config tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_config_default() {
        let config = Config::default();
        assert!(config.noise_model.is_none());
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
        let config = Config {
            expected_fwhm: 5.0,
            min_snr: 15.0,
            edge_margin: 20,
            noise_model: Some(NoiseModel::new(1.5, 5.0)),
            ..Default::default()
        };

        assert!((config.expected_fwhm - 5.0).abs() < 1e-6);
        assert!((config.min_snr - 15.0).abs() < 1e-6);
        assert_eq!(config.edge_margin, 20);
        assert!(config.noise_model.is_some());
        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn test_config_with_auto_fwhm() {
        let config = Config {
            auto_estimate_fwhm: true,
            expected_fwhm: 0.0,
            ..Default::default()
        };
        assert!(config.auto_estimate_fwhm);
        assert!((config.expected_fwhm - 0.0).abs() < 1e-6);
        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn test_config_validates_centroid() {
        let config = Config {
            centroid_method: CentroidMethod::MoffatFit { beta: 2.5 },
            ..Default::default()
        };
        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn test_config_invalid_parameters_return_exact_errors() {
        let cases = [
            (
                Config {
                    tile_size: 10,
                    ..Default::default()
                },
                StarDetectionConfigError::InvalidTileSize { value: 10 },
            ),
            (
                Config {
                    sigma_clip_iterations: 11,
                    ..Default::default()
                },
                StarDetectionConfigError::ExcessiveSigmaClipIterations { value: 11 },
            ),
            (
                Config {
                    refinement: BackgroundRefinement::Iterative { iterations: 0 },
                    ..Default::default()
                },
                StarDetectionConfigError::ZeroBackgroundRefinementIterations,
            ),
            (
                Config {
                    refinement: BackgroundRefinement::Iterative { iterations: 11 },
                    ..Default::default()
                },
                StarDetectionConfigError::ExcessiveBackgroundRefinementIterations { value: 11 },
            ),
            (
                Config {
                    bg_mask_dilation: 51,
                    ..Default::default()
                },
                StarDetectionConfigError::ExcessiveBackgroundMaskDilation { value: 51 },
            ),
            (
                Config {
                    sigma_threshold: 0.0,
                    ..Default::default()
                },
                StarDetectionConfigError::InvalidSigmaThreshold { value: 0.0 },
            ),
            (
                Config {
                    expected_fwhm: -1.0,
                    ..Default::default()
                },
                StarDetectionConfigError::InvalidExpectedFwhm { value: -1.0 },
            ),
            (
                Config {
                    psf_axis_ratio: 0.0,
                    ..Default::default()
                },
                StarDetectionConfigError::InvalidPsfAxisRatio { value: 0.0 },
            ),
            (
                Config {
                    psf_angle: f32::INFINITY,
                    ..Default::default()
                },
                StarDetectionConfigError::InvalidPsfAngle {
                    value: f32::INFINITY,
                },
            ),
            (
                Config {
                    min_stars_for_fwhm: 4,
                    ..Default::default()
                },
                StarDetectionConfigError::TooFewStarsForFwhm { value: 4 },
            ),
            (
                Config {
                    fwhm_estimation_sigma_factor: 0.5,
                    ..Default::default()
                },
                StarDetectionConfigError::InvalidFwhmEstimationSigmaFactor { value: 0.5 },
            ),
            (
                Config {
                    deblend_min_separation: 0,
                    ..Default::default()
                },
                StarDetectionConfigError::InvalidDeblendMinSeparation { value: 0 },
            ),
            (
                Config {
                    deblend_min_prominence: 1.5,
                    ..Default::default()
                },
                StarDetectionConfigError::InvalidDeblendMinProminence { value: 1.5 },
            ),
            (
                Config {
                    deblend_n_thresholds: 1,
                    ..Default::default()
                },
                StarDetectionConfigError::InvalidDeblendThresholdCount {
                    value: 1,
                    maximum: MAX_DEBLEND_N_THRESHOLDS,
                },
            ),
            (
                Config {
                    deblend_n_thresholds: MAX_DEBLEND_N_THRESHOLDS + 1,
                    ..Default::default()
                },
                StarDetectionConfigError::InvalidDeblendThresholdCount {
                    value: MAX_DEBLEND_N_THRESHOLDS + 1,
                    maximum: MAX_DEBLEND_N_THRESHOLDS,
                },
            ),
            (
                Config {
                    deblend_min_contrast: -0.1,
                    ..Default::default()
                },
                StarDetectionConfigError::InvalidDeblendMinContrast { value: -0.1 },
            ),
            (
                Config {
                    min_area: 0,
                    ..Default::default()
                },
                StarDetectionConfigError::ZeroMinArea,
            ),
            (
                Config {
                    min_area: 100,
                    max_area: 50,
                    ..Default::default()
                },
                StarDetectionConfigError::MaxAreaBelowMin {
                    min_area: 100,
                    max_area: 50,
                },
            ),
            (
                Config {
                    centroid_method: CentroidMethod::MoffatFit { beta: 0.0 },
                    ..Default::default()
                },
                StarDetectionConfigError::InvalidMoffatBeta { value: 0.0 },
            ),
            (
                Config {
                    min_snr: 0.0,
                    ..Default::default()
                },
                StarDetectionConfigError::InvalidMinSnr { value: 0.0 },
            ),
            (
                Config {
                    max_eccentricity: 1.5,
                    ..Default::default()
                },
                StarDetectionConfigError::InvalidMaxEccentricity { value: 1.5 },
            ),
            (
                Config {
                    max_sharpness: 0.0,
                    ..Default::default()
                },
                StarDetectionConfigError::InvalidMaxSharpness { value: 0.0 },
            ),
            (
                Config {
                    max_roundness: 0.0,
                    ..Default::default()
                },
                StarDetectionConfigError::InvalidMaxRoundness { value: 0.0 },
            ),
            (
                Config {
                    max_fwhm_deviation: -1.0,
                    ..Default::default()
                },
                StarDetectionConfigError::InvalidMaxFwhmDeviation { value: -1.0 },
            ),
            (
                Config {
                    duplicate_min_separation: -1.0,
                    ..Default::default()
                },
                StarDetectionConfigError::InvalidDuplicateMinSeparation { value: -1.0 },
            ),
            (
                Config {
                    noise_model: Some(NoiseModel::new(0.0, 1.0)),
                    ..Default::default()
                },
                StarDetectionConfigError::InvalidGain { value: 0.0 },
            ),
            (
                Config {
                    noise_model: Some(NoiseModel::new(1.0, -1.0)),
                    ..Default::default()
                },
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
            Config {
                deblend_n_thresholds: MAX_DEBLEND_N_THRESHOLDS,
                ..Default::default()
            }
            .validate(),
            Ok(())
        );
    }

    #[test]
    fn test_config_deblend_multi_threshold() {
        let config = Config {
            deblend_n_thresholds: 32,
            ..Default::default()
        };
        assert!(config.is_multi_threshold());
        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn test_config_wide_field_values() {
        let config = Config::wide_field();
        assert!((config.expected_fwhm - 6.0).abs() < 1e-6);
        assert!(config.auto_estimate_fwhm);
        assert_eq!(config.min_area, 7);
        assert_eq!(config.max_area, 1500);
        assert_eq!(config.edge_margin, 20);
        assert!((config.max_eccentricity - 0.7).abs() < 1e-6);
        assert_eq!(config.connectivity, Connectivity::Eight);
    }

    #[test]
    fn test_config_precise_ground_values() {
        let config = Config::precise_ground();
        assert!(matches!(
            config.centroid_method,
            CentroidMethod::MoffatFit { beta } if (beta - 2.5).abs() < 1e-6
        ));
        assert_eq!(config.local_background, LocalBackgroundMethod::LocalAnnulus);
        assert_eq!(config.deblend_n_thresholds, 32);
        assert!((config.min_snr - 15.0).abs() < 1e-6);
        assert_eq!(config.tile_size, 128);
        assert!((config.sigma_threshold - 3.0).abs() < 1e-6);
        assert!(config.auto_estimate_fwhm);
        assert_eq!(config.min_stars_for_fwhm, 30);
    }

    #[test]
    fn test_config_high_resolution_values() {
        let config = Config::high_resolution();
        assert!((config.expected_fwhm - 2.5).abs() < 1e-6);
        assert!(config.auto_estimate_fwhm);
        assert_eq!(config.min_area, 3);
        assert_eq!(config.max_area, 200);
        assert!((config.min_snr - 15.0).abs() < 1e-6);
        assert!((config.max_eccentricity - 0.5).abs() < 1e-6);
        assert!((config.max_roundness - 0.3).abs() < 1e-6);
        assert!(matches!(
            config.centroid_method,
            CentroidMethod::GaussianFit
        ));
    }

    #[test]
    fn test_config_crowded_field_values() {
        let config = Config::crowded_field();
        assert_eq!(config.deblend_n_thresholds, 32);
        assert_eq!(config.deblend_min_separation, 2);
        assert!((config.deblend_min_prominence - 0.15).abs() < 1e-6);
        assert!((config.deblend_min_contrast - 0.005).abs() < 1e-6);
        assert!(matches!(
            config.refinement,
            BackgroundRefinement::Iterative { iterations: 2 }
        ));
        assert!((config.duplicate_min_separation - 3.0).abs() < 1e-6);
        assert!(config.auto_estimate_fwhm);
    }

    #[test]
    fn test_config_rejects_non_finite_float_parameters() {
        let cases = [
            (
                Config {
                    sigma_threshold: f32::INFINITY,
                    ..Default::default()
                },
                StarDetectionConfigError::InvalidSigmaThreshold {
                    value: f32::INFINITY,
                },
            ),
            (
                Config {
                    expected_fwhm: f32::INFINITY,
                    ..Default::default()
                },
                StarDetectionConfigError::InvalidExpectedFwhm {
                    value: f32::INFINITY,
                },
            ),
            (
                Config {
                    psf_axis_ratio: f32::INFINITY,
                    ..Default::default()
                },
                StarDetectionConfigError::InvalidPsfAxisRatio {
                    value: f32::INFINITY,
                },
            ),
            (
                Config {
                    fwhm_estimation_sigma_factor: f32::INFINITY,
                    ..Default::default()
                },
                StarDetectionConfigError::InvalidFwhmEstimationSigmaFactor {
                    value: f32::INFINITY,
                },
            ),
            (
                Config {
                    deblend_min_prominence: f32::INFINITY,
                    ..Default::default()
                },
                StarDetectionConfigError::InvalidDeblendMinProminence {
                    value: f32::INFINITY,
                },
            ),
            (
                Config {
                    deblend_min_contrast: f32::INFINITY,
                    ..Default::default()
                },
                StarDetectionConfigError::InvalidDeblendMinContrast {
                    value: f32::INFINITY,
                },
            ),
            (
                Config {
                    min_snr: f32::INFINITY,
                    ..Default::default()
                },
                StarDetectionConfigError::InvalidMinSnr {
                    value: f32::INFINITY,
                },
            ),
            (
                Config {
                    max_eccentricity: f32::INFINITY,
                    ..Default::default()
                },
                StarDetectionConfigError::InvalidMaxEccentricity {
                    value: f32::INFINITY,
                },
            ),
            (
                Config {
                    max_sharpness: f32::INFINITY,
                    ..Default::default()
                },
                StarDetectionConfigError::InvalidMaxSharpness {
                    value: f32::INFINITY,
                },
            ),
            (
                Config {
                    max_roundness: f32::INFINITY,
                    ..Default::default()
                },
                StarDetectionConfigError::InvalidMaxRoundness {
                    value: f32::INFINITY,
                },
            ),
            (
                Config {
                    max_fwhm_deviation: f32::INFINITY,
                    ..Default::default()
                },
                StarDetectionConfigError::InvalidMaxFwhmDeviation {
                    value: f32::INFINITY,
                },
            ),
            (
                Config {
                    duplicate_min_separation: f32::INFINITY,
                    ..Default::default()
                },
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
        let config = Config {
            deblend_n_thresholds: 0,
            ..Default::default()
        };
        assert!(!config.is_multi_threshold());

        // >= 2 → true
        let config = Config {
            deblend_n_thresholds: 2,
            ..Default::default()
        };
        assert!(config.is_multi_threshold());
    }
}
