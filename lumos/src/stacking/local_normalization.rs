//! Local normalization for multi-session astrophotography stacking.
//!
//! This module implements PixInsight-style local normalization to match
//! illumination differences across frames by adjusting brightness locally
//! rather than globally. This handles:
//! - Vignetting (darker corners, brighter center)
//! - Sky gradients (light pollution, moon, twilight)
//! - Session-to-session brightness variations
//!
//! # Algorithm
//!
//! 1. Divide image into tiles (default: 128x128 pixels)
//! 2. Compute sigma-clipped median and scale (MAD) for each tile
//! 3. Compare target frame tiles to reference frame tiles
//! 4. Compute per-tile offset and scale correction factors
//! 5. Bilinearly interpolate between tile centers for smooth correction
//! 6. Apply: `pixel_corrected = (pixel - target_median) * scale + ref_median`

/// Normalization method for aligning frame statistics before stacking.
#[derive(Debug, Clone, PartialEq, Default)]
#[allow(dead_code)] // Public API - implementation in progress
pub enum NormalizationMethod {
    /// No normalization - use raw pixel values.
    None,
    /// Global normalization - match overall median and scale.
    /// Simple and fast, works well for single-session data.
    #[default]
    Global,
    /// Local normalization - tile-based matching.
    /// Best for multi-session data or frames with varying gradients.
    Local(LocalNormalizationConfig),
}

/// Configuration for local normalization.
///
/// Local normalization divides the image into tiles and computes correction
/// factors per-tile, then interpolates smoothly between tile centers.
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)] // Public API - implementation in progress
pub struct LocalNormalizationConfig {
    /// Tile size in pixels. Larger tiles are more robust to noise but
    /// less accurate for steep gradients.
    /// Default: 128, Range: 64-256
    pub tile_size: usize,
    /// Sigma threshold for clipping outliers (stars) when computing tile statistics.
    /// Default: 3.0
    pub clip_sigma: f32,
    /// Number of sigma-clipping iterations.
    /// Default: 3
    pub clip_iterations: usize,
}

impl Default for LocalNormalizationConfig {
    fn default() -> Self {
        Self {
            tile_size: 128,
            clip_sigma: 3.0,
            clip_iterations: 3,
        }
    }
}

#[allow(dead_code)] // Public API - implementation in progress
impl LocalNormalizationConfig {
    /// Create a new configuration with the specified tile size.
    ///
    /// # Panics
    ///
    /// Panics if tile_size is not in the range 64-256.
    pub fn new(tile_size: usize) -> Self {
        assert!(
            (64..=256).contains(&tile_size),
            "Tile size must be between 64 and 256, got {}",
            tile_size
        );
        Self {
            tile_size,
            ..Default::default()
        }
    }

    /// Set custom clipping parameters.
    pub fn with_clipping(mut self, sigma: f32, iterations: usize) -> Self {
        assert!(sigma > 0.0, "Clip sigma must be positive");
        assert!(iterations > 0, "Clip iterations must be at least 1");
        self.clip_sigma = sigma;
        self.clip_iterations = iterations;
        self
    }

    /// Create configuration optimized for fine gradients (smaller tiles).
    pub fn fine() -> Self {
        Self {
            tile_size: 64,
            ..Default::default()
        }
    }

    /// Create configuration optimized for stability (larger tiles).
    pub fn coarse() -> Self {
        Self {
            tile_size: 256,
            ..Default::default()
        }
    }
}

/// Per-tile normalization statistics computed from a frame.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Public API - implementation in progress
pub struct TileNormalizationStats {
    /// Per-tile median values.
    pub medians: Vec<f32>,
    /// Per-tile scale (MAD) values.
    pub scales: Vec<f32>,
    /// Number of tiles in X direction.
    pub tiles_x: usize,
    /// Number of tiles in Y direction.
    pub tiles_y: usize,
    /// Tile size used for computation.
    pub tile_size: usize,
    /// Image width.
    pub width: usize,
    /// Image height.
    pub height: usize,
}

/// Local normalization map for correcting a target frame to match a reference.
///
/// Contains per-tile offset and scale factors that can be interpolated
/// to correct individual pixels.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Public API - implementation in progress
pub struct LocalNormalizationMap {
    /// Per-tile offset (additive correction).
    pub offsets: Vec<f32>,
    /// Per-tile scale (multiplicative correction).
    pub scales: Vec<f32>,
    /// Tile centers X coordinates for interpolation.
    pub centers_x: Vec<f32>,
    /// Tile centers Y coordinates for interpolation.
    pub centers_y: Vec<f32>,
    /// Number of tiles in X direction.
    pub tiles_x: usize,
    /// Number of tiles in Y direction.
    pub tiles_y: usize,
    /// Image width.
    pub width: usize,
    /// Image height.
    pub height: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalization_method_default() {
        let method = NormalizationMethod::default();
        assert!(matches!(method, NormalizationMethod::Global));
    }

    #[test]
    fn test_local_normalization_config_default() {
        let config = LocalNormalizationConfig::default();
        assert_eq!(config.tile_size, 128);
        assert!((config.clip_sigma - 3.0).abs() < f32::EPSILON);
        assert_eq!(config.clip_iterations, 3);
    }

    #[test]
    fn test_local_normalization_config_new() {
        let config = LocalNormalizationConfig::new(64);
        assert_eq!(config.tile_size, 64);
    }

    #[test]
    #[should_panic(expected = "Tile size must be between 64 and 256")]
    fn test_local_normalization_config_invalid_tile_size_small() {
        LocalNormalizationConfig::new(32);
    }

    #[test]
    #[should_panic(expected = "Tile size must be between 64 and 256")]
    fn test_local_normalization_config_invalid_tile_size_large() {
        LocalNormalizationConfig::new(512);
    }

    #[test]
    fn test_local_normalization_config_with_clipping() {
        let config = LocalNormalizationConfig::default().with_clipping(2.5, 5);
        assert!((config.clip_sigma - 2.5).abs() < f32::EPSILON);
        assert_eq!(config.clip_iterations, 5);
    }

    #[test]
    #[should_panic(expected = "Clip sigma must be positive")]
    fn test_local_normalization_config_invalid_sigma() {
        LocalNormalizationConfig::default().with_clipping(0.0, 3);
    }

    #[test]
    #[should_panic(expected = "Clip iterations must be at least 1")]
    fn test_local_normalization_config_invalid_iterations() {
        LocalNormalizationConfig::default().with_clipping(3.0, 0);
    }

    #[test]
    fn test_local_normalization_config_fine() {
        let config = LocalNormalizationConfig::fine();
        assert_eq!(config.tile_size, 64);
    }

    #[test]
    fn test_local_normalization_config_coarse() {
        let config = LocalNormalizationConfig::coarse();
        assert_eq!(config.tile_size, 256);
    }

    #[test]
    fn test_normalization_method_none() {
        let method = NormalizationMethod::None;
        assert!(matches!(method, NormalizationMethod::None));
    }

    #[test]
    fn test_normalization_method_local() {
        let config = LocalNormalizationConfig::default();
        let method = NormalizationMethod::Local(config.clone());
        assert!(matches!(method, NormalizationMethod::Local(_)));

        if let NormalizationMethod::Local(c) = method {
            assert_eq!(c.tile_size, config.tile_size);
        }
    }
}
