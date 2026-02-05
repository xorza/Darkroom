//! Configuration for the registration module.

use crate::registration::transform::TransformType;

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

/// Configuration for image registration.
///
/// All parameters have sensible defaults calibrated against industry standards
/// (OpenCV, Astroalign, PixInsight). Most users only need to set `transform_type`
/// if they want a specific model.
///
/// # Example
///
/// ```ignore
/// use lumos::registration::{Config, register};
///
/// // Use defaults
/// let result = register(&ref_stars, &target_stars, &Config::default())?;
///
/// // Custom config
/// let config = Config {
///     inlier_threshold: 3.0,
///     ..Config::default()
/// };
/// let result = register(&ref_stars, &target_stars, &config)?;
///
/// // Use a preset
/// let result = register(&ref_stars, &target_stars, &Config::wide_field())?;
/// ```
#[derive(Debug, Clone)]
pub struct Config {
    // == Transform model ==
    /// Transformation model: Translation, Euclidean, Similarity, Affine, Homography, Auto.
    /// Default: Auto (starts with Similarity, upgrades to Homography if needed).
    pub transform_type: TransformType,

    // == Star matching ==
    /// Maximum stars to use for matching (brightest N). Default: 200.
    pub max_stars: usize,
    /// Minimum stars required in each image. Default: 10.
    pub min_stars: usize,
    /// Minimum matched star pairs to accept. Default: 8.
    pub min_matches: usize,
    /// Triangle ratio tolerance (0.01 = 1%). Default: 0.01.
    pub ratio_tolerance: f64,
    /// Minimum confirming triangles per match. Default: 3.
    pub min_votes: usize,
    /// Check orientation (false allows mirrored images). Default: true.
    pub check_orientation: bool,

    // == Spatial distribution ==
    /// Use spatial grid for star selection. Default: true.
    pub use_spatial_grid: bool,
    /// Grid size (NxN cells). Default: 8.
    pub spatial_grid_size: usize,

    // == RANSAC ==
    /// RANSAC iterations. Default: 2000.
    pub ransac_iterations: usize,
    /// Inlier distance threshold in pixels. Default: 2.0.
    pub inlier_threshold: f64,
    /// Target confidence for early termination. Default: 0.995.
    pub confidence: f64,
    /// Minimum inlier ratio. Default: 0.3.
    pub min_inlier_ratio: f64,
    /// Random seed for reproducibility (None = random). Default: None.
    pub seed: Option<u64>,
    /// Enable LO-RANSAC refinement. Default: true.
    pub local_optimization: bool,
    /// LO-RANSAC iterations. Default: 10.
    pub lo_iterations: usize,
    /// Maximum rotation in radians (None = unlimited). Default: Some(0.175).
    pub max_rotation: Option<f64>,
    /// Scale range (min, max). Default: Some((0.8, 1.2)).
    pub scale_range: Option<(f64, f64)>,

    // == Quality ==
    /// Maximum acceptable RMS error in pixels. Default: 2.0.
    pub max_rms_error: f64,

    // == Distortion correction ==
    /// Enable SIP polynomial distortion correction. Default: false.
    pub sip_enabled: bool,
    /// SIP polynomial order (2-5). Default: 3.
    pub sip_order: usize,

    // == Image warping ==
    /// Interpolation method for warping. Default: Lanczos3.
    pub interpolation: InterpolationMethod,
    /// Border value for out-of-bounds pixels. Default: 0.0.
    pub border_value: f32,
    /// Normalize Lanczos kernel weights. Default: true.
    pub normalize_kernel: bool,
    /// Clamp output to reduce Lanczos ringing. Default: false.
    pub clamp_output: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            // Transform
            transform_type: TransformType::Auto,

            // Star matching
            max_stars: 200,
            min_stars: 10,
            min_matches: 8,
            ratio_tolerance: 0.01,
            min_votes: 3,
            check_orientation: true,

