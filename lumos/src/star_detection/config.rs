//! Configuration types for star detection.
//!
//! This module defines all configuration structs used by the star detection pipeline.
//! The main entry point is [`StarDetectionConfig`], which groups all sub-configurations.

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
// Centroid Configuration
// ============================================================================

/// Configuration for centroid computation.
#[derive(Debug, Clone, Copy, Default)]
pub struct CentroidConfig {
    /// Method for computing sub-pixel centroids.
    pub method: CentroidMethod,
    /// Method for computing local background during centroid refinement.
    pub local_background_method: LocalBackgroundMethod,
}

impl CentroidConfig {
    /// Validate the configuration.
    pub fn validate(&self) {
        self.method.validate();
    }
}

// ============================================================================
// Deblend Configuration
// ============================================================================

/// Configuration for the deblending algorithm.
#[derive(Debug, Clone, Copy)]
pub struct DeblendConfig {
    /// Minimum separation between peaks for deblending (in pixels).
    pub min_separation: usize,
    /// Minimum peak prominence as fraction of primary peak for deblending.
    pub min_prominence: f32,
    /// Number of sub-thresholds for multi-threshold deblending.
    /// Set to 0 for simple local-maxima deblending (faster).
    /// Set to 32+ for SExtractor-style tree-based deblending (more accurate).
    pub n_thresholds: usize,
    /// Minimum contrast for multi-threshold deblending.
    pub min_contrast: f32,
}

impl Default for DeblendConfig {
    fn default() -> Self {
        Self {
            min_separation: 3,
            min_prominence: 0.3,
            n_thresholds: 0,
            min_contrast: 0.005,
        }
    }
}

impl DeblendConfig {
    /// Returns true if multi-threshold deblending is enabled.
    #[inline]
    pub const fn is_multi_threshold(&self) -> bool {
        self.n_thresholds > 0
    }

    /// Validate the configuration.
    pub fn validate(&self) {
        assert!(
            self.min_separation >= 1,
            "min_separation must be at least 1, got {}",
            self.min_separation
        );
        assert!(
            (0.0..=1.0).contains(&self.min_prominence),
            "min_prominence must be in [0, 1], got {}",
            self.min_prominence
        );
        assert!(
            self.n_thresholds == 0 || self.n_thresholds >= 2,
            "n_thresholds must be 0 (disabled) or at least 2, got {}",
            self.n_thresholds
        );
        assert!(
            (0.0..=1.0).contains(&self.min_contrast),
            "min_contrast must be in [0, 1], got {}",
            self.min_contrast
        );
    }
}

// ============================================================================
// Filtering Configuration
// ============================================================================

/// Configuration for star filtering criteria.
#[derive(Debug, Clone, Copy)]
pub struct FilteringConfig {
    /// Minimum star area in pixels.
    pub min_area: usize,
    /// Maximum star area in pixels.
    pub max_area: usize,
    /// Edge margin in pixels (stars too close to edge are rejected).
    pub edge_margin: usize,
    /// Minimum SNR for a star to be considered valid.
    pub min_snr: f32,
    /// Maximum eccentricity (0-1, higher = more elongated allowed).
    pub max_eccentricity: f32,
    /// Maximum sharpness for a star to be considered valid.
    /// Sharpness = peak_value / flux_in_3x3_core. Cosmic rays have very high sharpness
    /// (>0.7) because most flux is in a single pixel. Set to 1.0 to disable.
    pub max_sharpness: f32,
    /// Maximum roundness for a star to be considered valid.
    /// Two metrics are checked: roundness1 (peak height asymmetry between x/y
    /// marginals) and roundness2 (bilateral asymmetry). Circular sources have
    /// both metrics near 0; well-behaved stars are typically < 0.1.
    /// Set to 1.0 to disable.
    pub max_roundness: f32,
    /// Maximum FWHM deviation from median in MAD units.
    /// Stars with FWHM > median + max_fwhm_deviation * MAD are rejected.
    /// Set to 0.0 to disable.
    pub max_fwhm_deviation: f32,
    /// Minimum separation between stars for duplicate removal (in pixels).
    /// Stars closer than this are considered duplicates; only brightest is kept.
    pub duplicate_min_separation: f32,
    /// Pixel connectivity for connected component labeling.
    pub connectivity: Connectivity,
}

