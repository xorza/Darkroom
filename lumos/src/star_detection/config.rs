//! Configuration types for star detection.
//!
//! This module defines the flat [`Config`] struct and associated enums used by
//! the star detection pipeline. All parameters are grouped by comments into
//! logical sections.

use super::defect_map::DefectMap;

// ============================================================================
// Enums
// ============================================================================

/// Pixel connectivity for connected component labeling.
///
/// Determines which pixels are considered neighbors when grouping
/// above-threshold pixels into connected components.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Connectivity {
    /// 4-connectivity: only horizontal and vertical neighbors.
    /// Pixels at (x±1, y) and (x, y±1) are connected.
    /// Diagonal pixels are NOT connected.
    /// This is the default and matches SExtractor behavior.
    #[default]
    Four,
    /// 8-connectivity: includes diagonal neighbors.
    /// All 8 surrounding pixels are connected.
    /// Better for undersampled PSFs or elongated sources,
    /// but may merge close star pairs more aggressively.
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
    pub fn validate(&self) {
        if let CentroidMethod::MoffatFit { beta } = self {
            assert!(*beta > 0.0, "MoffatFit beta must be positive, got {}", beta);
            assert!(
                *beta <= 10.0,
                "MoffatFit beta should be <= 10.0 for realistic PSFs, got {}",
                beta
            );
        }
    }
}

// ============================================================================
// Noise Model
// ============================================================================

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
    pub fn validate(&self) {
        assert!(self.gain > 0.0, "gain must be positive, got {}", self.gain);
        assert!(
            self.read_noise >= 0.0,
            "read_noise must be non-negative, got {}",
            self.read_noise
        );
    }
}

// ============================================================================
// Adaptive Sigma Configuration
// ============================================================================

/// Configuration for adaptive sigma threshold computation.
///
/// Adaptive thresholding adjusts the detection sigma based on local image
/// characteristics. In high-contrast regions (nebulosity, gradients), the
/// sigma is increased to reduce false positives. In uniform sky regions,
/// the base sigma is used for maximum sensitivity.
#[derive(Debug, Clone, Copy)]
pub struct AdaptiveSigmaConfig {
    /// Base sigma threshold used in low-contrast (uniform sky) regions.
    pub base_sigma: f32,
    /// Maximum sigma threshold used in high-contrast (nebulous) regions.
    pub max_sigma: f32,
    /// Contrast sensitivity factor (higher = more sensitive to contrast).
    /// Controls how quickly sigma increases with local contrast.
    pub contrast_factor: f32,
}

impl Default for AdaptiveSigmaConfig {
    fn default() -> Self {
        Self {
            base_sigma: 4.0,
            max_sigma: 8.0,
            contrast_factor: 2.0,
        }
    }
}

impl AdaptiveSigmaConfig {
    /// Validate the configuration.
    pub fn validate(&self) {
        assert!(
            self.base_sigma > 0.0,
            "base_sigma must be positive, got {}",
            self.base_sigma
        );
        assert!(
            self.max_sigma >= self.base_sigma,
            "max_sigma ({}) must be >= base_sigma ({})",
            self.max_sigma,
            self.base_sigma
        );
        assert!(
            self.contrast_factor > 0.0,
            "contrast_factor must be positive, got {}",
            self.contrast_factor
        );
    }

    /// Create a conservative config (higher thresholds, fewer false positives).
    pub fn conservative() -> Self {
        Self {
            base_sigma: 4.0,
            max_sigma: 8.0,
            contrast_factor: 3.0,
        }
    }

    /// Create an aggressive config (lower thresholds, more detections).
    pub fn aggressive() -> Self {
        Self {
            base_sigma: 3.0,
            max_sigma: 5.0,
            contrast_factor: 1.5,
        }
    }
}

// ============================================================================
// Background Refinement
// ============================================================================