            // Spatial distribution
            use_spatial_grid: true,
            spatial_grid_size: 8,

            // RANSAC
            ransac_iterations: 2000,
            inlier_threshold: 2.0,
            confidence: 0.995,
            min_inlier_ratio: 0.3,
            seed: None,
            local_optimization: true,
            lo_iterations: 10,
            max_rotation: Some(10.0_f64.to_radians()),
            scale_range: Some((0.8, 1.2)),

            // Quality
            max_rms_error: 2.0,

            // Distortion
            sip_enabled: false,
            sip_order: 3,

            // Warping
            interpolation: InterpolationMethod::default(),
            border_value: 0.0,
            normalize_kernel: true,
            clamp_output: false,
        }
    }
}

impl Config {
    /// Fast configuration: fewer iterations, lower quality, faster.
    pub fn fast() -> Self {
        Self {
            ransac_iterations: 500,
            max_stars: 100,
            local_optimization: false,
            interpolation: InterpolationMethod::Bilinear,
            ..Self::default()
        }
    }

    /// Precise configuration: more iterations, SIP correction enabled.
    pub fn precise() -> Self {
        Self {
            ransac_iterations: 5000,
            confidence: 0.999,
            sip_enabled: true,
            max_rms_error: 1.0,
            ..Self::default()
        }
    }

    /// Wide-field configuration: handles lens distortion.
    pub fn wide_field() -> Self {
        Self {
            transform_type: TransformType::Homography,
            sip_enabled: true,
            max_rotation: None,
            scale_range: None,
            ..Self::default()
        }
    }

    /// Mosaic configuration: allows larger offsets and rotations.
    pub fn mosaic() -> Self {
        Self {
            max_rotation: None,
            scale_range: Some((0.5, 2.0)),
            use_spatial_grid: true,
            ..Self::default()
        }
    }

