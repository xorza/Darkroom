//! Configuration for the registration module.

use crate::config_value::ConfigValue;
use crate::error::InvalidConfigField;
use crate::registration::distortion::sip::SipConfig;
use crate::registration::ransac::ransac_config::{RansacConfig, ScaleRange};
use crate::registration::registrar::Registrar;
use crate::registration::transform_model::TransformModel;
use crate::registration::triangle::triangle_config::TriangleConfig;

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
        ConfigValue::count("max_stars", self.max_stars)
            .must_be("at least 3 for triangle matching", self.max_stars >= 3)?;
        if let Some(min_stars) = self.min_stars {
            ConfigValue::count("min_stars", min_stars)
                .must_be("at least 3 for triangle matching", min_stars >= 3)?;
        }
        let required_stars = self.required_stars(model);
        ConfigValue::count("max_stars", self.max_stars).must_be_against_count(
            "at least the star gate",
            required_stars,
            self.max_stars >= required_stars,
        )?;
        let required_points = model.most_general().min_points();
        ConfigValue::count("min_matches", self.min_matches).must_be_against_count(
            "at least the transform's minimum point count",
            required_points,
            self.min_matches >= required_points,
        )?;
        self.triangle.validate()
    }
}

/// Configuration for image registration.
///
/// All parameters have sensible defaults calibrated against industry standards
/// (OpenCV, Astroalign, PixInsight). Most users only need to set `model` if they
/// want a specific model.
///
/// # Example
///
/// ```no_run
/// use lumos::{RegistrationConfig, Star};
///
/// # fn example(
/// #     ref_stars: &[Star],
/// #     target_stars: &[Star],
/// # ) -> Result<(), Box<dyn std::error::Error>> {
/// // Use defaults
/// let result = RegistrationConfig::default()
///     .registrar()?
///     .register(ref_stars, target_stars)?;
///
/// // Use a preset
/// let result = RegistrationConfig::wide_field()
///     .registrar()?
///     .register(ref_stars, target_stars)?;
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
    pub max_rms_error: f64,
}

impl Default for RegistrationConfig {
    fn default() -> Self {
        Self {
            model: TransformModel::default(),
            matching: RegistrationMatchingConfig::default(),
            ransac: RansacConfig::default(),
            max_rms_error: 2.0,
        }
    }
}

impl RegistrationConfig {
    /// This configuration checked, ready to register frame pairs: a run that registers many
    /// pairs checks it once here, and each pair's failure is then the pair's own.
    ///
    /// # Errors
    ///
    /// The first field outside its range: a star count below triangle matching's three, a match
    /// floor below the model's minimal set, an RMS gate that is not finite and positive, or a
    /// SIP, RANSAC or triangle setting out of its own range.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use lumos::{RegistrationConfig, Star, TransformModel, TransformType};
    ///
    /// # fn example(
    /// #     ref_stars: &[Star],
    /// #     target_stars: &[Star],
    /// # ) -> Result<(), Box<dyn std::error::Error>> {
    /// // With defaults
    /// let result = RegistrationConfig::default()
    ///     .registrar()?
    ///     .register(ref_stars, target_stars)?;
    ///
    /// // With custom config
    /// let config = RegistrationConfig {
    ///     model: TransformModel::Fixed(TransformType::Similarity),
    ///     ..RegistrationConfig::default()
    /// };
    /// let result = config.registrar()?.register(ref_stars, target_stars)?;
    ///
    /// println!("Matched {} stars", result.num_inliers());
    /// println!("RMS error: {:.2} pixels", result.rms_error());
    /// # Ok(())
    /// # }
    /// ```
    pub fn registrar(&self) -> Result<Registrar<'_>, InvalidConfigField> {
        self.validate()?;
        Ok(Registrar::new(self))
    }

    /// Fast configuration: fewer iterations, lower quality, faster.
    pub fn fast() -> Self {
        Self {
            ransac: RansacConfig {
                max_iterations: 500,
                local_max_iterations: 0,
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
            model: TransformModel::Auto {
                sip: Some(SipConfig::default()),
            },
            max_rms_error: 1.0,
            ..Self::default()
        }
    }

    /// Wide-field configuration: handles lens distortion, by a SIP correction under the model GRIC
    /// chooses up to an affine map.
    pub fn wide_field() -> Self {
        Self {
            model: TransformModel::Auto {
                sip: Some(SipConfig::default()),
            },
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
    /// Builds on [`Self::wide_field`] (SIP up to an affine map, unlimited rotation/scale) and takes
    /// its iteration budget and RMS gate from [`Self::precise`], with extra stars, tighter matching
    /// and stricter confidence of its own.
    ///
    /// Both bases are read rather than copied: a preset that re-typed `precise()`'s numbers as
    /// literals would drift from it the first time either was tuned.
    pub fn precise_wide_field() -> Self {
        let precise = Self::precise();
        Self {
            ransac: RansacConfig {
                // Stricter than precise(), which is this preset's own choice.
                confidence: 0.9999,
                // From wide_field(): no rotation or scale prior.
                max_rotation: None,
                scale_range: None,
                // From precise(): the iteration budget.
                ..precise.ransac
            },
            max_rms_error: precise.max_rms_error,
            // Stricter than precise(): more stars, tighter matching.
            matching: RegistrationMatchingConfig {
                max_stars: 500,
                min_matches: 20,
                triangle: TriangleConfig {
                    ratio_tolerance: 0.02,
                    ..Default::default()
                },
                ..Default::default()
            },
            // From wide_field(): SIP up to an affine map, unlimited rotation/scale.
            ..Self::wide_field()
        }
    }

    /// Mosaic configuration: allows larger offsets and rotations.
    pub fn mosaic() -> Self {
        Self {
            ransac: RansacConfig {
                max_rotation: None,
                scale_range: Some(ScaleRange { min: 0.5, max: 2.0 }),
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
    pub(crate) fn validate(&self) -> Result<(), InvalidConfigField> {
        self.matching.validate(self.model)?;
        self.ransac.validate()?;
        ConfigValue::new("max_rms_error", self.max_rms_error)
            .finite_and("finite and positive", |value| value > 0.0)?;
        if let Some(sip) = self.model.sip() {
            sip.validate()?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests;