/// Strategy for refining background estimation.
///
/// Background estimation can be improved using one of two mutually exclusive
/// strategies:
///
/// - **Iterative refinement**: Mask detected sources and re-estimate background.
///   Best for crowded fields where initial estimate is biased by sources.
///
/// - **Adaptive sigma**: Use per-pixel detection thresholds based on local contrast.
///   Best for images with nebulosity or structured backgrounds where a fixed
///   threshold causes false detections.
///
/// These strategies are mutually exclusive because iterative refinement already
/// produces an accurate background that doesn't need adaptive thresholding, and
/// computing adaptive sigma during refinement iterations would be wasted work.
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

    /// Adaptive per-pixel sigma thresholds based on local contrast.
    /// Higher thresholds in nebulous regions reduce false positives.
    /// Only used with single-pass estimation (no iterative refinement).
    AdaptiveSigma(AdaptiveSigmaConfig),
}

impl BackgroundRefinement {
    /// Validate the configuration.
    pub fn validate(&self) {
        match self {
            Self::None => {}
            Self::Iterative { iterations } => {
                assert!(
                    *iterations <= 10,
                    "iterations must be <= 10, got {}",
                    iterations
                );
                assert!(
                    *iterations > 0,
                    "iterations must be > 0 for Iterative refinement"
                );
            }
            Self::AdaptiveSigma(config) => {
                config.validate();
            }
        }
    }

    /// Returns the number of iterations (0 for None and AdaptiveSigma).
    pub fn iterations(&self) -> usize {
        match self {
            Self::Iterative { iterations } => *iterations,
            _ => 0,
        }
    }

    /// Returns the adaptive sigma config if using that strategy.
    pub fn adaptive_sigma(&self) -> Option<AdaptiveSigmaConfig> {
        match self {
            Self::AdaptiveSigma(config) => Some(*config),
            _ => None,
        }
    }
}

// ============================================================================
// Star Detection Configuration
// ============================================================================

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
    /// Minimum fraction of unmasked pixels per tile for refinement.
    pub min_unmasked_fraction: f32,

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

    // -- Defect map --
    /// Optional defect map for masking bad pixels.
    pub defect_map: Option<DefectMap>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            // Background estimation
            tile_size: 64,
            sigma_clip_iterations: 3,
            refinement: BackgroundRefinement::None,
            bg_mask_dilation: 3,
            min_unmasked_fraction: 0.3,

            // Detection
            sigma_threshold: 4.0,
            connectivity: Connectivity::Four,

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

            // Defect map
            defect_map: None,
        }
    }
}

