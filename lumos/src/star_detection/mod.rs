//! Star detection and centroid computation for image registration.
//!
//! This module detects stars in astronomical images and computes sub-pixel
//! accurate centroids for use in image alignment and stacking.
//!
//! # Algorithm Overview
//!
//! 1. **Background estimation**: Divide image into tiles, compute sigma-clipped
//!    median per tile, then bilinearly interpolate to create a smooth background map.
//!
//! 2. **Star detection**: Threshold pixels above background + k×σ, then use
//!    connected component labeling to group pixels into candidate stars.
//!
//! 3. **Filtering**: Reject candidates that are too small, too large, elongated,
//!    near edges, or saturated.
//!
//! 4. **Sub-pixel centroid**: Compute precise centroid using iterative weighted
//!    centroid algorithm (achieves ~0.05 pixel accuracy).
//!
//! 5. **Quality metrics**: Compute FWHM, SNR, and eccentricity for each star.

pub(crate) mod background;
mod centroid;
pub(crate) mod constants;
mod convolution;
mod cosmic_ray;
mod deblend;
mod defect_map;
pub(crate) mod detection;
pub mod gpu;
mod median_filter;

#[cfg(test)]
pub mod tests;

#[cfg(feature = "bench")]
pub mod bench;

#[cfg(feature = "bench")]
pub mod benches {
    pub use super::background::bench as background;
    pub use super::bench as full_pipeline;
    pub use super::centroid::bench as centroid;
    pub use super::convolution::bench as convolution;
    pub use super::cosmic_ray::bench as cosmic_ray;
    pub use super::deblend::bench as deblend;
    pub use super::detection::bench as detection;
    pub use super::median_filter::bench as median_filter;
}

use core::f32;

// Public API exports - main entry points for external consumers
pub use centroid::LocalBackgroundMethod;

// Internal re-exports for advanced users (may change in future versions)
// Background estimation (used by calibration and advanced pipelines)
pub use background::{
    BackgroundMap, IterativeBackgroundConfig, estimate_background, estimate_background_image,
    estimate_background_iterative, estimate_background_iterative_image,
};

// Profile fitting (for custom centroiding pipelines)
pub use centroid::{GaussianFitConfig, GaussianFitResult, fit_gaussian_2d};
pub use centroid::{
    MoffatFitConfig, MoffatFitResult, alpha_beta_to_fwhm, fit_moffat_2d, fwhm_beta_to_alpha,
};

// Low-level detection (for custom pipelines)
pub(crate) use centroid::compute_centroid;
pub(crate) use convolution::{matched_filter, matched_filter_elliptical};
pub(crate) use detection::{detect_stars, detect_stars_filtered};
pub(crate) use median_filter::median_filter_3x3;

use crate::astro_image::AstroImage;

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

/// A detected star with sub-pixel position and quality metrics.
#[derive(Debug, Clone, Copy)]
pub struct Star {
    /// X coordinate (sub-pixel accurate).
    pub x: f32,
    /// Y coordinate (sub-pixel accurate).
    pub y: f32,
    /// Total flux (sum of background-subtracted pixel values).
    pub flux: f32,
    /// Full Width at Half Maximum in pixels.
    pub fwhm: f32,
    /// Eccentricity (0 = circular, 1 = elongated). Used to reject non-stellar objects.
    pub eccentricity: f32,
    /// Signal-to-noise ratio.
    pub snr: f32,
    /// Peak pixel value (for saturation detection).
    pub peak: f32,
    /// Sharpness metric (peak / flux_in_core). Cosmic rays have high sharpness (>0.8),
    /// real stars have lower sharpness (typically 0.2-0.6 depending on seeing).
    pub sharpness: f32,
    /// Roundness based on marginal Gaussian fits (DAOFIND GROUND).
    /// (Hx - Hy) / (Hx + Hy) where Hx, Hy are heights of marginal x and y fits.
    /// Circular sources → 0, x-extended → negative, y-extended → positive.
    pub roundness1: f32,
    /// Roundness based on symmetry (DAOFIND SROUND).
    /// Measures bilateral vs four-fold symmetry. Circular → 0, asymmetric → non-zero.
    pub roundness2: f32,
    /// L.A.Cosmic Laplacian SNR metric for cosmic ray detection.
    /// Cosmic rays have very sharp edges (high Laplacian), stars have smooth edges.
    /// Values > 50 typically indicate cosmic rays. Based on van Dokkum 2001.
    pub laplacian_snr: f32,
}

