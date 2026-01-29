//! Synthetic data generation for testing.
//!
//! This module provides comprehensive tools for generating synthetic astronomical
//! images and star fields for testing star detection and registration algorithms.
//!
//! # Quick Start
//!
//! ```rust,ignore
//! use lumos::testing::synthetic;
//!
//! // Generate a sparse star field (20 well-separated stars)
//! let (pixels, stars) = synthetic::sparse_field(512, 512);
//!
//! // Generate a dense field (200 stars)
//! let (pixels, stars) = synthetic::dense_field(1024, 1024);
//!
//! // Custom configuration
//! let config = synthetic::StarFieldConfig {
//!     width: 1024,
//!     height: 1024,
//!     num_stars: 100,
//!     noise_sigma: 0.02,
//!     ..Default::default()
//! };
//! let (pixels, stars) = synthetic::generate_star_field(&config);
//! ```
//!
//! # Modules
//!
//! - [`star_field`] - Main star field generator with comprehensive configuration
//! - [`star_profiles`] - PSF rendering (Gaussian, Moffat, elliptical, saturated)
//! - [`backgrounds`] - Background generators (uniform, gradient, vignette, nebula)
//! - [`artifacts`] - Artifact generators (cosmic rays, hot pixels, bad columns)
//! - [`transforms`] - Star position transforms for registration testing
//! - [`patterns`] - Simple test patterns (gradients, checkerboard, noise)
//! - [`stamps`] - Small star stamps for centroid/profile fitting tests
//! - [`background_map`] - BackgroundMap generation for detection tests

#![allow(dead_code)]

pub mod artifacts;
pub mod background_map;
pub mod backgrounds;
pub mod patterns;
pub mod stamps;
pub mod star_field;
pub mod star_profiles;
pub mod transforms;

// Re-export main types and functions for convenience
use crate::common::Buffer2;
pub use artifacts::add_cosmic_rays;
pub use backgrounds::{
    NebulaConfig, add_gradient_background, add_nebula_background, add_uniform_background,
    add_vignette_background,
};
pub use star_field::{
    CrowdingType, ElongationType, GroundTruthStar, StarFieldConfig, crowded_cluster_config,
    dense_field_config, elliptical_stars_config, faint_stars_config, generate_star_field,
    sparse_field_config,
};
pub use star_profiles::{fwhm_to_sigma, render_gaussian_star};
pub use transforms::{
    add_position_noise, add_spurious_stars, filter_to_bounds, generate_random_positions,
    remove_random_stars, transform_stars, translate_stars, translate_with_overlap,
};

// ============================================================================
// Convenient preset functions
// ============================================================================

/// Generate a sparse star field with 20 well-separated stars.
///
/// Good for testing basic detection accuracy without crowding issues.
pub fn sparse_field(width: usize, height: usize) -> (Buffer2<f32>, Vec<GroundTruthStar>) {
    let config = StarFieldConfig {
        width,
        height,
        ..sparse_field_config()
    };
    generate_star_field(&config)
}

/// Generate a dense star field with 200 stars.
///
/// Good for testing detection in moderately crowded fields.
pub fn dense_field(width: usize, height: usize) -> (Buffer2<f32>, Vec<GroundTruthStar>) {
    let config = StarFieldConfig {
        width,
        height,
        ..dense_field_config()
    };
    generate_star_field(&config)
}

/// Generate a crowded cluster with 500 stars.
///
/// Good for testing deblending and crowded field handling.
pub fn crowded_cluster(width: usize, height: usize) -> (Buffer2<f32>, Vec<GroundTruthStar>) {
    let config = StarFieldConfig {
        width,
        height,
        ..crowded_cluster_config()
    };
    generate_star_field(&config)
}

/// Generate faint stars near detection limit.
///
/// Good for testing detection sensitivity and SNR thresholds.
pub fn faint_field(width: usize, height: usize) -> (Buffer2<f32>, Vec<GroundTruthStar>) {
    let config = StarFieldConfig {
        width,
        height,
        ..faint_stars_config()
    };
    generate_star_field(&config)
}

/// Generate elliptical stars simulating tracking errors.
///
/// Good for testing detection with elongated PSFs.
pub fn elliptical_field(width: usize, height: usize) -> (Buffer2<f32>, Vec<GroundTruthStar>) {
    let config = StarFieldConfig {
        width,
        height,
        ..elliptical_stars_config()
    };
    generate_star_field(&config)
}

