//! Configuration types for star detection.

use super::CentroidMethod;
use super::centroid::LocalBackgroundMethod;
use super::defect_map::DefectMap;

// ============================================================================
// Connectivity
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

// ============================================================================
// Adaptive Threshold Configuration
// ============================================================================

/// Configuration for adaptive local thresholding.
///
/// Adaptive thresholding adjusts the detection sigma based on local image
/// characteristics. In high-contrast regions (nebulosity, gradients), the
/// sigma is increased to reduce false positives. In uniform sky regions,
/// the base sigma is used for maximum sensitivity.
#[derive(Debug, Clone, Copy)]
pub struct AdaptiveThresholdConfig {
    /// Base sigma threshold used in low-contrast (uniform sky) regions.
    /// Default: 3.5
    pub base_sigma: f32,

    /// Maximum sigma threshold used in high-contrast (nebulous) regions.
    /// Default: 6.0
    pub max_sigma: f32,

    /// Contrast sensitivity factor (higher = more sensitive to contrast).
    /// Controls how quickly sigma increases with local contrast.
    /// Default: 2.0
    pub contrast_factor: f32,
}

impl Default for AdaptiveThresholdConfig {
    fn default() -> Self {
        Self {
            base_sigma: 3.5,
            max_sigma: 6.0,
            contrast_factor: 2.0,
        }
    }
}