impl Star {
    /// Check if star is likely saturated.
    ///
    /// Stars with peak values near the maximum (>0.95 for normalized data)
    /// have unreliable centroids.
    pub fn is_saturated(&self) -> bool {
        self.peak > 0.95
    }

    /// Check if star is likely a cosmic ray (very sharp, single-pixel spike).
    ///
    /// Cosmic rays typically have sharpness > 0.7, while real stars are 0.2-0.5.
    pub fn is_cosmic_ray(&self, max_sharpness: f32) -> bool {
        self.sharpness > max_sharpness
    }

    /// Check if star is likely a cosmic ray using L.A.Cosmic Laplacian metric.
    ///
    /// Based on van Dokkum 2001: cosmic rays have very sharp edges that produce
    /// high Laplacian values. Stars, being smoothed by the PSF, have lower values.
    /// Threshold of ~50 is typical for rejecting cosmic rays.
    pub fn is_cosmic_ray_laplacian(&self, max_laplacian_snr: f32) -> bool {
        self.laplacian_snr > max_laplacian_snr
    }

    /// Check if star passes roundness filters.
    ///
    /// Both roundness metrics should be close to zero for circular sources.
    pub fn is_round(&self, max_roundness: f32) -> bool {
        self.roundness1.abs() <= max_roundness && self.roundness2.abs() <= max_roundness
    }

    /// Check if star passes quality filters for registration.
    ///
    /// Filters out saturated, elongated, low-SNR stars, cosmic rays, and non-round objects.
    /// Unlike simple `is_*` predicates, this method combines multiple quality criteria.
    pub fn passes_quality_filters(
        &self,
        min_snr: f32,
        max_eccentricity: f32,
        max_sharpness: f32,
        max_roundness: f32,
    ) -> bool {
        !self.is_saturated()
            && self.snr >= min_snr
            && self.eccentricity <= max_eccentricity
            && !self.is_cosmic_ray(max_sharpness)
            && self.is_round(max_roundness)
    }
}

pub use defect_map::DefectMap;
use defect_map::apply_defect_mask;

/// Detection threshold and area parameters.
#[derive(Debug, Clone)]
pub(crate) struct DetectionParams {
    /// Detection threshold in sigma above background (typically 3.0-5.0).
    pub detection_sigma: f32,
    /// Minimum star area in pixels.
    pub min_area: usize,
    /// Maximum star area in pixels.
    pub max_area: usize,
    /// Edge margin in pixels (stars too close to edge are rejected).
    pub edge_margin: usize,
    /// Expected FWHM of stars in pixels for matched filtering.
    /// Set to 0.0 to disable matched filtering.
    pub expected_fwhm: f32,
    /// Axis ratio for elliptical Gaussian matched filter (minor/major axis).
    /// Value of 1.0 means circular PSF.
    pub psf_axis_ratio: f32,
    /// Position angle of PSF major axis in radians.
    pub psf_angle: f32,
    /// Tile size for background estimation.
    pub background_tile_size: usize,
    /// Number of background estimation passes (0 = single pass, >0 = iterative).
    pub background_passes: usize,
}

impl Default for DetectionParams {
    fn default() -> Self {
        Self {
            detection_sigma: 4.0,
            min_area: 5,
            max_area: 500,
            edge_margin: 10,
            expected_fwhm: 4.0,
            psf_axis_ratio: 1.0,
            psf_angle: 0.0,
            background_tile_size: 64,
            background_passes: 0,
        }
    }
}

/// Quality filter parameters for rejecting spurious detections.
#[derive(Debug, Clone)]
pub(crate) struct QualityFilters {
    /// Maximum eccentricity (0-1, higher = more elongated allowed).
    pub max_eccentricity: f32,
    /// Minimum SNR for a star to be considered valid.
    pub min_snr: f32,
    /// Maximum FWHM deviation from median in MAD units.
    pub max_fwhm_deviation: f32,
    /// Maximum sharpness (cosmic ray rejection).
    pub max_sharpness: f32,
    /// Maximum roundness for a star to be considered valid.
    pub max_roundness: f32,
    /// Minimum separation between stars for duplicate removal.
    pub duplicate_min_separation: f32,
}

