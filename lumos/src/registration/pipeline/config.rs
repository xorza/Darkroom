//! Registration configuration.

use crate::registration::transform::TransformType;

/// Registration configuration.
#[derive(Debug, Clone)]
pub struct RegistrationConfig {
    /// Maximum transformation type to consider.
    pub transform_type: TransformType,

    /// Use phase correlation for coarse alignment first.
    pub use_phase_correlation: bool,

    /// RANSAC maximum iterations.
    pub ransac_iterations: usize,
    /// RANSAC inlier threshold in pixels.
    pub ransac_threshold: f64,
    /// RANSAC target confidence (0.0 - 1.0).
    pub ransac_confidence: f64,

    /// Minimum stars required for matching.
    pub min_stars_for_matching: usize,
    /// Maximum stars to use (brightest N).
    pub max_stars_for_matching: usize,
    /// Triangle side ratio tolerance.
    pub triangle_tolerance: f64,

    /// Refine transformation using star centroids.
    pub refine_with_centroids: bool,
    /// Maximum refinement iterations.
    pub max_refinement_iterations: usize,

    /// Minimum matched star pairs required.
    pub min_matched_stars: usize,
    /// Maximum acceptable RMS error in pixels.
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
}

impl Default for RegistrationConfig {
    fn default() -> Self {
        Self {
            transform_type: TransformType::Similarity,
            use_phase_correlation: false,
            ransac_iterations: 1000,
            ransac_threshold: 2.0,
            ransac_confidence: 0.999,
            min_stars_for_matching: 10,
            max_stars_for_matching: 200,
            triangle_tolerance: 0.01,
            refine_with_centroids: true,
            max_refinement_iterations: 10,
            min_matched_stars: 6,
            max_residual_pixels: 1.0,
            use_spatial_distribution: true,
            spatial_grid_size: 8,
        }
    }
}

impl RegistrationConfig {
    /// Validate configuration parameters.
    pub fn validate(&self) {
        assert!(
            self.ransac_iterations > 0,
            "RANSAC iterations must be positive"
        );
        assert!(
            self.ransac_threshold > 0.0,
            "RANSAC threshold must be positive"
        );
        assert!(
            (0.0..=1.0).contains(&self.ransac_confidence),
            "RANSAC confidence must be in [0, 1]"
        );
        assert!(
            self.min_stars_for_matching >= 3,
            "Need at least 3 stars for triangle matching"
        );
        assert!(
            self.max_stars_for_matching >= self.min_stars_for_matching,
            "max_stars must be >= min_stars"
        );
        assert!(
            self.triangle_tolerance > 0.0 && self.triangle_tolerance < 1.0,
            "Triangle tolerance must be in (0, 1)"
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default_values() {
        let config = RegistrationConfig::default();
        assert_eq!(config.transform_type, TransformType::Similarity);
        assert_eq!(config.ransac_iterations, 1000);
        assert!((config.ransac_threshold - 2.0).abs() < 1e-10);
    }

    #[test]
    fn test_config_struct_init() {
        let config = RegistrationConfig {
            transform_type: TransformType::Euclidean,
            ransac_iterations: 500,
            ransac_threshold: 3.0,
            max_stars_for_matching: 100,
            ..Default::default()
        };

        assert_eq!(config.transform_type, TransformType::Euclidean);
        assert_eq!(config.ransac_iterations, 500);
        assert!((config.ransac_threshold - 3.0).abs() < 1e-10);
        assert_eq!(config.max_stars_for_matching, 100);
    }

    #[test]
    fn test_config_validation() {
        // Valid config should not panic
        let config = RegistrationConfig::default();
        config.validate();
    }

    #[test]
    #[should_panic(expected = "RANSAC iterations must be positive")]
    fn test_config_invalid_iterations() {
        let config = RegistrationConfig {
            ransac_iterations: 0,
            ..Default::default()
        };
        config.validate();
    }
}
