//! Configuration for the registration module.

use crate::stacking::registration::distortion::sip::SipConfig;
use crate::stacking::registration::interpolation::WarpParams;
use crate::stacking::registration::ransac::RansacConfig;
use crate::stacking::registration::result::RegistrationError;
use crate::stacking::registration::transform::TransformType;
use crate::stacking::registration::triangle::TriangleConfig;

/// Default soft-clamping threshold for Lanczos deringing (PixInsight default).
/// Lower = more aggressive. Range [0.0, 1.0]. Negative disables deringing.
pub const DEFAULT_DERINGING_THRESHOLD: f32 = 0.3;

/// Interpolation method for image resampling.
///
/// Lanczos variants include a `deringing` threshold that controls soft clamping
/// of interpolated values to suppress ringing artifacts (dark halos around
/// bright stars). Uses PixInsight-style clamping that tracks positive/negative
/// Lanczos contributions and applies a quadratic fade. Enabled by default.
///
/// Deringing threshold values:
/// - **Negative**: disabled (no deringing)
/// - **0.0**: most aggressive (hard clamp all negative contribution)
/// - **0.3**: default (PixInsight default, good balance)
/// - **1.0**: least aggressive (only clamps extreme ringing)
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InterpolationMethod {
    /// Nearest neighbor - fastest, lowest quality
    Nearest,
    /// Bilinear interpolation - fast, reasonable quality
    Bilinear,
    /// Bicubic interpolation - good quality
    Bicubic,
    /// Lanczos-2 (4x4 kernel) - high quality
    Lanczos2 { deringing: f32 },
    /// Lanczos-3 (6x6 kernel) - highest quality, default
    Lanczos3 { deringing: f32 },
    /// Lanczos-4 (8x8 kernel) - extreme quality
    Lanczos4 { deringing: f32 },
}

impl Default for InterpolationMethod {
    fn default() -> Self {
        Self::Lanczos3 {
            deringing: DEFAULT_DERINGING_THRESHOLD,
        }
    }
}

impl InterpolationMethod {
    /// Returns the kernel radius for this interpolation method.
    #[inline]
    pub fn kernel_radius(&self) -> usize {
        match self {
            InterpolationMethod::Nearest => 1,
            InterpolationMethod::Bilinear => 1,
            InterpolationMethod::Bicubic => 2,
            InterpolationMethod::Lanczos2 { .. } => 2,
            InterpolationMethod::Lanczos3 { .. } => 3,
            InterpolationMethod::Lanczos4 { .. } => 4,
        }
    }

    /// Returns the deringing threshold. Negative means disabled.
    /// Always negative for non-Lanczos methods.
    #[inline]
    pub fn deringing(&self) -> f32 {
        match self {
            InterpolationMethod::Lanczos2 { deringing }
            | InterpolationMethod::Lanczos3 { deringing }
            | InterpolationMethod::Lanczos4 { deringing } => *deringing,
            _ => -1.0,
        }
    }

    /// Returns whether Lanczos soft-clamping deringing is enabled.
    #[inline]
    pub fn deringing_enabled(&self) -> bool {
        self.deringing() >= 0.0
    }

    /// Returns the Lanczos parameter `a` (kernel half-width), or `None` for non-Lanczos methods.
    #[inline]
    pub fn lanczos_param(&self) -> Option<usize> {
        match self {
            InterpolationMethod::Lanczos2 { .. } => Some(2),
            InterpolationMethod::Lanczos3 { .. } => Some(3),
            InterpolationMethod::Lanczos4 { .. } => Some(4),
            _ => None,
        }
    }
}

/// Configuration for the star-matching stage.
#[derive(Debug, Clone)]
pub struct RegistrationMatchingConfig {
    /// Maximum stars to use for matching (brightest N). Default: 200.
    pub max_stars: usize,
    /// Minimum stars required in each image. `None` derives the gate from the transform model.
    pub min_stars: Option<usize>,
    /// Minimum matched star pairs to accept. Default: 8.
    pub min_matches: usize,
    /// Triangle-invariant matching configuration.
    pub triangle: TriangleConfig,
}