impl Default for QualityFilters {
    fn default() -> Self {
        Self {
            max_eccentricity: 0.6,
            min_snr: 10.0,
            max_fwhm_deviation: 3.0,
            max_sharpness: 0.7,
            max_roundness: 1.0,
            duplicate_min_separation: 8.0,
        }
    }
}

/// Camera-specific parameters for accurate SNR calculation.
#[derive(Debug, Clone, Default)]
pub(crate) struct CameraParams {
    /// Camera gain in electrons per ADU (e-/ADU).
    pub gain: Option<f32>,
    /// Read noise in electrons (e-).
    pub read_noise: Option<f32>,
    /// Optional defect map for masking bad pixels.
    pub defect_map: Option<DefectMap>,
}

/// Deblending parameters for separating overlapping stars.
#[derive(Debug, Clone)]
pub(crate) struct DeblendParams {
    /// Minimum separation between peaks for deblending (in pixels).
    pub min_separation: usize,
    /// Minimum peak prominence for deblending (0.0-1.0).
    pub min_prominence: f32,
    /// Enable multi-threshold deblending (SExtractor-style).
    pub multi_threshold: bool,
    /// Number of deblending sub-thresholds.
    pub nthresh: usize,
    /// Minimum contrast for multi-threshold deblending.
    pub min_contrast: f32,
}

impl Default for DeblendParams {
    fn default() -> Self {
        Self {
            min_separation: 3,
            min_prominence: 0.3,
            multi_threshold: false,
            nthresh: 32,
            min_contrast: 0.005,
        }
    }
}

/// Centroiding parameters.
#[derive(Debug, Clone, Default)]
pub(crate) struct CentroidParams {
    /// Method for computing sub-pixel centroids.
    pub method: CentroidMethod,
    /// Method for computing local background during centroid refinement.
    pub local_background: LocalBackgroundMethod,
}

/// Configuration for star detection.
#[derive(Debug, Clone)]
pub struct StarDetectionConfig {
    /// Detection threshold in sigma above background (typically 3.0-5.0).
    pub detection_sigma: f32,
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
    /// Tile size for background estimation.
    pub background_tile_size: usize,
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
    /// Enable multi-threshold deblending (SExtractor-style).
    /// When enabled, uses tree-based deblending with multiple threshold levels
    /// instead of simple local maxima detection. More accurate for crowded fields
    /// but slower. Set to true for better crowded field handling.
    pub multi_threshold_deblend: bool,
    /// Number of deblending sub-thresholds for multi-threshold deblending.
    /// Higher values give finer deblending resolution but use more CPU.
    /// SExtractor default: 32. Typical range: 16-64.
    pub deblend_nthresh: usize,
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
    /// Number of background estimation passes (0 = single pass).
    /// When > 0, the background is re-estimated after masking detected objects,
    /// which improves accuracy in crowded fields. SExtractor-style algorithm.
    /// Typical value: 1-2 for crowded fields, 0 for sparse fields.
    pub background_passes: usize,
    /// Method for computing sub-pixel centroids.
    /// WeightedMoments (default) is fast (~0.05 pixel accuracy).
    /// GaussianFit and MoffatFit provide higher precision (~0.01 pixel) but are slower.
    pub centroid_method: CentroidMethod,
    /// Method for computing local background during centroid refinement.
    /// GlobalMap (default) uses the precomputed background map.
    /// Annulus and OuterRing compute local background around each star,
    /// which is more accurate in regions with variable nebulosity.
    pub local_background_method: LocalBackgroundMethod,
}

