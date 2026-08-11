//! Configuration for the registration module.

use crate::error::InvalidConfigField;
use crate::stacking::registration::distortion::sip::SipConfig;
use crate::stacking::registration::ransac::config::RansacConfig;
use crate::stacking::registration::transform::{TransformModel, TransformType};
use crate::stacking::registration::triangle::TriangleConfig;

/// Interpolation method for image resampling.
///
/// Adding one means editing four `match`es — this enum's two accessors, `plane::warp`'s dispatch,
/// and `quality_at`'s — which is deliberate rather than an oversight waiting to be unified. The
/// warp's dispatch selects between `lanczos_inner::<A, SIZE>` monomorphizations and per-method SIMD
/// kernels; routing it through one shared tap-weight abstraction would erase exactly the constants
/// those kernels are specialized on.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
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

    /// Returns the Lanczos parameter `a` (kernel half-width), or `None` for non-Lanczos methods.
    #[inline]
    pub(crate) fn lanczos_param(&self) -> Option<usize> {
        match self {
            InterpolationMethod::Lanczos2 => Some(2),
            InterpolationMethod::Lanczos3 => Some(3),
            InterpolationMethod::Lanczos4 => Some(4),
            _ => None,
        }
    }
}

/// Configuration for inverse-mapped image resampling.
#[derive(Debug, Clone, Copy)]
pub struct WarpParams {
    /// Resampling kernel.
    pub method: InterpolationMethod,
    /// Fill for source positions outside the closed source pixel footprint.
    ///
    /// Positions inside the footprint with partial kernel support are reconstructed from real
    /// source pixels only.
    pub border_value: f32,
}

impl Default for WarpParams {
    fn default() -> Self {
        Self {
            method: InterpolationMethod::default(),
            border_value: 0.0,
        }
    }
}

impl WarpParams {
    fn validate(&self) -> Result<(), InvalidConfigField> {
        InvalidConfigField::finite_only("warp border_value", self.border_value)
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
    /// otherwise twice the model's minimal sample, floored at three for triangle matching.
    /// Sized against [`TransformModel::most_general`], since `Auto` can climb to homography and
    /// must arrive with enough stars to fit one.
    pub fn required_stars(&self, model: TransformModel) -> usize {
        if let Some(n) = self.min_stars {
            return n;
        }
        (2 * model.most_general().min_points()).max(3)
    }

    fn validate(&self, model: TransformModel) -> Result<(), InvalidConfigField> {
        InvalidConfigField::check(
            self.max_stars >= 3,
            "max_stars",
            "at least 3 for triangle matching",
            self.max_stars as f64,
        )?;
        if let Some(min_stars) = self.min_stars {
            InvalidConfigField::check(
                min_stars >= 3,
                "min_stars",
                "at least 3 for triangle matching",
                min_stars as f64,
            )?;
        }
        let required_stars = self.required_stars(model);
        InvalidConfigField::check_against(
            self.max_stars >= required_stars,
            "max_stars",
            "at least the star gate",
            self.max_stars as f64,
            required_stars as f64,
        )?;
        let required_points = model.most_general().min_points();
        InvalidConfigField::check_against(
            self.min_matches >= required_points,
            "min_matches",
            "at least the transform's minimum point count",
            self.min_matches as f64,
            required_points as f64,
        )?;
        self.triangle.validate()
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
/// use lumos::{RegistrationConfig, register};
///
/// // Use defaults (max_sigma auto-derived from star FWHM)
/// let result = register(&ref_stars, &target_stars, &RegistrationConfig::default())?;
///
/// // Use a preset
/// let result = register(
///     &ref_stars,
///     &target_stars,
///     &RegistrationConfig::wide_field(),
/// )?;
/// ```
#[derive(Debug, Clone)]
pub struct Config {
    /// Which model to fit, or `Auto` to ladder Euclidean → Similarity → Affine → Homography and
    /// take the first that fits. Default: `Auto`.
    pub transform_type: TransformModel,

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
            transform_type: TransformModel::Auto,

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
            transform_type: TransformModel::Fixed(TransformType::Homography),
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
    /// Returns the offending field if any parameter is invalid:
    /// - `max_stars` or `min_stars` < 3
    /// - `max_stars` < `min_stars`
    /// - `min_matches` < transform minimum points
    /// - `ratio_tolerance` not in (0, 1)
    /// - `min_votes` < 1
    /// - invalid RANSAC configuration
    /// - `max_rms_error` non-finite or <= 0
    /// - invalid SIP configuration (when enabled)
    /// - invalid warp configuration
    pub fn validate(&self) -> Result<(), InvalidConfigField> {
        self.matching.validate(self.transform_type)?;
        self.ransac.validate()?;
        InvalidConfigField::finite(
            "max_rms_error",
            "finite and positive",
            self.max_rms_error,
            |value| value > 0.0,
        )?;
        if let Some(sip) = &self.sip {
            sip.validate()?;
        }
        self.warp.validate()?;

        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::stacking::registration::config::{InterpolationMethod, WarpParams};

    pub(crate) fn warp_params(method: InterpolationMethod) -> WarpParams {
        WarpParams {
            method,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests;