impl AdaptiveThresholdConfig {
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
// Background Configuration
// ============================================================================

/// Configuration for iterative background refinement.
#[derive(Debug, Clone)]
pub struct BackgroundConfig {
    /// Detection threshold in sigma above background for masking objects.
    /// Higher values = more conservative masking (only mask very bright objects).
    /// Typical value: 3.0-5.0
    pub sigma_threshold: f32,
    /// Number of refinement iterations. Usually 1-2 is sufficient.
    pub iterations: usize,
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
    /// SExtractor/Photutils use 5-10 with standard deviation.
    /// Typical value: 2-5
    pub sigma_clip_iterations: usize,
}

impl Default for BackgroundConfig {
    fn default() -> Self {
        Self {
            sigma_threshold: 4.0,
            iterations: 0,
            mask_dilation: 3,
            min_unmasked_fraction: 0.3,
            tile_size: 64,
            sigma_clip_iterations: 5,
        }
    }
}

impl BackgroundConfig {
    /// Validate the configuration and panic if invalid.
    ///
    /// # Panics
    /// Panics with a descriptive message if any parameter is out of valid range.
    pub fn validate(&self) {
        assert!(
            self.sigma_threshold > 0.0,
            "detection_sigma must be positive, got {}",
            self.sigma_threshold
        );
        assert!(
            self.iterations <= 10,
            "iterations must be <= 10, got {}",
            self.iterations
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
            "Tile size must be between 16 and 256"
        );
        assert!(
            self.sigma_clip_iterations <= 10,
            "sigma_clip_iterations must be <= 10, got {}",
            self.sigma_clip_iterations
        );
    }
}

// ============================================================================
// Detection Configuration
// ============================================================================

/// Configuration for the detection algorithm.
#[derive(Debug, Clone)]
pub struct DetectionConfig {
    /// Detection threshold in sigma above background.
    pub sigma_threshold: f32,
    /// Minimum area in pixels.
    pub min_area: usize,
    /// Edge margin (reject candidates near edges).
    pub edge_margin: usize,
    /// Pixel connectivity for connected component labeling.
    pub connectivity: Connectivity,
}

impl From<&StarDetectionConfig> for DetectionConfig {
    fn from(config: &StarDetectionConfig) -> Self {
        Self {
            sigma_threshold: config.background_config.sigma_threshold,
            min_area: config.min_area,
            edge_margin: config.edge_margin,
            connectivity: config.connectivity,
        }
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
    /// Maximum component area to process (in pixels).
    /// Components larger than this are skipped to avoid expensive processing
    /// of pathologically large regions. Also serves as a sanity check since
    /// very large components are unlikely to be real stars.
    pub max_area: usize,
}

impl DeblendConfig {
    /// Returns true if multi-threshold deblending is enabled.
    /// Multi-threshold is enabled when n_thresholds > 0.
    #[inline]
    pub const fn is_multi_threshold(&self) -> bool {
        self.n_thresholds > 0
    }
}

impl Default for DeblendConfig {
    fn default() -> Self {
        Self {
            min_separation: 3,
            min_prominence: 0.3,
            n_thresholds: 0,
            min_contrast: 0.005,
            max_area: 10_000,
        }
    }
}

impl From<&StarDetectionConfig> for DeblendConfig {
    fn from(config: &StarDetectionConfig) -> Self {
        Self {
            min_separation: config.deblend_min_separation,
            min_prominence: config.deblend_min_prominence,
            n_thresholds: config.deblend_n_thresh,
            min_contrast: config.deblend_min_contrast,
            max_area: config.max_area,
        }
    }
}

// ============================================================================
// Star Detection Configuration
// ============================================================================

/// Configuration for star detection.
#[derive(Debug, Clone)]
pub struct StarDetectionConfig {
    /// Minimum star area in pixels.
    pub min_area: usize,
    /// Maximum star area in pixels.
    pub max_area: usize,
    /// Maximum eccentricity (0-1, higher = more elongated allowed).
    pub max_eccentricity: f32,
    /// Edge margin in pixels (stars too close to edge are rejected).
    pub edge_margin: usize,
    /// Minimum SNR for a star to be considered valid.
    pub min_snr: f32,
    /// Maximum FWHM deviation from median in MAD (median absolute deviation) units.
    /// Stars with FWHM > median + max_fwhm_deviation * MAD are rejected as spurious.
    /// Typical value is 3.0-5.0 (similar to sigma clipping). Set to 0.0 to disable.
    pub max_fwhm_deviation: f32,
    /// Expected FWHM of stars in pixels for matched filtering.
    /// The matched filter (Gaussian convolution) dramatically improves detection of
    /// faint stars by boosting SNR. Set to 0.0 to disable matched filtering.
    /// Typical values are 2.0-6.0 pixels depending on seeing and sampling.
    pub expected_fwhm: f32,
    /// Axis ratio for elliptical Gaussian matched filter (minor/major axis).
    /// Value of 1.0 means circular PSF (default), smaller values mean more elongated.
    /// Useful for tracking errors, field rotation, or optical aberrations.
    /// Must be in range (0, 1].
    pub psf_axis_ratio: f32,
    /// Position angle of PSF major axis in radians (0 = along x-axis).
    /// Only used when psf_axis_ratio < 1.0.
    pub psf_angle: f32,
    /// Maximum sharpness for a star to be considered valid.
    /// Sharpness = peak_value / flux_in_3x3_core. Cosmic rays have very high sharpness
    /// (>0.7) because most flux is in a single pixel. Real stars spread flux across
    /// multiple pixels due to PSF, giving sharpness 0.2-0.5. Set to 1.0 to disable.
    pub max_sharpness: f32,
    /// Minimum separation between peaks for deblending star pairs (in pixels).
    /// Peaks closer than this are merged. Set to 0 to disable deblending.
    pub deblend_min_separation: usize,
    /// Minimum peak prominence for deblending (0.0-1.0).
    /// Secondary peaks must be at least this fraction of the primary peak to be
    /// considered for deblending. Prevents noise spikes from causing false splits.
    pub deblend_min_prominence: f32,
    /// Minimum separation between stars for duplicate removal (in pixels).
    /// Stars closer than this are considered duplicates; only brightest is kept.
    pub duplicate_min_separation: f32,
    /// Maximum roundness for a star to be considered valid.
    /// Roundness metrics (GROUND and SROUND from DAOFIND) measure asymmetry.
    /// Circular sources have roundness near 0. Cosmic rays, satellite trails,
    /// and galaxies have higher absolute roundness. Set to 1.0 to disable.
    pub max_roundness: f32,
    /// Number of deblending sub-thresholds for multi-threshold deblending.
    /// Set to 0 for simple local-maxima deblending (faster, default).
    /// Set to 32+ for SExtractor-style tree-based deblending (more accurate for crowded fields).
    /// Typical range when enabled: 16-64.
    pub deblend_n_thresh: usize,
    /// Minimum contrast for multi-threshold deblending (0.0-1.0).
    /// A branch is considered a separate object only if its flux is
    /// at least this fraction of the total flux. Lower values deblend more aggressively.
    /// SExtractor default: 0.005. Set to 1.0 to disable deblending.
    pub deblend_min_contrast: f32,
    /// Camera gain in electrons per ADU (e-/ADU).
    /// Used for accurate SNR calculation using the full CCD noise equation.
    /// When None, uses simplified background-dominated SNR formula.
    /// Typical values: 0.5-4.0 e-/ADU for modern CMOS sensors.
    pub gain: Option<f32>,
    /// Read noise in electrons (e-).
    /// Used for accurate SNR calculation, especially important for short exposures.
    /// When None, read noise is ignored in SNR calculation.
    /// Typical values: 1-10 e- for modern CMOS sensors.
    pub read_noise: Option<f32>,
    /// Optional defect map for masking bad pixels.
    /// When provided, defective pixels are replaced with local median before detection,
    /// and stars with centroids near defects are flagged.
    pub defect_map: Option<DefectMap>,
    /// Method for computing sub-pixel centroids.
    /// WeightedMoments (default) is fast (~0.05 pixel accuracy).
    /// GaussianFit and MoffatFit provide higher precision (~0.01 pixel) but are slower.
    pub centroid_method: CentroidMethod,
    /// Method for computing local background during centroid refinement.
    /// GlobalMap (default) uses the precomputed background map.
    /// Annulus and OuterRing compute local background around each star,
    /// which is more accurate in regions with variable nebulosity.
    pub local_background_method: LocalBackgroundMethod,
    /// Pixel connectivity for connected component labeling.
    /// Four (default): only horizontal/vertical neighbors are connected.
    /// Eight: diagonal neighbors are also connected (better for undersampled PSFs).
    pub connectivity: Connectivity,