impl Default for FilteringConfig {
    fn default() -> Self {
        Self {
            min_area: 5,
            max_area: 500,
            edge_margin: 10,
            min_snr: 10.0,
            max_eccentricity: 0.6,
            max_sharpness: 0.7,
            max_roundness: 0.5,
            max_fwhm_deviation: 3.0,
            duplicate_min_separation: 8.0,
            connectivity: Connectivity::Four,
        }
    }
}

impl FilteringConfig {
    /// Validate the configuration.
    pub fn validate(&self) {
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
    }
}

// ============================================================================
// PSF Configuration
// ============================================================================

/// Configuration for PSF and matched filtering.
#[derive(Debug, Clone, Copy)]
pub struct PsfConfig {
    /// Expected FWHM of stars in pixels for matched filtering.
    /// Set to 0.0 to disable matched filtering.
    /// Typical values are 2.0-6.0 pixels depending on seeing and sampling.
    pub expected_fwhm: f32,
    /// Axis ratio for elliptical Gaussian matched filter (minor/major axis).
    /// Value of 1.0 means circular PSF (default), smaller values mean more elongated.
    pub axis_ratio: f32,
    /// Position angle of PSF major axis in radians (0 = along x-axis).
    /// Only used when axis_ratio < 1.0.
    pub angle: f32,
    /// Enable automatic FWHM estimation from bright stars.
    pub auto_estimate: bool,
    /// Minimum number of stars required for valid FWHM estimation.
    pub min_stars_for_estimation: usize,
    /// Sigma threshold multiplier for first-pass bright star detection.
    pub estimation_sigma_factor: f32,
}

impl Default for PsfConfig {
    fn default() -> Self {
        Self {
            expected_fwhm: 4.0,
            axis_ratio: 1.0,
            angle: 0.0,
            auto_estimate: false,
            min_stars_for_estimation: 10,
            estimation_sigma_factor: 2.0,
        }
    }
}

impl PsfConfig {
    /// Validate the configuration.
    pub fn validate(&self) {
        assert!(
            self.expected_fwhm >= 0.0,
            "expected_fwhm must be non-negative (0.0 disables matched filter), got {}",
            self.expected_fwhm
        );
        assert!(
            self.axis_ratio > 0.0 && self.axis_ratio <= 1.0,
            "axis_ratio must be in (0, 1], got {}",
            self.axis_ratio
        );
        assert!(
            self.min_stars_for_estimation >= 5,
            "min_stars_for_estimation must be at least 5, got {}",
            self.min_stars_for_estimation
        );
        assert!(
            self.estimation_sigma_factor >= 1.0,
            "estimation_sigma_factor must be >= 1.0, got {}",
            self.estimation_sigma_factor
        );
    }
}

// ============================================================================
// Background Configuration
// ============================================================================

/// Configuration for background estimation.
#[derive(Debug, Clone)]
pub struct BackgroundConfig {
    /// Detection threshold in sigma above background for masking objects.
    /// Higher values = more conservative masking (only mask very bright objects).
    /// Typical value: 3.0-5.0
    pub sigma_threshold: f32,
    /// Dilation radius for object masks in pixels.
    /// Expands masked regions to ensure object wings are excluded.
    /// Typical value: 2-5 pixels.
    pub mask_dilation: usize,
    /// Minimum fraction of pixels that must remain unmasked per tile.
    /// If too many pixels are masked, use original (unrefined) estimate.
    /// Typical value: 0.3-0.5
    pub min_unmasked_fraction: f32,
    /// Tile size for background estimation in pixels.
    pub tile_size: usize,
    /// Number of sigma-clipping iterations for tile statistics.
    /// With MAD-based sigma, 2-3 iterations typically suffice.
    /// Early termination stops iteration when no values are clipped.
    /// Typical value: 2-3
    pub sigma_clip_iterations: usize,
    /// Background refinement strategy.
    /// Choose between iterative refinement (for crowded fields) or
    /// adaptive sigma thresholds (for nebulous backgrounds).
    pub refinement: BackgroundRefinement,
}

impl Default for BackgroundConfig {
    fn default() -> Self {
        Self {
            sigma_threshold: 4.0,
            mask_dilation: 3,
            min_unmasked_fraction: 0.3,
            tile_size: 64,
            sigma_clip_iterations: 3,
            refinement: BackgroundRefinement::None,
        }
    }
}