/// Generate a star field with custom parameters using a builder pattern.
///
/// # Example
///
/// ```rust,ignore
/// let (pixels, stars) = synthetic::field_builder(1024, 1024)
///     .star_count(100)
///     .fwhm(4.0)
///     .noise(0.03)
///     .seed(42)
///     .build();
/// ```
pub fn field_builder(width: usize, height: usize) -> StarFieldBuilder {
    StarFieldBuilder::new(width, height)
}

/// Builder for customizing star field generation.
#[derive(Debug, Clone)]
pub struct StarFieldBuilder {
    config: StarFieldConfig,
}

impl StarFieldBuilder {
    /// Create a new builder with default settings.
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            config: StarFieldConfig {
                width,
                height,
                ..Default::default()
            },
        }
    }

    /// Set the number of stars to generate.
    pub fn star_count(mut self, count: usize) -> Self {
        self.config.num_stars = count;
        self
    }

    /// Set the FWHM (single value, no variation).
    pub fn fwhm(mut self, fwhm: f32) -> Self {
        self.config.fwhm_range = (fwhm, fwhm);
        self
    }

    /// Set the FWHM range (min, max).
    pub fn fwhm_range(mut self, min: f32, max: f32) -> Self {
        self.config.fwhm_range = (min, max);
        self
    }

    /// Set the noise sigma.
    pub fn noise(mut self, sigma: f32) -> Self {
        self.config.noise_sigma = sigma;
        self
    }

    /// Set the background level.
    pub fn background(mut self, level: f32) -> Self {
        self.config.background_level = level;
        self
    }

    /// Set the random seed for reproducibility.
    pub fn seed(mut self, seed: u64) -> Self {
        self.config.seed = seed;
        self
    }

    /// Set the magnitude range (brightest, faintest).
    pub fn magnitude_range(mut self, brightest: f32, faintest: f32) -> Self {
        self.config.magnitude_range = (brightest, faintest);
        self
    }

    /// Set the crowding type.
    pub fn crowding(mut self, crowding: CrowdingType) -> Self {
        self.config.crowding = crowding;
        self
    }

    /// Set the elongation type.
    pub fn elongation(mut self, elongation: ElongationType) -> Self {
        self.config.elongation = elongation;
        self
    }

    /// Add cosmic rays.
    pub fn cosmic_rays(mut self, count: usize) -> Self {
        self.config.cosmic_ray_count = count;
        self
    }

    /// Use Moffat profile instead of Gaussian.
    pub fn moffat(mut self, beta: f32) -> Self {
        self.config.use_moffat = true;
        self.config.moffat_beta = beta;
        self
    }

    /// Set saturation fraction (0.0 to 1.0).
    pub fn saturation(mut self, fraction: f32) -> Self {
        self.config.saturation_fraction = fraction;
        self
    }

    /// Add a linear gradient background.
    pub fn gradient(mut self, start: f32, end: f32, angle: f32) -> Self {
        self.config.gradient = Some((start, end, angle));
        self
    }

    /// Add vignetting.
    pub fn vignette(mut self, center: f32, edge: f32, falloff: f32) -> Self {
        self.config.vignette = Some((center, edge, falloff));
        self
    }

    /// Set edge margin (stars won't be placed within this distance of edges).
    pub fn edge_margin(mut self, margin: usize) -> Self {
        self.config.edge_margin = margin;
        self
    }

    /// Build and generate the star field.
    pub fn build(self) -> (Buffer2<f32>, Vec<GroundTruthStar>) {
        generate_star_field(&self.config)
    }

    /// Get the configuration without generating.
    pub fn config(self) -> StarFieldConfig {
        self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sparse_field() {
        let (pixels, stars) = sparse_field(256, 256);
        assert_eq!(pixels.len(), 256 * 256);
        assert_eq!(stars.len(), 15);
    }

    #[test]
    fn test_dense_field() {
        let (pixels, stars) = dense_field(256, 256);
        assert_eq!(pixels.len(), 256 * 256);
        assert_eq!(stars.len(), 80);
    }

    #[test]
    fn test_builder() {
        let (pixels, stars) = field_builder(128, 128)
            .star_count(10)
            .fwhm(3.0)
            .noise(0.01)
            .seed(12345)
            .build();

        assert_eq!(pixels.len(), 128 * 128);
        assert_eq!(stars.len(), 10);
    }

    #[test]
    fn test_builder_reproducibility() {
        let (pixels1, _) = field_builder(128, 128).seed(42).star_count(5).build();

        let (pixels2, _) = field_builder(128, 128).seed(42).star_count(5).build();

        assert_eq!(pixels1.len(), pixels2.len());
        for (p1, p2) in pixels1.iter().zip(pixels2.iter()) {
            assert!((p1 - p2).abs() < 1e-6);
        }
    }
}
