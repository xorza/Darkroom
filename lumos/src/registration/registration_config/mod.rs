//! Configuration for the registration module.

use crate::config_field::ConfigField;
use crate::error::InvalidConfigField;
use crate::registration::distortion::sip::SipConfig;
use crate::registration::ransac::ransac_config::{RansacConfig, ScaleRange};
use crate::registration::transform_model::{ModelChoice, TransformModel};
use crate::registration::triangle::triangle_config::TriangleConfig;

/// The most stars matching takes from a catalog: the brightest 2000 over-determine any model a
/// registration fits many times over, and the triangles each star builds with its neighbours, and
/// the votes they cast, grow with every star past them.
pub const MAX_STARS: usize = 2000;

/// Configuration for the star-matching stage.
#[derive(Debug, Clone)]
pub struct RegistrationMatchingConfig {
    /// Maximum stars to use for matching (brightest N), at most [`MAX_STARS`]. Default: 200.
    pub max_stars: usize,
    /// Minimum matched star pairs to accept. Default: 8.
    pub min_matches: usize,
    /// Triangle-invariant matching configuration.
    pub triangle: TriangleConfig,
}

impl Default for RegistrationMatchingConfig {
    fn default() -> Self {
        Self {
            max_stars: 200,
            min_matches: 8,
            triangle: TriangleConfig::default(),
        }
    }
}

impl RegistrationMatchingConfig {
    fn validate(&self, model: TransformModel) -> Result<(), InvalidConfigField> {
        ConfigField::count("max_stars", self.max_stars)
            .must_be("at least 3 for triangle matching", self.max_stars >= 3)?;
        ConfigField::count("max_stars", self.max_stars).must_be_against_count(
            "at most MAX_STARS",
            MAX_STARS,
            self.max_stars <= MAX_STARS,
        )?;
        let required_stars = model.required_stars();
        ConfigField::count("max_stars", self.max_stars).must_be_against_count(
            "at least the star gate",
            required_stars,
            self.max_stars >= required_stars,
        )?;
        let required_points = model.most_general().min_points();
        ConfigField::count("min_matches", self.min_matches).must_be_against_count(
            "at least the transform's minimum point count",
            required_points,
            self.min_matches >= required_points,
        )?;
        self.triangle.validate()
    }
}

/// Configuration for image registration.
///
/// Defaults are calibrated against OpenCV, Astroalign and PixInsight; most callers set only
/// `model`.
///
/// # Example
///
/// ```no_run
/// use lumos::{ModelChoice, Registrar, RegistrationConfig, Star, TransformModel, TransformType};
///
/// # fn example(
/// #     ref_stars: Vec<Star>,
/// #     targets: &[Vec<Star>],
/// # ) -> Result<(), Box<dyn std::error::Error>> {
/// // A preset, every target registered to one reference.
/// let registrar = Registrar::checked(RegistrationConfig::wide_field())?;
/// let reference = registrar.reference(ref_stars)?;
/// for target in targets {
///     let result = reference.register(target)?;
///     println!("{} matches, RMS {:.2} px", result.matches().len(), result.rms_error_px());
/// }
///
/// // A fixed model.
/// let registrar = Registrar::checked(RegistrationConfig {
///     model: TransformModel::Plain(ModelChoice::Fixed(TransformType::Similarity)),
///     ..RegistrationConfig::default()
/// })?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct RegistrationConfig {
    /// Which model to fit, with or without a SIP distortion correction, or `Auto` to fit every
    /// model and take the one of lowest GRIC. Default: `Auto` with no correction.
    pub model: TransformModel,

    /// Star selection, acceptance gates, and triangle matching.
    pub matching: RegistrationMatchingConfig,

    /// Robust transform-estimation configuration.
    pub ransac: RansacConfig,

    /// Maximum acceptable RMS error in pixels. Default: 2.0.
    pub max_rms_error_px: f64,
}

impl Default for RegistrationConfig {
    fn default() -> Self {
        Self {
            model: TransformModel::default(),
            matching: RegistrationMatchingConfig::default(),
            ransac: RansacConfig::default(),
            max_rms_error_px: 2.0,
        }
    }
}

impl RegistrationConfig {
    /// Fast configuration: fewer iterations, lower quality, faster.
    pub fn fast() -> Self {
        Self {
            ransac: RansacConfig {
                max_iterations: 500,
                local_optimization: None,
                ..Default::default()
            },
            matching: RegistrationMatchingConfig {
                max_stars: 100,
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
            model: TransformModel::WithSip(ModelChoice::Auto, SipConfig::default()),
            max_rms_error_px: 1.0,
            ..Self::default()
        }
    }

    /// Wide-field configuration: handles lens distortion, by a SIP correction under the model GRIC
    /// chooses up to an affine map.
    pub fn wide_field() -> Self {
        Self {
            model: TransformModel::WithSip(ModelChoice::Auto, SipConfig::default()),
            ransac: RansacConfig {
                max_rotation_rad: None,
                scale_range: None,
                ..Default::default()
            },
            ..Self::default()
        }
    }

    /// Precise wide-field configuration: high accuracy with lens distortion handling.
    ///
    /// Builds on [`Self::wide_field`] (SIP up to an affine map, unlimited rotation/scale) and takes
    /// its iteration budget and RMS gate from [`Self::precise`], with extra stars, tighter matching
    /// and stricter confidence of its own.
    ///
    /// Both bases are read, not copied, so the preset cannot drift from them.
    pub fn precise_wide_field() -> Self {
        let precise = Self::precise();
        Self {
            ransac: RansacConfig {
                confidence: 0.9999,
                max_rotation_rad: None,
                scale_range: None,
                ..precise.ransac
            },
            max_rms_error_px: precise.max_rms_error_px,
            matching: RegistrationMatchingConfig {
                max_stars: 500,
                min_matches: 20,
                triangle: TriangleConfig {
                    ratio_tolerance: 0.02,
                    ..Default::default()
                },
            },
            ..Self::wide_field()
        }
    }

    /// For frames of another plate scale or orientation than the reference: RANSAC takes any
    /// rotation and a scale from half to twice the reference's, where the default holds both near
    /// the reference's — a second camera, a focal reducer, a meridian flip of a rotated camera. It
    /// changes nothing else.
    pub fn wide_scale() -> Self {
        Self {
            ransac: RansacConfig {
                max_rotation_rad: None,
                scale_range: Some(ScaleRange::new(0.5, 2.0)),
                ..Default::default()
            },
            ..Self::default()
        }
    }

    /// Validate all configuration parameters, returning the first offending field.
    pub(crate) fn validate(&self) -> Result<(), InvalidConfigField> {
        self.matching.validate(self.model)?;
        self.ransac.validate()?;
        ConfigField::new("max_rms_error_px", self.max_rms_error_px)
            .finite_and("finite and positive", |value| value > 0.0)?;
        if let Some(sip) = self.model.sip() {
            sip.validate()?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests;
