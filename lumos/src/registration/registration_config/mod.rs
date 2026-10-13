//! Configuration for the registration module.

use std::time::Instant;

use glam::DVec2;

use crate::config_value::{self, ConfigValue};
use crate::error::InvalidConfigField;
use crate::math::statistics::median_mut;
use crate::registration::distortion::sip::{SipConfig, SipModel};
use crate::registration::error::{RegistrationCatalog, RegistrationError, StarField};
use crate::registration::final_fit::fit_catalogs::FitCatalogs;
use crate::registration::kd_tree::KdTree;
use crate::registration::model_search::ModelSearch;
use crate::registration::point_pairs::PointPairs;
use crate::registration::ransac::ransac_config::{RansacConfig, ScaleRange};
use crate::registration::registration_result::RegistrationResult;
use crate::registration::resample::warp_params::{InterpolationMethod, WarpParams};
use crate::registration::transform::{TransformModel, TransformType};
use crate::registration::triangle::triangle_config::TriangleConfig;
use crate::registration::tuning;
use crate::star_detection::star::Star;

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
        ConfigValue::count("max_stars", self.max_stars).must_be_against(
            "at least the star gate",
            config_value::exact(required_stars),
            self.max_stars >= required_stars,
        )?;
        let required_points = model.most_general().min_points();
        ConfigValue::count("min_matches", self.min_matches).must_be_against(
            "at least the transform's minimum point count",
            config_value::exact(required_points),
            self.min_matches >= required_points,
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
/// ```no_run
/// use lumos::{RegistrationConfig, Star};
///
/// # fn example(ref_stars: &[Star], target_stars: &[Star]) -> Result<(), lumos::RegistrationError> {
/// // Use defaults
/// let result = RegistrationConfig::default().register(ref_stars, target_stars)?;
///
/// // Use a preset
/// let result = RegistrationConfig::wide_field().register(ref_stars, target_stars)?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct RegistrationConfig {
    /// Which model to fit, or `Auto` to fit every model from Euclidean to Homography and take the
    /// one of lowest GRIC. Default: `Auto`.
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

impl Default for RegistrationConfig {
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

impl RegistrationConfig {
    /// Register two sets of star positions.
    ///
    /// This is the main entry point for image registration. It finds the geometric
    /// transformation that maps reference star positions to target star positions.
    ///
    /// Stars should be sorted by brightness (flux) in descending order for best results.
    ///
    /// The RANSAC `max_sigma` parameter is automatically derived from the median FWHM
    /// of the input stars, providing optimal noise tolerance for the seeing conditions.
    ///
    /// # Errors
    ///
    /// [`RegistrationError::InvalidConfig`] if the config fails validation (see
    /// [`Self::validate`]), and the matching/accuracy failures below. A caller that runs this per
    /// frame pair must treat `InvalidConfig` apart from the rest: every other variant describes one
    /// pair, and is a frame to drop, but an invalid config fails every pair identically and is the
    /// run's own fault.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use lumos::{RegistrationConfig, Star, TransformModel, TransformType};
    ///
    /// # fn example(
    /// #     ref_stars: &[Star],
    /// #     target_stars: &[Star],
    /// # ) -> Result<(), lumos::RegistrationError> {
    /// // With defaults
    /// let result = RegistrationConfig::default().register(ref_stars, target_stars)?;
    ///
    /// // With custom config
    /// let config = RegistrationConfig {
    ///     transform_type: TransformModel::Fixed(TransformType::Similarity),
    ///     ..RegistrationConfig::default()
    /// };
    /// let result = config.register(ref_stars, target_stars)?;
    ///
    /// println!("Matched {} stars", result.num_inliers());
    /// println!("RMS error: {:.2} pixels", result.rms_error());
    /// # Ok(())
    /// # }
    /// ```
    pub fn register(
        &self,
        ref_stars: &[Star],
        target_stars: &[Star],
    ) -> Result<RegistrationResult, RegistrationError> {
        self.validate()?;
        validate_catalog(ref_stars, RegistrationCatalog::Reference)?;
        validate_catalog(target_stars, RegistrationCatalog::Target)?;
        let start = Instant::now();

        // The gate is keyed to the transform model unless `min_stars` overrides it.
        let required_stars = self.matching.required_stars(self.transform_type);
        for stars in [ref_stars, target_stars] {
            if stars.len() < required_stars {
                return Err(RegistrationError::InsufficientStars {
                    found: stars.len(),
                    required: required_stars,
                });
            }
        }

        let max_sigma = tuning::max_sigma_from_fwhm(median_fwhm(ref_stars, target_stars));
        let catalogs = FitCatalogs::new(ref_stars, target_stars).ok_or(
            RegistrationError::InsufficientStars {
                found: 0,
                required: required_stars,
            },
        )?;
        let sip = self.sip.as_ref().map(|sip| SipModel {
            order: sip.order,
            origin: sip
                .reference_point
                .unwrap_or_else(|| bounding_box_centre(ref_stars)),
        });

        // The brightest `max_stars` of each set, as trees: triangle matching forms its triangles
        // over both, and RANSAC samples their matches for every model.
        let brightest = |stars: &[Star]| {
            KdTree::build(
                stars
                    .iter()
                    .take(self.matching.max_stars)
                    .map(|s| s.pos)
                    .collect(),
            )
            .expect("the star gates leave at least three stars in each set")
        };
        let ref_tree = brightest(ref_stars);
        let target_tree = brightest(target_stars);

        let t0 = Instant::now();
        let matches = self
            .matching
            .triangle
            .match_trees(&ref_tree, &target_tree, max_sigma);
        let triangle_ms = t0.elapsed().as_secs_f64() * 1000.0;
        tracing::debug!(
            triangle_ms,
            num_matches = matches.len(),
            "Triangle matching complete"
        );

        if matches.len() < self.matching.min_matches {
            return Err(RegistrationError::NoMatchingPatterns);
        }

        let search = ModelSearch {
            pairs: PointPairs::from_matches(&matches, ref_tree.points(), target_tree.points()),
            confidences: matches.iter().map(|m| m.confidence).collect(),
            catalogs: &catalogs,
            max_sigma,
            config: self,
        };
        let fit = search.fit(self.transform_type, sip)?;

        let result = RegistrationResult::new(fit.warp.transform, fit.warp.sip, fit.matches)
            .with_elapsed(start.elapsed().as_secs_f64() * 1000.0);
        let rms_error = result.rms_error();
        if rms_error > self.max_rms_error {
            return Err(RegistrationError::AccuracyTooLow {
                rms_error,
                max_allowed: self.max_rms_error,
            });
        }
        Ok(result)
    }

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

    /// Wide-field configuration: handles lens distortion, by a SIP correction under the model GRIC
    /// chooses up to an affine map.
    pub fn wide_field() -> Self {
        Self {
            transform_type: TransformModel::Auto,
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
    /// Builds on [`Self::wide_field`] (Homography, SIP, unlimited rotation/scale) and takes its
    /// iteration budget and RMS gate from [`Self::precise`], with extra stars, tighter matching and
    /// stricter confidence of its own.
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
            // From wide_field(): Homography, SIP, unlimited rotation/scale.
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
    /// - invalid warp configuration
    pub fn validate(&self) -> Result<(), InvalidConfigField> {
        self.matching.validate(self.transform_type)?;
        self.ransac.validate()?;
        ConfigValue::new("max_rms_error", self.max_rms_error)
            .finite_and("finite and positive", |value| value > 0.0)?;
        if let Some(sip) = &self.sip {
            sip.validate()?;
            // A homography's perspective terms act to first order as the correction's quadratic
            // ones: the two are not determined together.
            ConfigValue::count(
                "transform_type with SIP",
                TransformType::Homography.parameter_count(),
            )
            .must_be(
                "at most affine, as a homography's perspective terms are the SIP correction's \
                 quadratic ones",
                self.transform_type != TransformModel::Fixed(TransformType::Homography),
            )?;
        }
        self.warp.validate()?;

        Ok(())
    }
}

fn validate_catalog(stars: &[Star], catalog: RegistrationCatalog) -> Result<(), RegistrationError> {
    for (index, star) in stars.iter().enumerate() {
        if !star.pos.x.is_finite() || !star.pos.y.is_finite() {
            return Err(RegistrationError::InvalidStarPosition {
                catalog,
                index,
                position: star.pos,
            });
        }
        let fields = [
            (StarField::Fwhm, f64::from(star.fwhm)),
            (StarField::Flux, f64::from(star.flux)),
            (StarField::Snr, f64::from(star.snr)),
            (StarField::Peak, f64::from(star.peak)),
            (StarField::Sharpness, f64::from(star.sharpness)),
            (StarField::Eccentricity, f64::from(star.eccentricity)),
            (StarField::Ground, f64::from(star.roundness.ground)),
            (StarField::Sround, f64::from(star.roundness.sround)),
            (StarField::PositionSigma, star.position_sigma),
        ];
        if let Some(&(field, value)) = fields.iter().find(|(_, value)| !value.is_finite()) {
            return Err(RegistrationError::InvalidStarField {
                catalog,
                index,
                field,
                value,
            });
        }
    }
    Ok(())
}

/// The centre of the stars' bounding box: the SIP origin when none is configured, shared by every
/// frame registered to the same reference.
pub(super) fn bounding_box_centre(stars: &[Star]) -> DVec2 {
    let (low, high) = stars.iter().fold(
        (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY)),
        |(low, high), star| (low.min(star.pos), high.max(star.pos)),
    );
    (low + high) / 2.0
}

/// The median FWHM over both catalogs.
pub(super) fn median_fwhm(ref_stars: &[Star], target_stars: &[Star]) -> f64 {
    let mut fwhms: Vec<f32> = ref_stars
        .iter()
        .chain(target_stars)
        .map(|s| s.fwhm)
        .collect();
    f64::from(median_mut(&mut fwhms))
}

#[cfg(test)]
mod tests;