impl Default for RegistrationMatchingConfig {
    fn default() -> Self {
        Self {
            max_stars: 200,
            min_stars: None,
            min_matches: 8,
            triangle: TriangleConfig::default(),
        }
    }
}

impl RegistrationMatchingConfig {
    /// The star-count gate applied to each input set: the explicit `min_stars` override when set,
    /// otherwise twice the transform's minimal sample, floored at three for triangle matching.
    /// `Auto` can climb to homography, so it uses homography's eight-star gate.
    pub fn required_stars(&self, transform_type: TransformType) -> usize {
        if let Some(n) = self.min_stars {
            return n;
        }
        let model = if transform_type == TransformType::Auto {
            TransformType::Homography
        } else {
            transform_type
        };
        (2 * model.min_points()).max(3)
    }

    fn validate(&self, transform_type: TransformType) -> Result<(), RegistrationError> {
        let invalid = |msg: String| Err(RegistrationError::InvalidConfig(msg));
        if self.max_stars < 3 {
            return invalid(format!(
                "max_stars must be >= 3 for triangle matching, got {}",
                self.max_stars
            ));
        }
        if let Some(n) = self.min_stars
            && n < 3
        {
            return invalid(format!(
                "min_stars must be >= 3 for triangle matching, got {n}"
            ));
        }
        let required_stars = self.required_stars(transform_type);
        if self.max_stars < required_stars {
            return invalid(format!(
                "max_stars ({}) must be >= the star gate ({required_stars})",
                self.max_stars
            ));
        }
        let required_points = if transform_type == TransformType::Auto {
            TransformType::Homography.min_points()
        } else {
            transform_type.min_points()
        };
        if self.min_matches < required_points {
            return invalid(format!(
                "min_matches ({}) must be >= transform minimum points ({required_points})",
                self.min_matches
            ));
        }
        if !(self.triangle.ratio_tolerance > 0.0 && self.triangle.ratio_tolerance < 1.0) {
            return invalid(format!(
                "ratio_tolerance must be in (0, 1), got {}",
                self.triangle.ratio_tolerance
            ));
        }
        if self.triangle.min_votes == 0 {
            return invalid(format!(
                "min_votes must be at least 1, got {}",
                self.triangle.min_votes
            ));
        }
        Ok(())
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
/// // Use defaults (max_sigma auto-derived from star FWHM)
/// let result = register(&ref_stars, &target_stars, &Config::default())?;
///
/// // Use a preset
/// let result = register(&ref_stars, &target_stars, &Config::wide_field())?;
/// ```
#[derive(Debug, Clone)]
pub struct Config {
    /// Transformation model: Translation, Euclidean, Similarity, Affine, Homography, Auto.
    /// Default: Auto (starts with Euclidean and upgrades through Homography if needed).
    pub transform_type: TransformType,

    /// Star selection, acceptance gates, and triangle matching.
    pub matching: RegistrationMatchingConfig,

    /// Robust transform-estimation configuration.
    pub ransac: RansacConfig,

    /// Maximum acceptable RMS error in pixels. Default: 2.0.
    pub max_rms_error: f64,

    /// Optional SIP polynomial distortion correction.
    pub sip: Option<SipConfig>,

    /// Image resampling configuration.
    pub warp: WarpParams,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            transform_type: TransformType::Auto,

            matching: RegistrationMatchingConfig::default(),

            ransac: RansacConfig::default(),

            max_rms_error: 2.0,

            sip: None,

            warp: WarpParams::default(),
        }
    }
}