impl Default for StarDetectionConfig {
    fn default() -> Self {
        Self {
            detection_sigma: 4.0,
            min_area: 5,
            max_area: 500,
            max_eccentricity: 0.6,
            edge_margin: 10,
            min_snr: 10.0,
            background_tile_size: 64,
            max_fwhm_deviation: 3.0,
            expected_fwhm: 4.0,
            psf_axis_ratio: 1.0, // Circular PSF by default
            psf_angle: 0.0,
            max_sharpness: 0.7,
            deblend_min_separation: 3,
            deblend_min_prominence: 0.3,
            duplicate_min_separation: 8.0,
            max_roundness: 1.0, // Disabled by default (accept all roundness values)
            multi_threshold_deblend: false, // Use simpler local maxima by default
            deblend_nthresh: 32,
            deblend_min_contrast: 0.005,
            gain: None, // Use simplified SNR formula by default
            read_noise: None,
            defect_map: None,
            background_passes: 0, // Single pass by default (fastest)
            centroid_method: CentroidMethod::WeightedMoments,
            local_background_method: LocalBackgroundMethod::GlobalMap,
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
        assert!(
            self.detection_sigma > 0.0,
            "detection_sigma must be positive, got {}",
            self.detection_sigma
        );
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
            (16..=256).contains(&self.background_tile_size),
            "background_tile_size must be in [16, 256], got {}",
            self.background_tile_size
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
            self.deblend_nthresh >= 2,
            "deblend_nthresh must be at least 2, got {}",
            self.deblend_nthresh
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
    }

    /// Create a new builder for constructing StarDetectionConfig.
    pub fn builder() -> StarDetectionConfigBuilder {
        StarDetectionConfigBuilder::default()
    }

    /// Create config from sub-struct parameters.
    pub(crate) fn from_params(
        detection: DetectionParams,
        quality: QualityFilters,
        camera: CameraParams,
        deblend: DeblendParams,
        centroid: CentroidParams,
    ) -> Self {
        Self {
            detection_sigma: detection.detection_sigma,
            min_area: detection.min_area,
            max_area: detection.max_area,
            edge_margin: detection.edge_margin,
            expected_fwhm: detection.expected_fwhm,
            psf_axis_ratio: detection.psf_axis_ratio,
            psf_angle: detection.psf_angle,
            background_tile_size: detection.background_tile_size,
            background_passes: detection.background_passes,
            max_eccentricity: quality.max_eccentricity,
            min_snr: quality.min_snr,
            max_fwhm_deviation: quality.max_fwhm_deviation,
            max_sharpness: quality.max_sharpness,
            max_roundness: quality.max_roundness,
            duplicate_min_separation: quality.duplicate_min_separation,
            gain: camera.gain,
            read_noise: camera.read_noise,
            defect_map: camera.defect_map,
            deblend_min_separation: deblend.min_separation,
            deblend_min_prominence: deblend.min_prominence,
            multi_threshold_deblend: deblend.multi_threshold,
            deblend_nthresh: deblend.nthresh,
            deblend_min_contrast: deblend.min_contrast,
            centroid_method: centroid.method,
            local_background_method: centroid.local_background,
        }
    }
}

/// Builder for StarDetectionConfig with fluent API.
#[derive(Debug, Clone, Default)]
pub struct StarDetectionConfigBuilder {
    detection: DetectionParams,
    quality: QualityFilters,
    camera: CameraParams,
    deblend: DeblendParams,
    centroid: CentroidParams,
}

impl StarDetectionConfigBuilder {
    /// Create a new builder with default parameters.
    pub fn new() -> Self {
        Self::default()
    }

    /// Configure for wide-field imaging (larger stars, relaxed filtering).
    pub fn for_wide_field(mut self) -> Self {
        self.detection.expected_fwhm = 6.0;
        self.detection.max_area = 1000;
        self.quality.max_eccentricity = 0.7;
        self
    }

    /// Configure for high-resolution imaging (smaller stars, stricter filtering).
    pub fn for_high_resolution(mut self) -> Self {
        self.detection.expected_fwhm = 2.5;
        self.detection.max_area = 200;
        self.quality.max_eccentricity = 0.5;
        self.quality.min_snr = 15.0;
        self
    }

    /// Configure for crowded fields (aggressive deblending).
    pub fn for_crowded_field(mut self) -> Self {
        self.deblend.multi_threshold = true;
        self.deblend.min_separation = 2;
        self.detection.background_passes = 2;
        self
    }

    /// Set expected FWHM for matched filtering.
    pub fn with_fwhm(mut self, fwhm: f32) -> Self {
        self.detection.expected_fwhm = fwhm;
        self
    }

    /// Set detection threshold in sigma.
    pub fn with_detection_sigma(mut self, sigma: f32) -> Self {
        self.detection.detection_sigma = sigma;
        self
    }

    /// Set minimum SNR threshold.
    pub fn with_min_snr(mut self, snr: f32) -> Self {
        self.quality.min_snr = snr;
        self
    }

    /// Set edge margin in pixels.
    pub fn with_edge_margin(mut self, margin: usize) -> Self {
        self.detection.edge_margin = margin;
        self
    }