impl BackgroundConfig {
    /// Validate the configuration.
    pub fn validate(&self) {
        assert!(
            self.sigma_threshold > 0.0,
            "sigma_threshold must be positive, got {}",
            self.sigma_threshold
        );
        assert!(
            self.mask_dilation <= 50,
            "mask_dilation must be <= 50, got {}",
            self.mask_dilation
        );
        assert!(
            (0.0..=1.0).contains(&self.min_unmasked_fraction),
            "min_unmasked_fraction must be in [0, 1], got {}",
            self.min_unmasked_fraction
        );
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
    }
}

// ============================================================================
// Star Detection Configuration
// ============================================================================

/// Configuration for star detection.
///
/// This is the main configuration struct that groups all detection parameters
/// into logical sub-configs for better organization.
#[derive(Debug, Clone, Default)]
pub struct StarDetectionConfig {
    /// Background estimation configuration.
    pub background: BackgroundConfig,
    /// Star filtering criteria.
    pub filtering: FilteringConfig,
    /// Deblending configuration.
    pub deblend: DeblendConfig,
    /// Centroid computation configuration.
    pub centroid: CentroidConfig,
    /// PSF and matched filtering configuration.
    pub psf: PsfConfig,
    /// Optional camera noise model for accurate SNR calculation.
    /// When None, uses simplified background-dominated SNR formula.
    pub noise_model: Option<NoiseModel>,
    /// Optional defect map for masking bad pixels.
    pub defect_map: Option<DefectMap>,
}

impl StarDetectionConfig {
    /// Validate the configuration and all sub-configs, panicking if invalid.
    pub fn validate(&self) {
        self.background.validate();
        self.filtering.validate();
        self.deblend.validate();
        self.centroid.validate();
        self.psf.validate();
        if let Some(ref noise) = self.noise_model {
            noise.validate();
        }
    }

    // =========================================================================
    // Chainable Preset Modifiers
    // =========================================================================

    /// Apply wide-field imaging settings (short focal length, large pixel scale).
    ///
    /// Wide-field setups produce larger stars (FWHM 5-8px) that may be slightly
    /// elongated at field edges due to coma and field curvature. Uses relaxed
    /// eccentricity filtering, auto FWHM estimation, and 8-connectivity for
    /// undersampled PSFs that may not connect well with 4-connectivity.
    pub fn wide_field(mut self) -> Self {
        self.psf.expected_fwhm = 6.0;
        self.psf.auto_estimate = true;
        self.psf.min_stars_for_estimation = 15;
        self.filtering.min_area = 7;
        self.filtering.max_area = 1500;
        self.filtering.edge_margin = 20;
        self.filtering.max_eccentricity = 0.7;
        self.filtering.connectivity = Connectivity::Eight;
        self
    }

    /// Apply high-resolution imaging settings (long focal length, small pixel scale).
    ///
    /// Well-sampled Nyquist PSFs (FWHM 2-4px) with symmetric profiles. Uses
    /// Gaussian centroid fitting for maximum precision on well-sampled stars,
    /// stricter eccentricity and roundness filtering, and higher SNR threshold
    /// to build a clean, high-quality star catalog. Auto FWHM estimation
    /// ensures the matched filter adapts to actual seeing conditions.
    pub fn high_resolution(mut self) -> Self {
        self.psf.expected_fwhm = 2.5;
        self.psf.auto_estimate = true;
        self.psf.min_stars_for_estimation = 15;
        self.filtering.min_area = 3;
        self.filtering.max_area = 200;
        self.filtering.min_snr = 15.0;
        self.filtering.max_eccentricity = 0.5;
        self.filtering.max_roundness = 0.3;
        self.centroid.method = CentroidMethod::GaussianFit;
        self
    }