    /// Background estimation configuration.
    pub background_config: BackgroundConfig,

    /// Adaptive threshold configuration.
    /// When Some, uses per-pixel adaptive sigma thresholds based on local contrast.
    /// When None, uses the global sigma_threshold from background_config.
    pub adaptive_threshold: Option<AdaptiveThresholdConfig>,
}

impl Default for StarDetectionConfig {
    fn default() -> Self {
        Self {
            min_area: 5,
            max_area: 500,
            max_eccentricity: 0.6,
            edge_margin: 10,
            min_snr: 10.0,
            max_fwhm_deviation: 3.0,
            expected_fwhm: 4.0,
            psf_axis_ratio: 1.0,
            psf_angle: 0.0,
            max_sharpness: 0.7,
            deblend_min_separation: 3,
            deblend_min_prominence: 0.3,
            duplicate_min_separation: 8.0,
            max_roundness: 1.0,
            deblend_n_thresh: 0,
            deblend_min_contrast: 0.005,
            gain: None,
            read_noise: None,
            defect_map: None,
            centroid_method: CentroidMethod::WeightedMoments,
            local_background_method: LocalBackgroundMethod::GlobalMap,
            connectivity: Connectivity::Four,
            background_config: BackgroundConfig::default(),
            adaptive_threshold: None, // Disabled by default for backward compatibility
        }
    }
}

impl StarDetectionConfig {
    /// Validate the configuration and panic if invalid.
    ///
    /// This is called automatically by `find_stars()` but can be called
    /// manually to check configuration before processing.
    ///
    /// # Panics
    /// Panics with a descriptive message if any parameter is out of valid range.
    pub fn validate(&self) {
        self.background_config.validate();

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
            (0.0..=1.0).contains(&self.max_eccentricity),
            "max_eccentricity must be in [0, 1], got {}",
            self.max_eccentricity
        );
        assert!(
            self.min_snr > 0.0,
            "min_snr must be positive, got {}",
            self.min_snr
        );
        assert!(
            self.expected_fwhm >= 0.0,
            "expected_fwhm must be non-negative (0.0 disables matched filter), got {}",
            self.expected_fwhm
        );
        assert!(
            (0.0..=1.0).contains(&self.psf_axis_ratio),
            "psf_axis_ratio must be in (0, 1], got {}",
            self.psf_axis_ratio
        );
        assert!(
            (0.0..=1.0).contains(&self.max_sharpness),
            "max_sharpness must be in [0, 1], got {}",
            self.max_sharpness
        );
        assert!(
            (0.0..=1.0).contains(&self.deblend_min_prominence),
            "deblend_min_prominence must be in [0, 1], got {}",
            self.deblend_min_prominence
        );
        assert!(
            self.duplicate_min_separation >= 0.0,
            "duplicate_min_separation must be non-negative, got {}",
            self.duplicate_min_separation
        );
        assert!(
            (0.0..=1.0).contains(&self.max_roundness),
            "max_roundness must be in [0, 1], got {}",
            self.max_roundness
        );
        assert!(
            self.deblend_n_thresh == 0 || self.deblend_n_thresh >= 2,
            "deblend_n_thresh must be 0 (disabled) or at least 2, got {}",
            self.deblend_n_thresh
        );
        assert!(
            (0.0..=1.0).contains(&self.deblend_min_contrast),
            "deblend_min_contrast must be in [0, 1], got {}",
            self.deblend_min_contrast
        );
        if let Some(gain) = self.gain {
            assert!(gain > 0.0, "gain must be positive, got {}", gain);
        }
        if let Some(read_noise) = self.read_noise {
            assert!(
                read_noise >= 0.0,
                "read_noise must be non-negative, got {}",
                read_noise
            );
        }
        if let Some(ref adaptive) = self.adaptive_threshold {
            adaptive.validate();
        }
    }

