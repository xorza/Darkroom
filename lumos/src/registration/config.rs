//! Configuration types for the registration module.
//!
//! All configuration structs and related enums for the registration pipeline
//! are consolidated here. Individual submodules re-export the types they need.

use crate::registration::transform::TransformType;

// =============================================================================
// Interpolation configuration
// =============================================================================

/// Interpolation method for image resampling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InterpolationMethod {
    /// Nearest neighbor - fastest, lowest quality
    Nearest,
    /// Bilinear interpolation - fast, reasonable quality
    Bilinear,
    /// Bicubic interpolation - good quality
    Bicubic,
    /// Lanczos-2 (4x4 kernel) - high quality
    Lanczos2,
    /// Lanczos-3 (6x6 kernel) - highest quality, default
    #[default]
    Lanczos3,
    /// Lanczos-4 (8x8 kernel) - extreme quality
    Lanczos4,
}

impl InterpolationMethod {
    /// Returns the kernel radius for this interpolation method.
    #[inline]
    pub fn kernel_radius(&self) -> usize {
        match self {
            InterpolationMethod::Nearest => 1,
            InterpolationMethod::Bilinear => 1,
            InterpolationMethod::Bicubic => 2,
            InterpolationMethod::Lanczos2 => 2,
            InterpolationMethod::Lanczos3 => 3,
            InterpolationMethod::Lanczos4 => 4,
        }
    }
}

/// Configuration for image warping.
#[derive(Debug, Clone)]
pub struct WarpConfig {
    /// Interpolation method to use
    pub method: InterpolationMethod,
    /// Value to use for pixels outside the image bounds
    pub border_value: f32,
    /// Whether to normalize Lanczos kernel weights (recommended)
    pub normalize_kernel: bool,
    /// Clamp output to the min/max of neighborhood pixels to reduce ringing.
    ///
    /// Lanczos interpolation can produce values outside the range of input pixels
    /// (ringing artifacts) especially near sharp edges. Enabling this option clamps
    /// the result to [min, max] of the sampled neighborhood, eliminating overshoot
    /// and undershoot while preserving most of the sharpness benefit.
    ///
    /// This option only affects Lanczos interpolation methods.
    pub clamp_output: bool,
}

impl WarpConfig {
    /// Validate configuration parameters.
    pub fn validate(&self) {
        // border_value can be any f32, no constraint needed
        // normalize_kernel and clamp_output are booleans, always valid
        // method is an enum, always valid
        // No invalid states for WarpConfig - all field combinations are valid
    }
}

impl Default for WarpConfig {
    fn default() -> Self {
        Self {
            method: InterpolationMethod::default(),
            border_value: 0.0,
            normalize_kernel: true,
            clamp_output: false,
        }
    }
}

// =============================================================================
// RANSAC configuration
// =============================================================================

/// RANSAC configuration.
///
/// Defaults are calibrated against industry standards:
/// - OpenCV `findHomography`: threshold=3.0, iterations=2000, confidence=0.995
/// - Astroalign: pixel_tol=2.0
/// - PixInsight StarAlignment: RANSAC tolerance=2.0, iterations=2000
///
/// Our defaults use threshold=2.0 (tighter than OpenCV for sub-pixel astronomy),
/// iterations=2000 (matches OpenCV/PixInsight for reliability), and confidence=0.995
/// (OpenCV default; avoids overly aggressive early termination that 0.999 can cause).
#[derive(Debug, Clone)]
pub struct RansacConfig {
    /// Maximum iterations.
    pub max_iterations: usize,
    /// Inlier distance threshold in pixels.
    /// OpenCV default is 3.0, Astroalign uses 2.0. We use 2.0 for sub-pixel astronomy.
    pub inlier_threshold: f64,
    /// Target confidence for early termination.
    /// OpenCV uses 0.995. Higher values (e.g. 0.999) can cause premature termination
    /// when a spurious model is found early with few inliers.
    pub confidence: f64,
    /// Minimum inlier ratio to accept model.
    /// Set to 0.3 to handle partial overlap and noisy match sets.
    /// Astroalign uses 0.8 but operates on pre-filtered control points.
    pub min_inlier_ratio: f64,
    /// Random seed for reproducibility (None for random).
    pub seed: Option<u64>,
    /// Enable Local Optimization (LO-RANSAC).
    /// When enabled, promising hypotheses are refined iteratively.
    /// Typically improves inlier count by 10-20% (Chum et al., 2003).
    pub use_local_optimization: bool,
    /// Maximum iterations for local optimization step.
    pub lo_max_iterations: usize,
    /// Maximum allowed rotation in radians. Hypotheses with rotation exceeding
    /// this threshold are rejected as implausible. Set to `None` to disable.
    /// Default: 10 degrees (~0.175 rad), suitable for tracked mounts.
    /// Mosaics or untracked mounts may need higher values or `None`.
    pub max_rotation: Option<f64>,
    /// Allowed scale range (min, max). Hypotheses with scale outside this range
    /// are rejected as implausible. Set to `None` to disable.
    /// Default: (0.8, 1.2), suitable for same-telescope stacking.
    /// Different focal lengths or binning modes may need wider ranges or `None`.
    pub scale_range: Option<(f64, f64)>,
}