    /// Set maximum eccentricity for star acceptance.
    pub fn with_max_eccentricity(mut self, eccentricity: f32) -> Self {
        self.quality.max_eccentricity = eccentricity;
        self
    }

    /// Enable cosmic ray rejection with specified sharpness threshold.
    pub fn with_cosmic_ray_rejection(mut self, max_sharpness: f32) -> Self {
        self.quality.max_sharpness = max_sharpness;
        self
    }

    /// Set camera noise model for accurate SNR calculation.
    pub fn with_noise_model(mut self, gain: f32, read_noise: f32) -> Self {
        self.camera.gain = Some(gain);
        self.camera.read_noise = Some(read_noise);
        self
    }

    /// Set elliptical PSF parameters.
    pub fn with_elliptical_psf(mut self, axis_ratio: f32, angle: f32) -> Self {
        self.detection.psf_axis_ratio = axis_ratio;
        self.detection.psf_angle = angle;
        self
    }

    /// Set centroid method.
    pub fn with_centroid_method(mut self, method: CentroidMethod) -> Self {
        self.centroid.method = method;
        self
    }

    /// Set local background method for centroiding.
    pub fn with_local_background(mut self, method: LocalBackgroundMethod) -> Self {
        self.centroid.local_background = method;
        self
    }

    /// Enable multi-threshold deblending.
    pub fn with_multi_threshold_deblend(mut self, enable: bool) -> Self {
        self.deblend.multi_threshold = enable;
        self
    }

    /// Build the final StarDetectionConfig.
    pub fn build(self) -> StarDetectionConfig {
        StarDetectionConfig::from_params(
            self.detection,
            self.quality,
            self.camera,
            self.deblend,
            self.centroid,
        )
    }
}

/// Star detector with builder pattern for convenient star detection.
///
/// Wraps [`StarDetectionConfig`] and provides methods for detecting stars
/// in single images or batches.
///
/// # Example
///
/// ```rust,ignore
/// use lumos::{StarDetector, AstroImage};
///
/// // Simple usage with defaults
/// let detector = StarDetector::new();
/// let result = detector.detect(&image);
///
/// // With custom configuration
/// let detector = StarDetector::new()
///     .with_fwhm(4.0)
///     .with_min_snr(15.0)
///     .with_edge_margin(20)
///     .build();
/// let result = detector.detect(&image);
///
/// // Batch detection (parallel)
/// let results = detector.detect_all(&images);
/// ```
#[derive(Debug)]
pub struct StarDetector {
    config: StarDetectionConfig,
}

impl Default for StarDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl StarDetector {
    /// Create a new star detector with default configuration.
    pub fn new() -> Self {
        Self {
            config: StarDetectionConfig::default(),
            bump: bumpalo::Bump::new(),
        }
    }

    /// Create a star detector from an existing configuration.
    pub fn from_config(config: StarDetectionConfig) -> Self {
        Self {
            config,
            ..Default::default()
        }
    }

    /// Configure for wide-field imaging (larger stars, relaxed filtering).
    #[must_use]
    pub fn for_wide_field(mut self) -> Self {
        let builder = StarDetectionConfigBuilder::default().for_wide_field();
        self.config = builder.build();
        self
    }

    /// Configure for high-resolution imaging (smaller stars, stricter filtering).
    #[must_use]
    pub fn for_high_resolution(mut self) -> Self {
        let builder = StarDetectionConfigBuilder::default().for_high_resolution();
        self.config = builder.build();
        self
    }

    /// Configure for crowded fields (aggressive deblending).
    #[must_use]
    pub fn for_crowded_field(mut self) -> Self {
        let builder = StarDetectionConfigBuilder::default().for_crowded_field();
        self.config = builder.build();
        self
    }

    /// Set expected FWHM for matched filtering.
    #[must_use]
    pub fn with_fwhm(mut self, fwhm: f32) -> Self {
        self.config.expected_fwhm = fwhm;
        self
    }

    /// Set detection threshold in sigma.
    #[must_use]
    pub fn with_detection_sigma(mut self, sigma: f32) -> Self {
        self.config.detection_sigma = sigma;
        self
    }

    /// Set minimum SNR threshold.
    #[must_use]
    pub fn with_min_snr(mut self, snr: f32) -> Self {
        self.config.min_snr = snr;
        self
    }

    /// Set edge margin in pixels.
    #[must_use]
    pub fn with_edge_margin(mut self, margin: usize) -> Self {
        self.config.edge_margin = margin;
        self
    }