    /// Create config for wide-field imaging (larger stars, relaxed filtering).
    pub fn for_wide_field() -> Self {
        Self {
            expected_fwhm: 6.0,
            max_area: 1000,
            max_eccentricity: 0.7,
            ..Default::default()
        }
    }

    /// Create config for high-resolution imaging (smaller stars, stricter filtering).
    pub fn for_high_resolution() -> Self {
        Self {
            expected_fwhm: 2.5,
            max_area: 200,
            max_eccentricity: 0.5,
            min_snr: 15.0,
            ..Default::default()
        }
    }

    /// Create config for crowded fields (aggressive deblending).
    pub fn for_crowded_field() -> Self {
        Self {
            deblend_n_thresh: 32,
            deblend_min_separation: 2,
            background_config: BackgroundConfig {
                iterations: 2,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Create config for nebulous fields (adaptive thresholding enabled).
    ///
    /// Uses adaptive sigma thresholds that are higher in nebulous regions
    /// to reduce false positives while maintaining sensitivity in clear sky.
    pub fn for_nebulous_field() -> Self {
        Self {
            adaptive_threshold: Some(AdaptiveThresholdConfig::default()),
            background_config: BackgroundConfig {
                iterations: 1, // Refinement helps with nebulosity
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Set expected FWHM for matched filtering.
    #[must_use]
    pub fn with_fwhm(mut self, fwhm: f32) -> Self {
        self.expected_fwhm = fwhm;
        self
    }

    /// Set minimum SNR threshold.
    #[must_use]
    pub fn with_min_snr(mut self, snr: f32) -> Self {
        self.min_snr = snr;
        self
    }

    /// Set edge margin in pixels.
    #[must_use]
    pub fn with_edge_margin(mut self, margin: usize) -> Self {
        self.edge_margin = margin;
        self
    }

    /// Set camera noise model for accurate SNR calculation.
    #[must_use]
    pub fn with_noise_model(mut self, gain: f32, read_noise: f32) -> Self {
        self.gain = Some(gain);
        self.read_noise = Some(read_noise);
        self
    }

    /// Enable adaptive thresholding with default configuration.
    ///
    /// Adaptive thresholding adjusts the detection sigma based on local
    /// image characteristics, reducing false positives in nebulous regions.
    #[must_use]
    pub fn with_adaptive_threshold(mut self) -> Self {
        self.adaptive_threshold = Some(AdaptiveThresholdConfig::default());
        self
    }

    /// Enable adaptive thresholding with custom configuration.
    #[must_use]
    pub fn with_adaptive_threshold_config(mut self, config: AdaptiveThresholdConfig) -> Self {
        self.adaptive_threshold = Some(config);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // AdaptiveThresholdConfig Tests
    // =========================================================================

    #[test]
    fn test_adaptive_threshold_config_default() {
        let config = AdaptiveThresholdConfig::default();

        assert!((config.base_sigma - 3.5).abs() < 1e-6);
        assert!((config.max_sigma - 6.0).abs() < 1e-6);
        assert!((config.contrast_factor - 2.0).abs() < 1e-6);
    }

    #[test]
    fn test_adaptive_threshold_config_conservative() {
        let config = AdaptiveThresholdConfig::conservative();

        assert!((config.base_sigma - 4.0).abs() < 1e-6);
        assert!((config.max_sigma - 8.0).abs() < 1e-6);
        assert!((config.contrast_factor - 3.0).abs() < 1e-6);
    }

    #[test]
    fn test_adaptive_threshold_config_aggressive() {
        let config = AdaptiveThresholdConfig::aggressive();

        assert!((config.base_sigma - 3.0).abs() < 1e-6);
        assert!((config.max_sigma - 5.0).abs() < 1e-6);
        assert!((config.contrast_factor - 1.5).abs() < 1e-6);
    }

    #[test]
    fn test_adaptive_threshold_config_validate_valid() {
        let config = AdaptiveThresholdConfig::default();
        config.validate(); // Should not panic
    }

    #[test]
    #[should_panic(expected = "base_sigma must be positive")]
    fn test_adaptive_threshold_config_validate_zero_base_sigma() {
        let config = AdaptiveThresholdConfig {
            base_sigma: 0.0,
            ..Default::default()
        };
        config.validate();
    }

    #[test]
    #[should_panic(expected = "base_sigma must be positive")]
    fn test_adaptive_threshold_config_validate_negative_base_sigma() {
        let config = AdaptiveThresholdConfig {
            base_sigma: -1.0,
            ..Default::default()
        };
        config.validate();
    }

    #[test]
    #[should_panic(expected = "max_sigma")]
    fn test_adaptive_threshold_config_validate_max_less_than_base() {
        let config = AdaptiveThresholdConfig {
            base_sigma: 5.0,
            max_sigma: 3.0,
            ..Default::default()
        };
        config.validate();
    }

    #[test]
    #[should_panic(expected = "contrast_factor must be positive")]
    fn test_adaptive_threshold_config_validate_zero_contrast_factor() {
        let config = AdaptiveThresholdConfig {
            contrast_factor: 0.0,
            ..Default::default()
        };
        config.validate();
    }

    // =========================================================================
    // StarDetectionConfig Adaptive Threshold Tests
    // =========================================================================

    #[test]
    fn test_star_detection_config_default_no_adaptive() {
        let config = StarDetectionConfig::default();
        assert!(config.adaptive_threshold.is_none());
    }

    #[test]
    fn test_star_detection_config_for_nebulous_field() {
        let config = StarDetectionConfig::for_nebulous_field();
        assert!(config.adaptive_threshold.is_some());
    }

    #[test]
    fn test_star_detection_config_with_adaptive_threshold() {
        let config = StarDetectionConfig::default().with_adaptive_threshold();
        assert!(config.adaptive_threshold.is_some());
        let adaptive = config.adaptive_threshold.unwrap();
        assert!((adaptive.base_sigma - 3.5).abs() < 1e-6);
    }

    #[test]
    fn test_star_detection_config_with_adaptive_threshold_config() {
        let custom = AdaptiveThresholdConfig {
            base_sigma: 2.5,
            max_sigma: 7.0,
            contrast_factor: 1.0,
        };
        let config = StarDetectionConfig::default().with_adaptive_threshold_config(custom);

        assert!(config.adaptive_threshold.is_some());
        let adaptive = config.adaptive_threshold.unwrap();
        assert!((adaptive.base_sigma - 2.5).abs() < 1e-6);
        assert!((adaptive.max_sigma - 7.0).abs() < 1e-6);
        assert!((adaptive.contrast_factor - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_star_detection_config_validate_with_adaptive() {
        let config = StarDetectionConfig::for_nebulous_field();
        config.validate(); // Should not panic
    }

    #[test]
    #[should_panic(expected = "base_sigma must be positive")]
    fn test_star_detection_config_validate_invalid_adaptive() {
        let config = StarDetectionConfig::default().with_adaptive_threshold_config(
            AdaptiveThresholdConfig {
                base_sigma: -1.0,
                ..Default::default()
            },
        );
        config.validate();
    }
}