    /// Validate all configuration parameters.
    pub fn validate(&self) {
        // Star matching
        assert!(
            self.max_stars >= 3,
            "max_stars must be >= 3 for triangle matching, got {}",
            self.max_stars
        );
        assert!(
            self.min_stars >= 3,
            "min_stars must be >= 3 for triangle matching, got {}",
            self.min_stars
        );
        assert!(
            self.max_stars >= self.min_stars,
            "max_stars ({}) must be >= min_stars ({})",
            self.max_stars,
            self.min_stars
        );
        assert!(
            self.min_matches >= self.transform_type.min_points(),
            "min_matches ({}) must be >= transform minimum points ({})",
            self.min_matches,
            self.transform_type.min_points()
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

        // Spatial distribution
        assert!(
            self.spatial_grid_size >= 2,
            "spatial_grid_size must be at least 2, got {}",
            self.spatial_grid_size
        );

        // RANSAC
        assert!(
            self.ransac_iterations > 0,
            "ransac_iterations must be positive, got {}",
            self.ransac_iterations
        );
        assert!(
            self.inlier_threshold > 0.0,
            "inlier_threshold must be positive, got {}",
            self.inlier_threshold
        );
        assert!(
            (0.0..=1.0).contains(&self.confidence),
            "confidence must be in [0, 1], got {}",
            self.confidence
        );
        assert!(
            self.min_inlier_ratio > 0.0 && self.min_inlier_ratio <= 1.0,
            "min_inlier_ratio must be in (0, 1], got {}",
            self.min_inlier_ratio
        );
        if let Some(max_rot) = self.max_rotation {
            assert!(
                max_rot > 0.0,
                "max_rotation must be positive, got {}",
                max_rot
            );
        }
        if let Some((min_scale, max_scale)) = self.scale_range {
            assert!(
                min_scale > 0.0 && max_scale > min_scale,
                "scale_range must have 0 < min < max, got ({}, {})",
                min_scale,
                max_scale
            );
        }

        // Quality
        assert!(
            self.max_rms_error > 0.0,
            "max_rms_error must be positive, got {}",
            self.max_rms_error
        );

        // Distortion
        if self.sip_enabled {
            assert!(
                (2..=5).contains(&self.sip_order),
                "sip_order must be 2-5, got {}",
                self.sip_order
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default_values() {
        let config = Config::default();
        assert_eq!(config.transform_type, TransformType::Auto);
        assert_eq!(config.max_stars, 200);
        assert_eq!(config.min_stars, 10);
        assert_eq!(config.min_matches, 8);
        assert!((config.ratio_tolerance - 0.01).abs() < 1e-10);
        assert_eq!(config.min_votes, 3);
        assert!(config.check_orientation);
        assert!(config.use_spatial_grid);
        assert_eq!(config.spatial_grid_size, 8);
        assert_eq!(config.ransac_iterations, 2000);
        assert!((config.inlier_threshold - 2.0).abs() < 1e-10);
        assert!((config.confidence - 0.995).abs() < 1e-10);
        assert!((config.min_inlier_ratio - 0.3).abs() < 1e-10);
        assert!(config.seed.is_none());
        assert!(config.local_optimization);
        assert_eq!(config.lo_iterations, 10);
        assert!((config.max_rms_error - 2.0).abs() < 1e-10);
        assert!(!config.sip_enabled);
        assert_eq!(config.sip_order, 3);
        assert_eq!(config.interpolation, InterpolationMethod::Lanczos3);
        assert!((config.border_value - 0.0).abs() < 1e-10);
        assert!(config.normalize_kernel);
        assert!(!config.clamp_output);
    }

    #[test]
    fn test_config_validation() {
        Config::default().validate();
    }

    #[test]
    fn test_config_fast_preset() {
        let config = Config::fast();
        assert_eq!(config.ransac_iterations, 500);
        assert_eq!(config.max_stars, 100);
        assert!(!config.local_optimization);
        assert_eq!(config.interpolation, InterpolationMethod::Bilinear);
        config.validate();
    }

    #[test]
    fn test_config_precise_preset() {
        let config = Config::precise();
        assert_eq!(config.ransac_iterations, 5000);
        assert!((config.confidence - 0.999).abs() < 1e-10);
        assert!(config.sip_enabled);
        assert!((config.max_rms_error - 1.0).abs() < 1e-10);
        config.validate();
    }

    #[test]
    fn test_config_wide_field_preset() {
        let config = Config::wide_field();
        assert_eq!(config.transform_type, TransformType::Homography);
        assert!(config.sip_enabled);
        assert!(config.max_rotation.is_none());
        assert!(config.scale_range.is_none());
        config.validate();
    }

    #[test]
    fn test_config_mosaic_preset() {
        let config = Config::mosaic();
        assert!(config.max_rotation.is_none());
        assert_eq!(config.scale_range, Some((0.5, 2.0)));
        assert!(config.use_spatial_grid);
        config.validate();
    }

    #[test]
    fn test_config_custom() {
        let config = Config {
            transform_type: TransformType::Similarity,
            inlier_threshold: 3.0,
            ransac_iterations: 1000,
            ..Config::default()
        };
        assert_eq!(config.transform_type, TransformType::Similarity);
        assert!((config.inlier_threshold - 3.0).abs() < 1e-10);
        assert_eq!(config.ransac_iterations, 1000);
        config.validate();
    }

    #[test]
    #[should_panic(expected = "ransac_iterations must be positive")]
    fn test_config_invalid_iterations() {
        let config = Config {
            ransac_iterations: 0,
            ..Config::default()
        };
        config.validate();
    }

    #[test]
    #[should_panic(expected = "max_stars must be >= 3")]
    fn test_config_invalid_max_stars() {
        let config = Config {
            max_stars: 2,
            ..Config::default()
        };
        config.validate();
    }

    #[test]
    #[should_panic(expected = "inlier_threshold must be positive")]
    fn test_config_invalid_threshold() {
        let config = Config {
            inlier_threshold: 0.0,
            ..Config::default()
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