impl Config {
    /// Fast configuration: fewer iterations, lower quality, faster.
    pub fn fast() -> Self {
        Self {
            ransac: RansacConfig {
                max_iterations: 500,
                local_optimization: false,
                ..Default::default()
            },
            matching: RegistrationMatchingConfig {
                max_stars: 100,
                ..Default::default()
            },
            warp: WarpParams {
                method: InterpolationMethod::Bilinear,
                ..Default::default()
            },
            ..Self::default()
        }
    }

    /// Precise configuration: more iterations, SIP correction enabled.
    pub fn precise() -> Self {
        Self {
            ransac: RansacConfig {
                max_iterations: 5000,
                confidence: 0.999,
                ..Default::default()
            },
            sip: Some(SipConfig::default()),
            max_rms_error: 1.0,
            ..Self::default()
        }
    }

    /// Wide-field configuration: handles lens distortion.
    pub fn wide_field() -> Self {
        Self {
            transform_type: TransformType::Homography,
            sip: Some(SipConfig::default()),
            ransac: RansacConfig {
                max_rotation: None,
                scale_range: None,
                ..Default::default()
            },
            ..Self::default()
        }
    }

    /// Precise wide-field configuration: high accuracy with lens distortion handling.
    ///
    /// Builds on `wide_field()` (Homography, unlimited rotation/scale) with
    /// tighter matching from `precise()` plus extra stars and stricter confidence.
    pub fn precise_wide_field() -> Self {
        Self {
            ransac: RansacConfig {
                max_iterations: 5000,
                confidence: 0.9999,
                max_rotation: None,
                scale_range: None,
                ..Default::default()
            },
            max_rms_error: 1.0,
            // Stricter than precise(): more stars, tighter matching
            matching: RegistrationMatchingConfig {
                max_stars: 500,
                min_matches: 20,
                triangle: TriangleConfig {
                    ratio_tolerance: 0.02,
                    ..Default::default()
                },
                ..Default::default()
            },
            // From wide_field(): Homography, SIP, unlimited rotation/scale
            ..Self::wide_field()
        }
    }

    /// Mosaic configuration: allows larger offsets and rotations.
    pub fn mosaic() -> Self {
        Self {
            ransac: RansacConfig {
                max_rotation: None,
                scale_range: Some((0.5, 2.0)),
                ..Default::default()
            },
            ..Self::default()
        }
    }