    /// Set maximum eccentricity for star acceptance.
    #[must_use]
    pub fn with_max_eccentricity(mut self, eccentricity: f32) -> Self {
        self.config.max_eccentricity = eccentricity;
        self
    }

    /// Enable cosmic ray rejection with specified sharpness threshold.
    #[must_use]
    pub fn with_cosmic_ray_rejection(mut self, max_sharpness: f32) -> Self {
        self.config.max_sharpness = max_sharpness;
        self
    }

    /// Set camera noise model for accurate SNR calculation.
    #[must_use]
    pub fn with_noise_model(mut self, gain: f32, read_noise: f32) -> Self {
        self.config.gain = Some(gain);
        self.config.read_noise = Some(read_noise);
        self
    }

    /// Set elliptical PSF parameters.
    #[must_use]
    pub fn with_elliptical_psf(mut self, axis_ratio: f32, angle: f32) -> Self {
        self.config.psf_axis_ratio = axis_ratio;
        self.config.psf_angle = angle;
        self
    }

    /// Set centroid method.
    #[must_use]
    pub fn with_centroid_method(mut self, method: CentroidMethod) -> Self {
        self.config.centroid_method = method;
        self
    }

    /// Set local background method for centroiding.
    #[must_use]
    pub fn with_local_background(mut self, method: LocalBackgroundMethod) -> Self {
        self.config.local_background_method = method;
        self
    }

    /// Enable multi-threshold deblending.
    #[must_use]
    pub fn with_multi_threshold_deblend(mut self, enable: bool) -> Self {
        self.config.multi_threshold_deblend = enable;
        self
    }

    /// Finalize configuration (optional, detector is usable without calling this).
    #[must_use]
    pub fn build(self) -> Self {
        self.config.validate();
        self
    }

    /// Get reference to the underlying configuration.
    pub fn config(&self) -> &StarDetectionConfig {
        &self.config
    }

    /// Detect stars in a single image.
    pub fn detect(&self, image: &AstroImage) -> StarDetectionResult {
        find_stars(image, &self.config)
    }