impl Default for RansacConfig {
    fn default() -> Self {
        Self {
            max_iterations: 2000,
            inlier_threshold: 2.0,
            confidence: 0.995,
            min_inlier_ratio: 0.3,
            seed: None,
            use_local_optimization: true,
            lo_max_iterations: 10,
            max_rotation: Some(10.0_f64.to_radians()),
            scale_range: Some((0.8, 1.2)),
        }
    }
}

impl RansacConfig {
    /// Validate configuration parameters.
    pub fn validate(&self) {
        assert!(
            self.max_iterations > 0,
            "RANSAC max_iterations must be positive, got {}",
            self.max_iterations
        );
        assert!(
            self.inlier_threshold > 0.0,
            "RANSAC inlier_threshold must be positive, got {}",
            self.inlier_threshold
        );
        assert!(
            (0.0..=1.0).contains(&self.confidence),
            "RANSAC confidence must be in [0, 1], got {}",
            self.confidence
        );
        assert!(
            self.min_inlier_ratio > 0.0 && self.min_inlier_ratio <= 1.0,
            "RANSAC min_inlier_ratio must be in (0, 1], got {}",
            self.min_inlier_ratio
        );
        if let Some(max_rot) = self.max_rotation {
            assert!(
                max_rot > 0.0,
                "RANSAC max_rotation must be positive, got {}",
                max_rot
            );
        }
        if let Some((min_scale, max_scale)) = self.scale_range {
            assert!(
                min_scale > 0.0 && max_scale > min_scale,
                "RANSAC scale_range must have 0 < min < max, got ({}, {})",
                min_scale,
                max_scale
            );
        }
    }
}

// =============================================================================
// Triangle matching configuration
// =============================================================================

/// Configuration for triangle matching.
///
/// Defaults are based on industry practice:
/// - Siril uses brightest 20 stars with brute-force O(n^3) triangle formation.
///   We use k-d tree based formation on 200 stars for O(n*k^2) scaling.
/// - Astroalign uses 50 control points with 5 nearest neighbors per star
///   and KDTree search radius 0.02 for invariant matching.
/// - Our 1% ratio tolerance is stricter than Astroalign's 0.02 radius but
///   we compensate with a larger star count (200 vs 50).
#[derive(Debug, Clone)]
pub struct TriangleMatchConfig {
    /// Maximum number of stars to use (brightest N).
    /// Siril uses 20 (brute-force), Astroalign uses 50, we use 200 with k-d tree.
    pub max_stars: usize,
    /// Tolerance for side ratio comparison.
    /// 1% tolerance provides good selectivity while allowing for centroid noise.
    pub ratio_tolerance: f64,
    /// Minimum votes required to accept a match.
    /// A star pair must be confirmed by at least this many independent triangles.
    pub min_votes: usize,
    /// Check orientation (set false to handle mirrored images).
    pub check_orientation: bool,
    /// Enable two-step matching with transform-guided refinement.
    /// Phase 1 uses normal tolerance for initial matches.
    /// Phase 2 estimates a transform, then re-votes with 0.5x tolerance
    /// and position-proximity bonus for matches consistent with the transform.
    pub two_step_matching: bool,
}

impl Default for TriangleMatchConfig {
    fn default() -> Self {
        Self {
            max_stars: 200,
            ratio_tolerance: 0.01,
            min_votes: 3,
            check_orientation: true,
            two_step_matching: true,
        }
    }
}