    /// Apply crowded field settings (globular clusters, dense star fields).
    ///
    /// Enables SExtractor-style multi-threshold deblending (32 sub-thresholds)
    /// with low contrast threshold to separate close blends. Uses iterative
    /// background refinement to re-estimate background after masking sources
    /// (critical in crowded fields where sources bias the initial estimate).
    /// 8-connectivity prevents artificial splitting of close pairs.
    /// Lower min_prominence allows detecting secondary peaks in blends.
    pub fn crowded_field(mut self) -> Self {
        self.deblend.n_thresholds = 32;
        self.deblend.min_separation = 2;
        self.deblend.min_prominence = 0.15;
        self.deblend.min_contrast = 0.005;
        self.background.refinement = BackgroundRefinement::Iterative { iterations: 2 };
        self.filtering.duplicate_min_separation = 3.0;
        self.filtering.connectivity = Connectivity::Eight;
        self.psf.auto_estimate = true;
        self
    }

    /// Apply nebulous field settings (adaptive thresholding enabled).
    ///
    /// Images with bright nebulosity, H-II regions, or structured backgrounds
    /// where a fixed detection threshold causes massive false positives in
    /// bright regions. Uses adaptive per-pixel sigma thresholds that increase
    /// in high-contrast nebular regions while maintaining sensitivity in clear
    /// sky areas. Larger tile size (128px) averages over nebular structure for
    /// more stable background estimation. Higher mask dilation prevents nebular
    /// wings from contaminating tile statistics.
    pub fn nebulous_field(mut self) -> Self {
        self.background.tile_size = 128;
        self.background.mask_dilation = 5;
        self.background.refinement = BackgroundRefinement::AdaptiveSigma(AdaptiveSigmaConfig {
            base_sigma: 4.0,
            max_sigma: 10.0,
            contrast_factor: 2.5,
        });
        self.psf.auto_estimate = true;
        self
    }

    /// Apply maximum centroid precision settings for ground-based astrophotography.
    ///
    /// Optimized for sub-pixel astrometric accuracy. Uses Moffat PSF fitting
    /// (beta=2.5, IRAF default) which models atmospheric seeing wings better
    /// than Gaussian (Trujillo et al. 2001). Local annulus background subtraction
    /// handles nebulosity near stars. Adaptive sigma thresholds suppress false
    /// detections in variable backgrounds. Aggressive multi-threshold deblending
    /// (SExtractor-style, 32 levels) resolves close pairs. Higher SNR threshold
    /// (15σ) and strict shape filtering ensure only well-measured, isolated
    /// stars enter the final catalog.
    pub fn precise_ground(mut self) -> Self {
        self.background.sigma_threshold = 3.0;
        self.background.mask_dilation = 5;
        self.background.min_unmasked_fraction = 0.2;
        self.background.tile_size = 128;
        self.background.sigma_clip_iterations = 3;
        self.background.refinement = BackgroundRefinement::AdaptiveSigma(AdaptiveSigmaConfig {
            base_sigma: 3.0,
            max_sigma: 10.0,
            contrast_factor: 3.0,
        });
        self.filtering.min_area = 7;
        self.filtering.max_area = 2000;
        self.filtering.edge_margin = 15;
        self.filtering.min_snr = 15.0;
        self.filtering.max_eccentricity = 0.5;
        self.filtering.max_sharpness = 0.7;
        self.filtering.max_roundness = 0.3;
        self.filtering.max_fwhm_deviation = 4.0;
        self.filtering.duplicate_min_separation = 5.0;
        self.filtering.connectivity = Connectivity::Eight;
        self.deblend.min_separation = 2;
        self.deblend.min_prominence = 0.15;
        self.deblend.n_thresholds = 32;
        self.deblend.min_contrast = 0.003;
        self.centroid.method = CentroidMethod::MoffatFit { beta: 2.5 };
        self.centroid.local_background_method = LocalBackgroundMethod::LocalAnnulus;
        self.psf.expected_fwhm = 3.0;
        self.psf.auto_estimate = true;
        self.psf.min_stars_for_estimation = 30;
        self.psf.estimation_sigma_factor = 2.5;
        self
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
    // CentroidConfig tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_centroid_config_default() {
        let config = CentroidConfig::default();
        assert_eq!(config.method, CentroidMethod::WeightedMoments);
        assert_eq!(
            config.local_background_method,
            LocalBackgroundMethod::GlobalMap
        );
        config.validate();
    }