impl Config {
    /// Validate the configuration, panicking if invalid.
    pub fn validate(&self) {
        // Background
        assert!(
            (16..=256).contains(&self.tile_size),
            "tile_size must be between 16 and 256, got {}",
            self.tile_size
        );
        assert!(
            self.sigma_clip_iterations <= 10,
            "sigma_clip_iterations must be <= 10, got {}",
            self.sigma_clip_iterations
        );
        self.refinement.validate();
        assert!(
            self.bg_mask_dilation <= 50,
            "bg_mask_dilation must be <= 50, got {}",
            self.bg_mask_dilation
        );
        assert!(
            (0.0..=1.0).contains(&self.min_unmasked_fraction),
            "min_unmasked_fraction must be in [0, 1], got {}",
            self.min_unmasked_fraction
        );

        // Detection
        assert!(
            self.sigma_threshold > 0.0,
            "sigma_threshold must be positive, got {}",
            self.sigma_threshold
        );

        // PSF
        assert!(
            self.expected_fwhm >= 0.0,
            "expected_fwhm must be non-negative (0.0 disables matched filter), got {}",
            self.expected_fwhm
        );
        assert!(
            self.psf_axis_ratio > 0.0 && self.psf_axis_ratio <= 1.0,
            "psf_axis_ratio must be in (0, 1], got {}",
            self.psf_axis_ratio
        );
        assert!(
            self.min_stars_for_fwhm >= 5,
            "min_stars_for_fwhm must be at least 5, got {}",
            self.min_stars_for_fwhm
        );
        assert!(
            self.fwhm_estimation_sigma_factor >= 1.0,
            "fwhm_estimation_sigma_factor must be >= 1.0, got {}",
            self.fwhm_estimation_sigma_factor
        );

        // Deblending
        assert!(
            self.deblend_min_separation >= 1,
            "deblend_min_separation must be at least 1, got {}",
            self.deblend_min_separation
        );
        assert!(
            (0.0..=1.0).contains(&self.deblend_min_prominence),
            "deblend_min_prominence must be in [0, 1], got {}",
            self.deblend_min_prominence
        );
        assert!(
            self.deblend_n_thresholds == 0 || self.deblend_n_thresholds >= 2,
            "deblend_n_thresholds must be 0 (disabled) or at least 2, got {}",
            self.deblend_n_thresholds
        );
        assert!(
            (0.0..=1.0).contains(&self.deblend_min_contrast),
            "deblend_min_contrast must be in [0, 1], got {}",
            self.deblend_min_contrast
        );

        // Region filtering
        assert!(
            self.min_area >= 1,
            "min_area must be at least 1, got {}",
            self.min_area
        );
        assert!(
            self.max_area >= self.min_area,
            "max_area ({}) must be >= min_area ({})",
            self.max_area,
            self.min_area
        );

        // Centroid
        self.centroid_method.validate();

        // Star quality filtering
        assert!(
            self.min_snr > 0.0,
            "min_snr must be positive, got {}",
            self.min_snr
        );
        assert!(
            (0.0..=1.0).contains(&self.max_eccentricity),
            "max_eccentricity must be in [0, 1], got {}",
            self.max_eccentricity
        );
        assert!(
            self.max_sharpness > 0.0 && self.max_sharpness <= 1.0,
            "max_sharpness must be in (0, 1], got {}",
            self.max_sharpness
        );
        assert!(
            self.max_roundness > 0.0 && self.max_roundness <= 1.0,
            "max_roundness must be in (0, 1], got {}",
            self.max_roundness
        );
        assert!(
            self.max_fwhm_deviation >= 0.0,
            "max_fwhm_deviation must be non-negative, got {}",
            self.max_fwhm_deviation
        );
        assert!(
            self.duplicate_min_separation >= 0.0,
            "duplicate_min_separation must be non-negative, got {}",
            self.duplicate_min_separation
        );

        // Noise model
        if let Some(ref noise) = self.noise_model {
            noise.validate();
        }
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

    /// Nebulous field settings (adaptive thresholding enabled).
    ///
    /// Images with bright nebulosity, H-II regions, or structured backgrounds
    /// where a fixed detection threshold causes massive false positives in
    /// bright regions. Uses adaptive per-pixel sigma thresholds.
    pub fn nebulous_field() -> Self {
        Self {
            tile_size: 128,
            bg_mask_dilation: 5,
            refinement: BackgroundRefinement::AdaptiveSigma(AdaptiveSigmaConfig {
                base_sigma: 4.0,
                max_sigma: 10.0,
                contrast_factor: 2.5,
            }),
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
            min_unmasked_fraction: 0.2,
            tile_size: 128,
            sigma_clip_iterations: 3,
            refinement: BackgroundRefinement::AdaptiveSigma(AdaptiveSigmaConfig {
                base_sigma: 3.0,
                max_sigma: 10.0,
                contrast_factor: 3.0,
            }),

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
            max_eccentricity: 0.5,
            max_sharpness: 0.7,
            max_roundness: 0.3,
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

// ============================================================================
// Tests
// ============================================================================

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
        model.validate();
    }

    #[test]
    #[should_panic(expected = "gain must be positive")]
    fn test_noise_model_invalid_gain() {
        NoiseModel::new(0.0, 5.0).validate();
    }

    #[test]
    #[should_panic(expected = "read_noise must be non-negative")]
    fn test_noise_model_invalid_read_noise() {
        NoiseModel::new(1.0, -1.0).validate();
    }

    // -------------------------------------------------------------------------
    // AdaptiveSigmaConfig tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_adaptive_sigma_config_default() {
        let config = AdaptiveSigmaConfig::default();
        assert!((config.base_sigma - 4.0).abs() < 1e-6);
        assert!((config.max_sigma - 8.0).abs() < 1e-6);
        assert!((config.contrast_factor - 2.0).abs() < 1e-6);
        config.validate();
    }

    #[test]
    fn test_adaptive_sigma_config_conservative() {
        let config = AdaptiveSigmaConfig::conservative();
        assert!((config.base_sigma - 4.0).abs() < 1e-6);
        assert!((config.max_sigma - 8.0).abs() < 1e-6);
        config.validate();
    }

    #[test]
    fn test_adaptive_sigma_config_aggressive() {
        let config = AdaptiveSigmaConfig::aggressive();
        assert!((config.base_sigma - 3.0).abs() < 1e-6);
        assert!((config.max_sigma - 5.0).abs() < 1e-6);
        config.validate();
    }

    #[test]
    #[should_panic(expected = "base_sigma must be positive")]
    fn test_adaptive_sigma_config_invalid_base_sigma() {
        AdaptiveSigmaConfig {
            base_sigma: 0.0,
            ..Default::default()
        }
        .validate();
    }

    #[test]
    #[should_panic(expected = "max_sigma")]
    fn test_adaptive_sigma_config_max_less_than_base() {
        AdaptiveSigmaConfig {
            base_sigma: 5.0,
            max_sigma: 3.0,
            ..Default::default()
        }
        .validate();
    }

    #[test]
    #[should_panic(expected = "contrast_factor must be positive")]
    fn test_adaptive_sigma_config_invalid_contrast_factor() {
        AdaptiveSigmaConfig {
            contrast_factor: 0.0,
            ..Default::default()
        }
        .validate();
    }

    // -------------------------------------------------------------------------
    // CentroidMethod tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_centroid_method_validate() {
        CentroidMethod::WeightedMoments.validate();
        CentroidMethod::GaussianFit.validate();
        CentroidMethod::MoffatFit { beta: 2.5 }.validate();
    }