    /// Detect stars in multiple images in parallel.
    ///
    /// Returns results in the same order as the input images.
    pub fn detect_all(&self, images: &[AstroImage]) -> Vec<StarDetectionResult> {
        use rayon::prelude::*;
        let config = self.config.clone();

        images
            .par_iter()
            .map(|image| find_stars(image, &config))
            .collect()
    }
}

/// Result of star detection with diagnostics.
#[derive(Debug, Clone)]
pub struct StarDetectionResult {
    /// Detected stars sorted by flux (brightest first).
    pub stars: Vec<Star>,
    /// Diagnostic information from the detection pipeline.
    pub diagnostics: StarDetectionDiagnostics,
}

/// Diagnostic information from star detection.
///
/// Contains statistics and counts from each stage of the detection pipeline
/// for debugging and tuning purposes.
#[derive(Debug, Clone, Default)]
pub struct StarDetectionDiagnostics {
    /// Number of pixels above detection threshold.
    pub pixels_above_threshold: usize,
    /// Number of connected components found.
    pub connected_components: usize,
    /// Number of candidates after size/edge filtering.
    pub candidates_after_filtering: usize,
    /// Number of candidates that were deblended into multiple stars.
    pub deblended_components: usize,
    /// Number of stars after centroid computation (before quality filtering).
    pub stars_after_centroid: usize,
    /// Number of stars rejected for low SNR.
    pub rejected_low_snr: usize,
    /// Number of stars rejected for high eccentricity.
    pub rejected_high_eccentricity: usize,
    /// Number of stars rejected as cosmic rays (high sharpness).
    pub rejected_cosmic_rays: usize,
    /// Number of stars rejected as saturated.
    pub rejected_saturated: usize,
    /// Number of stars rejected for non-circular shape (roundness).
    pub rejected_roundness: usize,
    /// Number of stars rejected for abnormal FWHM.
    pub rejected_fwhm_outliers: usize,
    /// Number of duplicate detections removed.
    pub rejected_duplicates: usize,
    /// Final number of stars returned.
    pub final_star_count: usize,
    /// Median FWHM of detected stars (pixels).
    pub median_fwhm: f32,
    /// Median SNR of detected stars.
    pub median_snr: f32,
}

/// Detect stars in an astronomical image.
///
/// Returns detected stars sorted by flux (brightest first) along with
/// diagnostic information from the detection pipeline.
///
/// For RGB images, only the first channel (red) is used. For grayscale
/// images, the single channel is used directly.
///
/// # Arguments
/// * `image` - Astronomical image (grayscale or RGB, normalized 0.0-1.0)
/// * `config` - Detection configuration
fn find_stars(image: &AstroImage, config: &StarDetectionConfig) -> StarDetectionResult {
    config.validate();

    let width = image.width();
    let height = image.height();
    let mut pixels = image.to_grayscale_pixels();
    let mut output = vec![0.0f32; pixels.len()];

    let mut diagnostics = StarDetectionDiagnostics::default();

    // Step 0a: Apply defect mask if provided
    if config.defect_map.as_ref().is_some_and(|m| !m.is_empty()) {
        apply_defect_mask(
            &pixels,
            width,
            height,
            config.defect_map.as_ref().unwrap(),
            &mut output,
        );
        std::mem::swap(&mut pixels, &mut output);
    }

    // Step 0b: Apply 3x3 median filter to remove Bayer pattern artifacts
    // Only applied for CFA sensors; skip for monochrome (~6ms faster on 4K images)
    if image.metadata.is_cfa {
        median_filter_3x3(&pixels, width, height, &mut output);
        std::mem::swap(&mut pixels, &mut output);
    }

    drop(output);

    // Step 1: Estimate background
    let background = {
        if config.background_passes > 0 {
            // Use iterative background estimation for crowded fields
            let iter_config = IterativeBackgroundConfig {
                iterations: config.background_passes,
                detection_sigma: config.detection_sigma,
                ..IterativeBackgroundConfig::default()
            };
            estimate_background_iterative(
                &pixels,
                width,
                height,
                config.background_tile_size,
                &iter_config,
            )
        } else {
            // Single-pass background estimation (faster)
            estimate_background(&pixels, width, height, config.background_tile_size)
        }
    };

    // Step 2: Detect star candidates
    let candidates = {
        if config.expected_fwhm > f32::EPSILON {
            // Apply matched filter (Gaussian convolution) for better faint star detection
            // This is the DAOFIND/SExtractor technique
            let filtered = if config.psf_axis_ratio < 0.99 {
                tracing::debug!(
                    "Applying elliptical matched filter with FWHM={:.1}, axis_ratio={:.2}, angle={:.2}",
                    config.expected_fwhm,
                    config.psf_axis_ratio,
                    config.psf_angle
                );
                matched_filter_elliptical(
                    &pixels,
                    width,
                    height,
                    &background.background,
                    config.expected_fwhm,
                    config.psf_axis_ratio,
                    config.psf_angle,
                )
            } else {
                tracing::debug!(
                    "Applying matched filter with FWHM={:.1} pixels",
                    config.expected_fwhm
                );
                matched_filter(
                    &pixels,
                    width,
                    height,
                    &background.background,
                    config.expected_fwhm,
                )
            };
            detect_stars_filtered(&pixels, &filtered, width, height, &background, &config)
        } else {
            // No matched filter - use standard detection
            detect_stars(&pixels, width, height, &background, &config)
        }
    };
    diagnostics.candidates_after_filtering = candidates.len();
    tracing::debug!("Detected {} star candidates", candidates.len());

    // Step 3: Compute precise centroids
    let stars_after_centroid: Vec<Star> = {
        candidates
            .into_iter()
            .filter_map(|candidate| {
                compute_centroid(&pixels, width, height, &background, &candidate, &config)
            })
            .collect()
    };
    diagnostics.stars_after_centroid = stars_after_centroid.len();

    // Step 4: Apply quality filters and count rejections
    let mut stars = {
        let mut stars = stars_after_centroid;
        stars.retain(|star| {
            if star.is_saturated() {
                diagnostics.rejected_saturated += 1;
                false
            } else if star.snr < config.min_snr {
                diagnostics.rejected_low_snr += 1;
                false
            } else if star.eccentricity > config.max_eccentricity {
                diagnostics.rejected_high_eccentricity += 1;
                false
            } else if star.is_cosmic_ray(config.max_sharpness) {
                diagnostics.rejected_cosmic_rays += 1;
                false
            } else if !star.is_round(config.max_roundness) {
                diagnostics.rejected_roundness += 1;
                false
            } else {
                true
            }
        });
        stars
    };

    // Sort by flux (brightest first)
    stars.sort_by(|a, b| {
        b.flux
            .partial_cmp(&a.flux)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Filter FWHM outliers - spurious detections often have abnormally large FWHM
    let removed = filter_fwhm_outliers(&mut stars, config.max_fwhm_deviation);
    diagnostics.rejected_fwhm_outliers = removed;
    if removed > 0 {
        tracing::debug!("Removed {} stars with abnormally large FWHM", removed);
    }

    // Remove duplicate detections - keep only the brightest star within min_separation pixels
    let removed = remove_duplicate_stars(&mut stars, config.duplicate_min_separation);
    diagnostics.rejected_duplicates = removed;
    if removed > 0 {
        tracing::debug!("Removed {} duplicate star detections", removed);
    }

    // Compute final statistics
    diagnostics.final_star_count = stars.len();

    if !stars.is_empty() {
        let mut buf: Vec<f32> = stars.iter().map(|s| s.fwhm).collect();
        diagnostics.median_fwhm = crate::math::median_f32_mut(&mut buf);

        buf.clear();
        buf.extend(stars.iter().map(|s| s.snr));
        diagnostics.median_snr = crate::math::median_f32_mut(&mut buf);
    }

    StarDetectionResult { stars, diagnostics }
}

/// Remove duplicate star detections that are too close together.
///
/// Keeps the brightest star (by flux) within `min_separation` pixels of each other.
/// Stars must be sorted by flux (brightest first) before calling.
/// Returns the number of stars removed.
fn remove_duplicate_stars(stars: &mut Vec<Star>, min_separation: f32) -> usize {
    if stars.len() < 2 {
        return 0;
    }

    let min_sep_sq = min_separation * min_separation;
    let mut kept = vec![true; stars.len()];

    for i in 0..stars.len() {
        if !kept[i] {
            continue;
        }
        for j in (i + 1)..stars.len() {
            if !kept[j] {
                continue;
            }
            let dx = stars[i].x - stars[j].x;
            let dy = stars[i].y - stars[j].y;
            if dx * dx + dy * dy < min_sep_sq {
                // Keep i (higher flux since sorted), mark j for removal
                kept[j] = false;
            }
        }
    }

    let removed_count = kept.iter().filter(|&&k| !k).count();

    // Filter in place
    let mut write_idx = 0;
    for read_idx in 0..stars.len() {
        if kept[read_idx] {
            if write_idx != read_idx {
                stars[write_idx] = stars[read_idx];
            }
            write_idx += 1;
        }
    }
    stars.truncate(write_idx);

    removed_count
}

/// Compute median and MAD (median absolute deviation) for FWHM filtering.
///
/// Returns (median, mad) computed from the given FWHM values.
fn compute_fwhm_median_mad(fwhms: Vec<f32>) -> (f32, f32) {
    assert!(!fwhms.is_empty(), "Need at least one FWHM value");

    let mut sorted: Vec<f32> = fwhms;
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = sorted[sorted.len() / 2];

    let mut deviations: Vec<f32> = sorted.iter().map(|&f| (f - median).abs()).collect();
    deviations.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mad = deviations[deviations.len() / 2];

    (median, mad)
}

/// Filter stars by FWHM using MAD-based outlier detection.
///
/// Removes stars with FWHM > median + max_deviation * effective_mad.
/// The effective_mad is max(mad, median * 0.1) to handle uniform FWHM.
///
/// Stars should be sorted by flux (brightest first) before calling.
/// Returns the number of stars removed.
fn filter_fwhm_outliers(stars: &mut Vec<Star>, max_deviation: f32) -> usize {
    if max_deviation <= 0.0 || stars.len() < 5 {
        return 0;
    }

    // Use top half for robust median/MAD estimate
    let reference_count = (stars.len() / 2).max(5).min(stars.len());
    let fwhms: Vec<f32> = stars.iter().take(reference_count).map(|s| s.fwhm).collect();
    let (median_fwhm, mad) = compute_fwhm_median_mad(fwhms);

    // Use at least 10% of median as minimum MAD
    let effective_mad = mad.max(median_fwhm * 0.1);
    let max_fwhm = median_fwhm + max_deviation * effective_mad;

    let before_count = stars.len();
    stars.retain(|s| s.fwhm <= max_fwhm);
    before_count - stars.len()
}