    // -------------------------------------------------------------------------
    // DeblendConfig tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_deblend_config_default() {
        let config = DeblendConfig::default();
        assert!(!config.is_multi_threshold());
        config.validate();
    }

    #[test]
    fn test_deblend_config_multi_threshold() {
        let config = DeblendConfig {
            n_thresholds: 32,
            ..Default::default()
        };
        assert!(config.is_multi_threshold());
        config.validate();
    }

    #[test]
    #[should_panic(expected = "min_separation must be at least 1")]
    fn test_deblend_config_invalid_min_separation() {
        DeblendConfig {
            min_separation: 0,
            ..Default::default()
        }
        .validate();
    }

    #[test]
    #[should_panic(expected = "n_thresholds must be 0 (disabled) or at least 2")]
    fn test_deblend_config_invalid_n_thresholds() {
        DeblendConfig {
            n_thresholds: 1,
            ..Default::default()
        }
        .validate();
    }

    // -------------------------------------------------------------------------
    // FilteringConfig tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_filtering_config_default() {
        let config = FilteringConfig::default();
        config.validate();
    }

    #[test]
    #[should_panic(expected = "min_area must be at least 1")]
    fn test_filtering_config_invalid_min_area() {
        FilteringConfig {
            min_area: 0,
            ..Default::default()
        }
        .validate();
    }

    #[test]
    #[should_panic(expected = "max_area")]
    fn test_filtering_config_invalid_max_area() {
        FilteringConfig {
            min_area: 100,
            max_area: 50,
            ..Default::default()
        }
        .validate();
    }

    #[test]
    #[should_panic(expected = "min_snr must be positive")]
    fn test_filtering_config_invalid_min_snr() {
        FilteringConfig {
            min_snr: 0.0,
            ..Default::default()
        }
        .validate();
    }

    // -------------------------------------------------------------------------
    // PsfConfig tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_psf_config_default() {
        let config = PsfConfig::default();
        config.validate();
    }

    #[test]
    #[should_panic(expected = "axis_ratio must be in (0, 1]")]
    fn test_psf_config_invalid_axis_ratio() {
        PsfConfig {
            axis_ratio: 0.0,
            ..Default::default()
        }
        .validate();
    }

    #[test]
    #[should_panic(expected = "min_stars_for_estimation must be at least 5")]
    fn test_psf_config_invalid_min_stars() {
        PsfConfig {
            min_stars_for_estimation: 3,
            ..Default::default()
        }
        .validate();
    }

    // -------------------------------------------------------------------------
    // BackgroundConfig tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_background_config_default() {
        let config = BackgroundConfig::default();
        config.validate();
    }

    #[test]
    #[should_panic(expected = "sigma_threshold must be positive")]
    fn test_background_config_invalid_sigma_threshold() {
        BackgroundConfig {
            sigma_threshold: 0.0,
            ..Default::default()
        }
        .validate();
    }

    #[test]
    #[should_panic(expected = "tile_size must be between 16 and 256")]
    fn test_background_config_invalid_tile_size() {
        BackgroundConfig {
            tile_size: 10,
            ..Default::default()
        }
        .validate();
    }

    // -------------------------------------------------------------------------
    // StarDetectionConfig tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_star_detection_config_default() {
        let config = StarDetectionConfig::default();
        assert!(config.background.refinement.adaptive_sigma().is_none());
        assert!(config.noise_model.is_none());
        assert!(config.defect_map.is_none());
        config.validate();
    }

    #[test]
    fn test_star_detection_config_presets() {
        StarDetectionConfig::default().wide_field().validate();
        StarDetectionConfig::default().high_resolution().validate();
        StarDetectionConfig::default().crowded_field().validate();
        StarDetectionConfig::default().nebulous_field().validate();
        StarDetectionConfig::default().precise_ground().validate();
    }

    #[test]
    fn test_star_detection_config_nebulous_field() {
        let config = StarDetectionConfig::default().nebulous_field();
        assert!(config.background.refinement.adaptive_sigma().is_some());
        config.validate();
    }

    #[test]
    fn test_star_detection_config_custom() {
        let config = StarDetectionConfig {
            psf: PsfConfig {
                expected_fwhm: 5.0,
                ..Default::default()
            },
            filtering: FilteringConfig {
                min_snr: 15.0,
                edge_margin: 20,
                ..Default::default()
            },
            noise_model: Some(NoiseModel::new(1.5, 5.0)),
            background: BackgroundConfig {
                refinement: BackgroundRefinement::AdaptiveSigma(AdaptiveSigmaConfig::default()),
                ..Default::default()
            },
            ..Default::default()
        };

        assert!((config.psf.expected_fwhm - 5.0).abs() < 1e-6);
        assert!((config.filtering.min_snr - 15.0).abs() < 1e-6);
        assert_eq!(config.filtering.edge_margin, 20);
        assert!(config.noise_model.is_some());
        assert!(config.background.refinement.adaptive_sigma().is_some());
        config.validate();
    }

    #[test]
    fn test_star_detection_config_with_adaptive_sigma() {
        let config = StarDetectionConfig {
            background: BackgroundConfig {
                refinement: BackgroundRefinement::AdaptiveSigma(AdaptiveSigmaConfig::default()),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(config.background.refinement.adaptive_sigma().is_some());
        config.validate();
    }

    #[test]
    fn test_star_detection_config_with_custom_adaptive_sigma() {
        let custom = AdaptiveSigmaConfig {
            base_sigma: 2.5,
            max_sigma: 7.0,
            contrast_factor: 1.0,
        };
        let config = StarDetectionConfig {
            background: BackgroundConfig {
                refinement: BackgroundRefinement::AdaptiveSigma(custom),
                ..Default::default()
            },
            ..Default::default()
        };
        let adaptive = config.background.refinement.adaptive_sigma().unwrap();
        assert!((adaptive.base_sigma - 2.5).abs() < 1e-6);
        assert!((adaptive.max_sigma - 7.0).abs() < 1e-6);
        config.validate();
    }

    #[test]
    fn test_star_detection_config_with_auto_fwhm() {
        let config = StarDetectionConfig {
            psf: PsfConfig {
                auto_estimate: true,
                expected_fwhm: 0.0,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(config.psf.auto_estimate);
        assert!((config.psf.expected_fwhm - 0.0).abs() < 1e-6);
        config.validate();
    }

    #[test]
    fn test_star_detection_config_validates_subconfigs() {
        // This tests that validate() calls all sub-config validations
        let config = StarDetectionConfig {
            centroid: CentroidConfig {
                method: CentroidMethod::MoffatFit { beta: 2.5 },
                ..Default::default()
            },
            ..Default::default()
        };
        config.validate(); // Should not panic with valid beta
    }

    #[test]
    fn test_preset_chaining() {
        // Chaining nebulous_field then crowded_field: crowded_field's refinement
        // (Iterative) should override nebulous_field's (AdaptiveSigma),
        // but nebulous_field's tile_size and mask_dilation should survive
        // since crowded_field doesn't set them.
        let config = StarDetectionConfig::default()
            .nebulous_field()
            .crowded_field();

        // From nebulous_field (not overridden by crowded_field)
        assert_eq!(config.background.tile_size, 128);
        assert_eq!(config.background.mask_dilation, 5);

        // From crowded_field (overrides nebulous_field's AdaptiveSigma)
        assert!(matches!(
            config.background.refinement,
            BackgroundRefinement::Iterative { iterations: 2 }
        ));
        assert_eq!(config.deblend.n_thresholds, 32);
        assert_eq!(config.deblend.min_separation, 2);
        assert!((config.deblend.min_prominence - 0.15).abs() < 1e-6);
        assert!((config.deblend.min_contrast - 0.005).abs() < 1e-6);
        assert_eq!(config.filtering.connectivity, Connectivity::Eight);

        config.validate();
    }

    #[test]
    fn test_preset_chaining_reverse_order() {
        // Reverse order: crowded_field then nebulous_field.
        // nebulous_field's AdaptiveSigma should win over Iterative.
        let config = StarDetectionConfig::default()
            .crowded_field()
            .nebulous_field();

        // From crowded_field (not overridden by nebulous_field)
        assert_eq!(config.deblend.n_thresholds, 32);
        assert_eq!(config.deblend.min_separation, 2);
        assert_eq!(config.filtering.connectivity, Connectivity::Eight);

        // From nebulous_field (overrides crowded_field's Iterative)
        assert_eq!(config.background.tile_size, 128);
        assert_eq!(config.background.mask_dilation, 5);
        assert!(config.background.refinement.adaptive_sigma().is_some());

        config.validate();
    }
}