impl TriangleMatchConfig {
    /// Validate configuration parameters.
    pub fn validate(&self) {
        assert!(
            self.max_stars >= 3,
            "max_stars must be >= 3 for triangle matching, got {}",
            self.max_stars
        );
        assert!(
            self.ratio_tolerance > 0.0 && self.ratio_tolerance < 1.0,
            "ratio_tolerance must be in (0, 1), got {}",
            self.ratio_tolerance
        );
        assert!(
            self.min_votes >= 1,
            "min_votes must be at least 1, got {}",
            self.min_votes
        );
    }
}

// =============================================================================
// Registration pipeline configuration
// =============================================================================

/// SIP distortion correction configuration for the registration pipeline.
///
/// When enabled, a SIP polynomial is fit to the residuals after RANSAC
/// to correct for optical distortion that a single homography cannot capture.
#[derive(Debug, Clone)]
pub struct SipCorrectionConfig {
    /// Whether to enable SIP correction.
    pub enabled: bool,
    /// Polynomial order (2-5). Order 2 handles barrel/pincushion,
    /// order 3 handles mustache distortion.
    pub order: usize,
}

impl Default for SipCorrectionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            order: 3,
        }
    }
}

impl SipCorrectionConfig {
    /// Validate configuration.
    pub fn validate(&self) {
        if self.enabled {
            assert!(
                (2..=5).contains(&self.order),
                "SIP order must be 2-5, got {}",
                self.order
            );
        }
    }
}

/// Registration configuration.
///
/// Defaults target reasonable precision and quality for typical astrophotography:
/// - Homography transform (8 DOF) — Siril's default, handles wide-field perspective.
///   Similarity is insufficient for wide-field lenses; Homography is robust for both
///   narrow and wide fields.
/// - 2.0 pixel max residual — matches Astroalign/PixInsight tolerance. 1.0 is too
///   strict and rejects valid registrations with slight distortion.
/// - Spatial distribution enabled — ensures star coverage across the field, critical
///   for accurate distortion modeling on wide-field images.
#[derive(Debug, Clone)]
pub struct RegistrationConfig {
    /// Transformation model to use.
    /// Default is `Auto`, which starts with Similarity (4 DOF) and upgrades to
    /// Homography (8 DOF) if residuals exceed 0.5 px. Use an explicit type to
    /// force a specific model.
    pub transform_type: TransformType,

    /// Minimum stars required for matching.
    pub min_stars_for_matching: usize,
    /// Minimum matched star pairs required.
    /// Siril uses 10, we use 8 as a balanced minimum that ensures statistical
    /// robustness without being too strict for sparse fields.
    pub min_matched_stars: usize,
    /// Maximum acceptable RMS error in pixels.
    /// Astroalign uses 2.0, PixInsight starts at 2.0. Our default of 2.0 avoids
    /// rejecting valid registrations that have slight optical distortion.
    pub max_residual_pixels: f64,

    /// Use spatial distribution when selecting stars for matching.
    /// When enabled, stars are selected from a grid across the image to ensure
    /// coverage of all regions, rather than just taking the brightest N stars
    /// which may cluster in one area. This improves registration accuracy for
    /// images with lens distortion.
    pub use_spatial_distribution: bool,

    /// Grid size for spatial distribution (NxN cells).
    /// Only used when `use_spatial_distribution` is true.
    pub spatial_grid_size: usize,

    /// Triangle matching configuration.
    pub triangle: TriangleMatchConfig,
    /// RANSAC robust estimation configuration.
    pub ransac: RansacConfig,
    /// Image warping configuration.
    pub warp: WarpConfig,
    /// SIP distortion correction (post-RANSAC polynomial refinement).
    pub sip: SipCorrectionConfig,
}

impl Default for RegistrationConfig {
    fn default() -> Self {
        Self {
            transform_type: TransformType::Auto,
            min_stars_for_matching: 10,
            min_matched_stars: 8,
            max_residual_pixels: 2.0,
            use_spatial_distribution: true,
            spatial_grid_size: 8,
            triangle: TriangleMatchConfig::default(),
            ransac: RansacConfig::default(),
            warp: WarpConfig::default(),
            sip: SipCorrectionConfig::default(),
        }
    }
}