    #[test]
    #[should_panic(expected = "MoffatFit beta must be positive")]
    fn test_centroid_method_moffat_invalid_beta_zero() {
        CentroidMethod::MoffatFit { beta: 0.0 }.validate();
    }

    #[test]
    #[should_panic(expected = "MoffatFit beta should be <= 10.0")]
    fn test_centroid_method_moffat_invalid_beta_large() {
        CentroidMethod::MoffatFit { beta: 15.0 }.validate();
    }

    // -------------------------------------------------------------------------
    // Config tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_config_default() {
        let config = Config::default();
        assert!(config.refinement.adaptive_sigma().is_none());
        assert!(config.noise_model.is_none());
        assert!(config.defect_map.is_none());
        config.validate();
    }

    #[test]
    fn test_config_presets() {
        Config::wide_field().validate();
        Config::high_resolution().validate();
        Config::crowded_field().validate();
        Config::nebulous_field().validate();
        Config::precise_ground().validate();
    }

    #[test]
    fn test_config_nebulous_field() {
        let config = Config::nebulous_field();
        assert!(config.refinement.adaptive_sigma().is_some());
        config.validate();
    }

    #[test]
    fn test_config_custom() {
        let config = Config {
            expected_fwhm: 5.0,
            min_snr: 15.0,
            edge_margin: 20,
            noise_model: Some(NoiseModel::new(1.5, 5.0)),
            refinement: BackgroundRefinement::AdaptiveSigma(AdaptiveSigmaConfig::default()),
            ..Default::default()
        };

        assert!((config.expected_fwhm - 5.0).abs() < 1e-6);
        assert!((config.min_snr - 15.0).abs() < 1e-6);
        assert_eq!(config.edge_margin, 20);
        assert!(config.noise_model.is_some());
        assert!(config.refinement.adaptive_sigma().is_some());
        config.validate();
    }

    #[test]
    fn test_config_with_adaptive_sigma() {
        let config = Config {
            refinement: BackgroundRefinement::AdaptiveSigma(AdaptiveSigmaConfig::default()),
            ..Default::default()
        };
        assert!(config.refinement.adaptive_sigma().is_some());
        config.validate();
    }

    #[test]
    fn test_config_with_custom_adaptive_sigma() {
        let custom = AdaptiveSigmaConfig {
            base_sigma: 2.5,
            max_sigma: 7.0,
            contrast_factor: 1.0,
        };
        let config = Config {
            refinement: BackgroundRefinement::AdaptiveSigma(custom),
            ..Default::default()
        };
        let adaptive = config.refinement.adaptive_sigma().unwrap();
        assert!((adaptive.base_sigma - 2.5).abs() < 1e-6);
        assert!((adaptive.max_sigma - 7.0).abs() < 1e-6);
        config.validate();
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
        config.validate();
    }

    #[test]
    fn test_config_validates_centroid() {
        let config = Config {
            centroid_method: CentroidMethod::MoffatFit { beta: 2.5 },
            ..Default::default()
        };
        config.validate(); // Should not panic with valid beta
    }

    #[test]
    #[should_panic(expected = "sigma_threshold must be positive")]
    fn test_config_invalid_sigma_threshold() {
        Config {
            sigma_threshold: 0.0,
            ..Default::default()
        }
        .validate();
    }

    #[test]
    #[should_panic(expected = "tile_size must be between 16 and 256")]
    fn test_config_invalid_tile_size() {
        Config {
            tile_size: 10,
            ..Default::default()
        }
        .validate();
    }

    #[test]
    #[should_panic(expected = "min_area must be at least 1")]
    fn test_config_invalid_min_area() {
        Config {
            min_area: 0,
            ..Default::default()
        }
        .validate();
    }

    #[test]
    #[should_panic(expected = "max_area")]
    fn test_config_invalid_max_area() {
        Config {
            min_area: 100,
            max_area: 50,
            ..Default::default()
        }
        .validate();
    }

    #[test]
    #[should_panic(expected = "min_snr must be positive")]
    fn test_config_invalid_min_snr() {
        Config {
            min_snr: 0.0,
            ..Default::default()
        }
        .validate();
    }

    #[test]
    #[should_panic(expected = "psf_axis_ratio must be in (0, 1]")]
    fn test_config_invalid_axis_ratio() {
        Config {
            psf_axis_ratio: 0.0,
            ..Default::default()
        }
        .validate();
    }

    #[test]
    #[should_panic(expected = "min_stars_for_fwhm must be at least 5")]
    fn test_config_invalid_min_stars() {
        Config {
            min_stars_for_fwhm: 3,
            ..Default::default()
        }
        .validate();
    }

    #[test]
    #[should_panic(expected = "deblend_min_separation must be at least 1")]
    fn test_config_invalid_deblend_min_separation() {
        Config {
            deblend_min_separation: 0,
            ..Default::default()
        }
        .validate();
    }

    #[test]
    #[should_panic(expected = "deblend_n_thresholds must be 0 (disabled) or at least 2")]
    fn test_config_invalid_deblend_n_thresholds() {
        Config {
            deblend_n_thresholds: 1,
            ..Default::default()
        }
        .validate();
    }

    #[test]
    fn test_config_deblend_multi_threshold() {
        let config = Config {
            deblend_n_thresholds: 32,
            ..Default::default()
        };
        assert!(config.is_multi_threshold());
        config.validate();
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
        assert!(config.refinement.adaptive_sigma().is_some());
        assert_eq!(config.deblend_n_thresholds, 32);
        assert!((config.min_snr - 15.0).abs() < 1e-6);
    }
}
