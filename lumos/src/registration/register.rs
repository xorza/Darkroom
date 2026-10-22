//! [`PreparedPair`]: one frame pair's catalogs checked and prepared for registration, and their
//! registration.

use std::time::Instant;

use glam::DVec2;

use crate::math::statistics;
use crate::registration::error::{RegistrationCatalog, RegistrationError, StarField};
use crate::registration::final_fit::fit_catalogs::FitCatalogs;
use crate::registration::kd_tree::KdTree;
use crate::registration::model_search::ModelSearch;
use crate::registration::point_pairs::PointPairs;
use crate::registration::ransac::RansacEstimator;
use crate::registration::registration_config::RegistrationConfig;
use crate::registration::registration_result::RegistrationResult;
use crate::registration::triangle::voting::PointMatch;
use crate::registration::tuning;
use crate::star_detection::star::Star;

/// Two catalogs checked against a configuration and prepared for its stages: the noise scale
/// RANSAC scores at, the catalogs the final fit reads, each catalog's brightest stars as a tree for
/// triangle matching, and the origin a SIP correction is taken about when the configuration names
/// none.
#[derive(Debug)]
pub(super) struct PreparedPair<'a> {
    config: &'a RegistrationConfig,
    max_sigma: f64,
    catalogs: FitCatalogs,
    reference: KdTree,
    target: KdTree,
    origin: DVec2,
}

impl<'a> PreparedPair<'a> {
    /// `ref_stars` and `target_stars` prepared under `config`, which its caller validated.
    ///
    /// # Errors
    ///
    /// A star registration cannot read, or a catalog below the star gates.
    pub(super) fn new(
        config: &'a RegistrationConfig,
        ref_stars: &[Star],
        target_stars: &[Star],
    ) -> Result<Self, RegistrationError> {
        validate_catalog(ref_stars, RegistrationCatalog::Reference)?;
        validate_catalog(target_stars, RegistrationCatalog::Target)?;

        // The gate is keyed to the transform model unless `min_stars` overrides it.
        let required_stars = config.matching.required_stars(config.model);
        for stars in [ref_stars, target_stars] {
            if stars.len() < required_stars {
                return Err(RegistrationError::TooFewStars {
                    found: stars.len(),
                    required: required_stars,
                });
            }
        }

        // The final fit reads no saturated star, so each catalog needs as many unsaturated ones
        // as the fit needs pairs.
        for (stars, catalog) in [
            (ref_stars, RegistrationCatalog::Reference),
            (target_stars, RegistrationCatalog::Target),
        ] {
            let usable = stars.iter().filter(|star| !star.saturated).count();
            if usable < config.matching.min_matches {
                return Err(RegistrationError::TooFewUnsaturatedStars {
                    catalog,
                    found: usable,
                    required: config.matching.min_matches,
                });
            }
        }

        // The brightest `max_stars` of each set, as trees: triangle matching forms its triangles
        // over both, and RANSAC samples their matches for every model.
        let brightest = |stars: &[Star]| {
            KdTree::build(
                stars
                    .iter()
                    .take(config.matching.max_stars)
                    .map(|s| s.pos)
                    .collect(),
            )
            .expect("the star gates leave at least three stars in each set")
        };
        Ok(Self {
            config,
            max_sigma: tuning::max_sigma_from_fwhm(median_fwhm(ref_stars, target_stars)),
            catalogs: FitCatalogs::new(ref_stars, target_stars),
            reference: brightest(ref_stars),
            target: brightest(target_stars),
            origin: bounding_box_centre(ref_stars),
        })
    }

    /// The triangle matches between the brightest stars of each catalog.
    pub(super) fn matches(&self) -> Vec<PointMatch> {
        self.config
            .matching
            .triangle
            .match_trees(&self.reference, &self.target, self.max_sigma)
    }

    /// The model search over `matches`: RANSAC on their positions, and the final fit over the
    /// catalogs.
    pub(super) fn search(&self, matches: &[PointMatch]) -> ModelSearch<'_> {
        ModelSearch {
            pairs: PointPairs::from_matches(matches, self.reference.points(), self.target.points()),
            confidences: matches.iter().map(|m| m.confidence).collect(),
            catalogs: &self.catalogs,
            ransac: RansacEstimator::new(self.config.ransac, self.max_sigma),
            recovery_radius: tuning::recovery_radius(self.max_sigma),
            min_matches: self.config.matching.min_matches,
            max_rms_error: self.config.max_rms_error,
        }
    }

    /// The pair registered: matched, fitted by the configured model, and held to the RMS gate.
    ///
    /// # Errors
    ///
    /// Too few matches, every model's failure, or a fit past the RMS gate.
    pub(super) fn register(&self) -> Result<RegistrationResult, RegistrationError> {
        let start = Instant::now();
        let matches = self.matches();
        tracing::debug!(
            triangle_ms = start.elapsed().as_secs_f64() * 1000.0,
            num_matches = matches.len(),
            "Triangle matching complete"
        );
        if matches.len() < self.config.matching.min_matches {
            return Err(RegistrationError::NoMatchingPatterns);
        }
        let fit = self.search(&matches).fit(self.config.model, self.origin)?;
        tracing::debug!(
            register_ms = start.elapsed().as_secs_f64() * 1000.0,
            "Registration complete"
        );
        if fit.rms_error > self.config.max_rms_error {
            return Err(RegistrationError::AccuracyTooLow {
                rms_error: fit.rms_error,
                max_allowed: self.config.max_rms_error,
            });
        }
        Ok(RegistrationResult::new(
            fit.warp,
            fit.matches,
            fit.rms_error,
        ))
    }
}

/// Every star of `catalog` carries what registration reads in a form it can use: a finite position
/// and FWHM, and a finite, positive position σ. The other measurements are not read.
fn validate_catalog(stars: &[Star], catalog: RegistrationCatalog) -> Result<(), RegistrationError> {
    for (idx, star) in stars.iter().enumerate() {
        if !star.pos.x.is_finite() || !star.pos.y.is_finite() {
            return Err(RegistrationError::InvalidStarPos {
                catalog,
                idx,
                pos: star.pos,
            });
        }
        let fields = [
            (StarField::Fwhm, f64::from(star.fwhm), star.fwhm.is_finite()),
            (
                StarField::PosSigma,
                star.pos_sigma,
                star.pos_sigma.is_finite() && star.pos_sigma > 0.0,
            ),
        ];
        if let Some(&(field, value, _)) = fields.iter().find(|(_, _, valid)| !valid) {
            return Err(RegistrationError::InvalidStarField {
                catalog,
                idx,
                field,
                value,
            });
        }
    }
    Ok(())
}

/// The centre of the stars' bounding box: the SIP origin when none is configured, shared by every
/// frame registered to the same reference.
fn bounding_box_centre(stars: &[Star]) -> DVec2 {
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
    f64::from(statistics::median_mut(&mut fwhms))
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use glam::DVec2;

    use crate::registration::register::PreparedPair;

    impl PreparedPair<'_> {
        /// The noise scale RANSAC scores at, from the stars' median FWHM.
        pub(crate) const fn max_sigma(&self) -> f64 {
            self.max_sigma
        }

        /// The origin a SIP correction is taken about when the configuration names none.
        pub(crate) const fn origin(&self) -> DVec2 {
            self.origin
        }
    }
}