impl RegistrationConfig {
    /// Validate configuration parameters.
    pub fn validate(&self) {
        assert!(
            self.min_stars_for_matching >= 3,
            "Need at least 3 stars for triangle matching"
        );
        assert!(
            self.triangle.max_stars >= self.min_stars_for_matching,
            "triangle.max_stars must be >= min_stars_for_matching"
        );
        assert!(
            self.min_matched_stars >= self.transform_type.min_points(),
            "min_matched_stars must be >= transform minimum points"
        );
        assert!(
            self.max_residual_pixels > 0.0,
            "max_residual must be positive"
        );
        assert!(
            self.spatial_grid_size >= 2,
            "spatial_grid_size must be at least 2"
        );

        // Delegate to sub-config validation
        self.triangle.validate();
        self.ransac.validate();
        self.warp.validate();
        self.sip.validate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default_values() {
        let config = RegistrationConfig::default();
        assert_eq!(config.transform_type, TransformType::Auto);
        assert_eq!(config.min_matched_stars, 8);
        assert!((config.max_residual_pixels - 2.0).abs() < 1e-10);
        assert_eq!(config.ransac.max_iterations, 2000);
        assert!((config.ransac.inlier_threshold - 2.0).abs() < 1e-10);
        assert!((config.ransac.confidence - 0.995).abs() < 1e-10);
        assert!((config.ransac.min_inlier_ratio - 0.3).abs() < 1e-10);
        assert_eq!(config.triangle.max_stars, 200);
    }

    #[test]
    fn test_config_struct_init() {
        let config = RegistrationConfig {
            transform_type: TransformType::Euclidean,
            ransac: RansacConfig {
                max_iterations: 500,
                inlier_threshold: 3.0,
                ..RansacConfig::default()
            },
            triangle: TriangleMatchConfig {
                max_stars: 100,
                ..TriangleMatchConfig::default()
            },
            ..Default::default()
        };

        assert_eq!(config.transform_type, TransformType::Euclidean);
        assert_eq!(config.ransac.max_iterations, 500);
        assert!((config.ransac.inlier_threshold - 3.0).abs() < 1e-10);
        assert_eq!(config.triangle.max_stars, 100);
    }

    #[test]
    fn test_config_validation() {
        let config = RegistrationConfig::default();
        config.validate();
    }

    #[test]
    #[should_panic(expected = "RANSAC max_iterations must be positive")]
    fn test_config_invalid_iterations() {
        let config = RegistrationConfig {
            ransac: RansacConfig {
                max_iterations: 0,
                ..RansacConfig::default()
            },
            ..Default::default()
        };
        config.validate();
    }

    #[test]
    fn test_warp_config_default() {
        let config = WarpConfig::default();
        assert_eq!(config.method, InterpolationMethod::Lanczos3);
        assert!((config.border_value - 0.0).abs() < 1e-10);
        assert!(config.normalize_kernel);
        assert!(!config.clamp_output);
        config.validate();
    }

    #[test]
    fn test_ransac_config_default() {
        let config = RansacConfig::default();
        assert_eq!(config.max_iterations, 2000);
        assert!((config.inlier_threshold - 2.0).abs() < 1e-10);
        assert!((config.confidence - 0.995).abs() < 1e-10);
        assert!((config.min_inlier_ratio - 0.3).abs() < 1e-10);
        assert!(config.use_local_optimization);
        config.validate();
    }

    #[test]
    fn test_triangle_config_default() {
        let config = TriangleMatchConfig::default();
        assert_eq!(config.max_stars, 200);
        assert!((config.ratio_tolerance - 0.01).abs() < 1e-10);
        config.validate();
    }

    #[test]
    #[should_panic(expected = "max_stars must be >= 3")]
    fn test_triangle_config_invalid() {
        let config = TriangleMatchConfig {
            max_stars: 2,
            ..TriangleMatchConfig::default()
        };
        config.validate();
    }

    #[test]
    fn test_interpolation_method_kernel_radius() {
        assert_eq!(InterpolationMethod::Nearest.kernel_radius(), 1);
        assert_eq!(InterpolationMethod::Bilinear.kernel_radius(), 1);
        assert_eq!(InterpolationMethod::Bicubic.kernel_radius(), 2);
        assert_eq!(InterpolationMethod::Lanczos2.kernel_radius(), 2);
        assert_eq!(InterpolationMethod::Lanczos3.kernel_radius(), 3);
        assert_eq!(InterpolationMethod::Lanczos4.kernel_radius(), 4);
    }
}