    /// Validate all configuration parameters.
    ///
    /// # Errors
    ///
    /// Returns [`RegistrationError::InvalidConfig`] if any parameter is invalid:
    /// - `max_stars` or `min_stars` < 3
    /// - `max_stars` < `min_stars`
    /// - `min_matches` < transform minimum points
    /// - `ratio_tolerance` not in (0, 1)
    /// - `min_votes` < 1
    /// - invalid RANSAC configuration
    /// - `max_rms_error` <= 0
    /// - invalid SIP configuration (when enabled)
    /// - invalid warp configuration
    pub fn validate(&self) -> Result<(), RegistrationError> {
        let invalid = |msg: String| Err(RegistrationError::InvalidConfig(msg));

        self.matching.validate(self.transform_type)?;
        self.ransac.validate()?;

        // Quality
        if self.max_rms_error <= 0.0 {
            return invalid(format!(
                "max_rms_error must be positive, got {}",
                self.max_rms_error
            ));
        }

        if let Some(sip) = &self.sip {
            sip.validate()?;
        }
        self.warp.validate()?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default_values() {
        let config = Config::default();
        assert_eq!(config.transform_type, TransformType::Auto);
        assert_eq!(config.matching.max_stars, 200);
        assert_eq!(config.matching.min_stars, None);
        // Auto gates like Homography: 2 × 4 minimal points = 8.
        assert_eq!(config.matching.required_stars(config.transform_type), 8);
        assert_eq!(config.matching.required_stars(TransformType::Similarity), 4);
        assert_eq!(
            RegistrationMatchingConfig {
                min_stars: Some(20),
                ..Default::default()
            }
            .required_stars(TransformType::Auto),
            20
        );
        assert_eq!(config.matching.min_matches, 8);
        assert!((config.matching.triangle.ratio_tolerance - 0.01).abs() < 1e-10);
        assert_eq!(config.matching.triangle.min_votes, 3);
        assert!(config.matching.triangle.check_orientation);
        assert_eq!(config.ransac.max_iterations, 2000);
        assert!((config.ransac.confidence - 0.995).abs() < 1e-10);
        assert!((config.ransac.min_inlier_ratio - 0.3).abs() < 1e-10);
        assert!(config.ransac.seed.is_none());
        assert!(config.ransac.local_optimization);
        assert_eq!(config.ransac.lo_iterations, 10);
        assert!((config.max_rms_error - 2.0).abs() < 1e-10);
        assert!(config.sip.is_none());
        assert_eq!(
            config.warp.method,
            InterpolationMethod::Lanczos3 {
                deringing: DEFAULT_DERINGING_THRESHOLD
            }
        );
        assert!(config.warp.method.deringing_enabled());
        assert!((config.warp.border_value - 0.0).abs() < 1e-10);
    }

    #[test]
    fn test_config_fast_preset() {
        let config = Config::fast();
        assert_eq!(config.ransac.max_iterations, 500);
        assert_eq!(config.matching.max_stars, 100);
        assert!(!config.ransac.local_optimization);
        assert_eq!(config.warp.method, InterpolationMethod::Bilinear);
        config.validate().unwrap();
    }

    #[test]
    fn test_config_precise_preset() {
        let config = Config::precise();
        assert_eq!(config.ransac.max_iterations, 5000);
        assert!((config.ransac.confidence - 0.999).abs() < 1e-10);
        assert!(config.sip.is_some());
        assert!((config.max_rms_error - 1.0).abs() < 1e-10);
        config.validate().unwrap();
    }

    #[test]
    fn test_config_wide_field_preset() {
        let config = Config::wide_field();
        assert_eq!(config.transform_type, TransformType::Homography);
        assert!(config.sip.is_some());
        assert!(config.ransac.max_rotation.is_none());
        assert!(config.ransac.scale_range.is_none());
        config.validate().unwrap();
    }

    #[test]
    fn test_config_precise_wide_field_preset() {
        let config = Config::precise_wide_field();
        assert_eq!(config.transform_type, TransformType::Homography);
        assert_eq!(config.matching.max_stars, 500);
        assert_eq!(config.matching.min_matches, 20);
        assert!((config.matching.triangle.ratio_tolerance - 0.02).abs() < 1e-10);
        assert_eq!(config.ransac.max_iterations, 5000);
        assert!((config.ransac.confidence - 0.9999).abs() < 1e-10);
        assert!(config.sip.is_some());
        assert!((config.max_rms_error - 1.0).abs() < 1e-10);
        // Inherits unlimited rotation/scale from wide_field()
        assert!(config.ransac.max_rotation.is_none());
        assert!(config.ransac.scale_range.is_none());
        config.validate().unwrap();
    }

    #[test]
    fn test_config_mosaic_preset() {
        let config = Config::mosaic();
        assert!(config.ransac.max_rotation.is_none());
        assert_eq!(config.ransac.scale_range, Some((0.5, 2.0)));
        config.validate().unwrap();
    }

    #[test]
    fn test_config_custom() {
        let config = Config {
            transform_type: TransformType::Similarity,
            ransac: RansacConfig {
                max_iterations: 1000,
                ..Default::default()
            },
            ..Config::default()
        };
        assert_eq!(config.transform_type, TransformType::Similarity);
        assert_eq!(config.ransac.max_iterations, 1000);
        config.validate().unwrap();
    }

    #[test]
    fn test_interpolation_method_kernel_radius() {
        assert_eq!(InterpolationMethod::Nearest.kernel_radius(), 1);
        assert_eq!(InterpolationMethod::Bilinear.kernel_radius(), 1);
        assert_eq!(InterpolationMethod::Bicubic.kernel_radius(), 2);
        assert_eq!(
            InterpolationMethod::Lanczos2 { deringing: 0.3 }.kernel_radius(),
            2
        );
        assert_eq!(
            InterpolationMethod::Lanczos3 { deringing: 0.3 }.kernel_radius(),
            3
        );
        assert_eq!(
            InterpolationMethod::Lanczos4 { deringing: 0.3 }.kernel_radius(),
            4
        );
    }

    #[test]
    fn test_deringing_threshold() {
        assert!(InterpolationMethod::Lanczos3 { deringing: 0.3 }.deringing_enabled());
        assert!(InterpolationMethod::Lanczos3 { deringing: 0.0 }.deringing_enabled());
        assert!(!InterpolationMethod::Lanczos3 { deringing: -1.0 }.deringing_enabled());
        assert!(!InterpolationMethod::Bilinear.deringing_enabled());
        assert!(
            (InterpolationMethod::Lanczos3 { deringing: 0.3 }.deringing() - 0.3).abs()
                < f32::EPSILON
        );
        assert!(InterpolationMethod::Nearest.deringing() < 0.0);
    }

    #[test]
    fn test_lanczos_param() {
        // Non-Lanczos methods return None
        assert_eq!(InterpolationMethod::Nearest.lanczos_param(), None);
        assert_eq!(InterpolationMethod::Bilinear.lanczos_param(), None);
        assert_eq!(InterpolationMethod::Bicubic.lanczos_param(), None);
        // Lanczos methods return their parameter a
        assert_eq!(
            InterpolationMethod::Lanczos2 { deringing: 0.3 }.lanczos_param(),
            Some(2)
        );
        assert_eq!(
            InterpolationMethod::Lanczos3 { deringing: -1.0 }.lanczos_param(),
            Some(3)
        );
        assert_eq!(
            InterpolationMethod::Lanczos4 { deringing: 0.0 }.lanczos_param(),
            Some(4)
        );
    }

    #[test]
    fn test_interpolation_method_default() {
        let method = InterpolationMethod::default();
        assert_eq!(
            method,
            InterpolationMethod::Lanczos3 {
                deringing: DEFAULT_DERINGING_THRESHOLD
            }
        );
        // DEFAULT_DERINGING_THRESHOLD = 0.3
        assert!((DEFAULT_DERINGING_THRESHOLD - 0.3).abs() < f32::EPSILON);
    }

    #[test]
    fn test_config_validation_rejects_invalid() {
        // Each case: a single out-of-range field and the substring its error must contain.
        let cases: &[(Config, &str)] = &[
            (
                Config {
                    ransac: RansacConfig {
                        max_iterations: 0,
                        ..Default::default()
                    },
                    ..Config::default()
                },
                "ransac max_iterations must be positive",
            ),
            (
                Config {
                    matching: RegistrationMatchingConfig {
                        max_stars: 2,
                        ..Default::default()
                    },
                    ..Config::default()
                },
                "max_stars must be >= 3",
            ),
            (
                Config {
                    matching: RegistrationMatchingConfig {
                        min_stars: Some(2),
                        ..Default::default()
                    },
                    ..Config::default()
                },
                "min_stars must be >= 3",
            ),
            (
                Config {
                    matching: RegistrationMatchingConfig {
                        max_stars: 5,
                        min_stars: Some(10),
                        ..Default::default()
                    },
                    ..Config::default()
                },
                "max_stars (5) must be >= the star gate (10)",
            ),
            (
                Config {
                    matching: RegistrationMatchingConfig {
                        triangle: TriangleConfig {
                            ratio_tolerance: 0.0,
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    ..Config::default()
                },
                "ratio_tolerance must be in (0, 1)",
            ),
            (
                Config {
                    matching: RegistrationMatchingConfig {
                        triangle: TriangleConfig {
                            ratio_tolerance: 1.0,
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    ..Config::default()
                },
                "ratio_tolerance must be in (0, 1)",
            ),
            (
                Config {
                    matching: RegistrationMatchingConfig {
                        triangle: TriangleConfig {
                            min_votes: 0,
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    ..Config::default()
                },
                "min_votes must be at least 1",
            ),
            (
                Config {
                    ransac: RansacConfig {
                        confidence: 1.5,
                        ..Default::default()
                    },
                    ..Config::default()
                },
                "confidence must be in [0, 1]",
            ),
            (
                Config {
                    ransac: RansacConfig {
                        min_inlier_ratio: 0.0,
                        ..Default::default()
                    },
                    ..Config::default()
                },
                "min_inlier_ratio must be in (0, 1]",
            ),
            (
                Config {
                    ransac: RansacConfig {
                        local_optimization: true,
                        lo_iterations: 0,
                        ..Default::default()
                    },
                    ..Config::default()
                },
                "lo_iterations must be positive",
            ),
            (
                Config {
                    ransac: RansacConfig {
                        max_rotation: Some(-0.1),
                        ..Default::default()
                    },
                    ..Config::default()
                },
                "max_rotation must be positive",
            ),
            (
                Config {
                    ransac: RansacConfig {
                        scale_range: Some((1.5, 0.5)),
                        ..Default::default()
                    },
                    ..Config::default()
                },
                "scale_range must have 0 < min < max",
            ),
            (
                Config {
                    max_rms_error: 0.0,
                    ..Config::default()
                },
                "max_rms_error must be positive",
            ),
            (
                Config {
                    sip: Some(SipConfig {
                        order: 6,
                        ..Default::default()
                    }),
                    ..Config::default()
                },
                "SIP order must be 2-5",
            ),
            (
                // Homography needs 4 points, so min_matches = 3 is too few.
                Config {
                    transform_type: TransformType::Homography,
                    matching: RegistrationMatchingConfig {
                        min_matches: 3,
                        ..Default::default()
                    },
                    ..Config::default()
                },
                "min_matches (3) must be >= transform minimum points (4)",
            ),
            (
                Config {
                    warp: WarpParams {
                        border_value: f32::NAN,
                        ..Default::default()
                    },
                    ..Config::default()
                },
                "border_value must be finite",
            ),
        ];

        for (config, expected) in cases {
            let err = config.validate().unwrap_err();
            assert!(
                matches!(err, RegistrationError::InvalidConfig(_)),
                "expected InvalidConfig for case '{expected}', got {err:?}"
            );
            let msg = err.to_string();
            assert!(
                msg.contains(expected),
                "expected error to contain '{expected}', got '{msg}'"
            );
        }
    }

    #[test]
    fn test_config_lo_iterations_zero_ok_when_lo_disabled() {
        // lo_iterations is only validated when local_optimization is enabled.
        let config = Config {
            ransac: RansacConfig {
                local_optimization: false,
                lo_iterations: 0,
                ..Default::default()
            },
            ..Config::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_presets_differ() {
        // Verify presets produce different configs (parameter sensitivity)
        let default = Config::default();
        let fast = Config::fast();
        let precise = Config::precise();

        // fast has fewer iterations than default
        assert!(fast.ransac.max_iterations < default.ransac.max_iterations);
        // precise has more iterations than default
        assert!(precise.ransac.max_iterations > default.ransac.max_iterations);
        // precise has tighter RMS tolerance
        assert!(precise.max_rms_error < default.max_rms_error);
        // fast disables LO, default enables it
        assert!(!fast.ransac.local_optimization);
        assert!(default.ransac.local_optimization);
    }

    #[test]
    fn test_config_all_presets_validate() {
        Config::default().validate().unwrap();
        Config::fast().validate().unwrap();
        Config::precise().validate().unwrap();
        Config::wide_field().validate().unwrap();
        Config::precise_wide_field().validate().unwrap();
        Config::mosaic().validate().unwrap();
    }
}
