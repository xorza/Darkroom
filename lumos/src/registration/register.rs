//! [`PreparedPair`]: a target's catalog checked and prepared beside the reference's, and the pair's
//! registration.

use std::time::Instant;

use crate::math::statistics;
use crate::registration::error::{RegistrationCatalog, RegistrationError, StarField};
use crate::registration::final_fit::fit_catalogs::FitCatalogs;
use crate::registration::kd_tree::KdTree;
use crate::registration::model_search::ModelSearch;
use crate::registration::point_pairs::PointPairs;
use crate::registration::ransac::RansacEstimator;
use crate::registration::registrar::ReferenceCatalog;
use crate::registration::registration_config::RegistrationConfig;
use crate::registration::registration_result::RegistrationResult;
use crate::registration::triangle::voting::PointMatch;
use crate::registration::tuning;
use crate::star_detection::star::Star;

/// A target checked against the reference's configuration and prepared beside the reference for
/// the stages: the noise scale RANSAC scores at, the catalogs the final fit reads, and the
/// target's brightest stars as a tree for triangle matching.
#[derive(Debug)]
pub(super) struct PreparedPair<'a> {
    reference: &'a ReferenceCatalog<'a>,
    max_sigma: f64,
    catalogs: FitCatalogs,
    target: KdTree,
}

impl<'a> PreparedPair<'a> {
    /// `target` prepared beside `reference`.
    ///
    /// # Errors
    ///
    /// A star registration cannot read, or a catalog below the star gates.
    pub(super) fn new(
        reference: &'a ReferenceCatalog<'a>,
        target: &[Star],
    ) -> Result<Self, RegistrationError> {
        let target_tree = check_catalog(reference.config(), target, RegistrationCatalog::Target)?;
        Ok(Self {
            reference,
            max_sigma: tuning::max_sigma_from_fwhm(median_fwhm(reference.stars(), target)),
            catalogs: FitCatalogs::new(reference.stars(), target),
            target: target_tree,
        })
    }

    /// The triangle matches between the brightest stars of each catalog.
    pub(super) fn matches(&self) -> Vec<PointMatch> {
        self.reference.config().matching.triangle.match_trees(
            self.reference.brightest(),
            &self.target,
            self.max_sigma,
        )
    }

    /// The model search over `matches`: RANSAC on their positions, and the final fit over the
    /// catalogs.
    pub(super) fn search(&self, matches: &[PointMatch]) -> ModelSearch<'_> {
        let config = self.reference.config();
        ModelSearch {
            pairs: PointPairs::from_matches(
                matches,
                self.reference.brightest().points(),
                self.target.points(),
            ),
            confidences: matches.iter().map(|m| m.confidence).collect(),
            catalogs: &self.catalogs,
            ransac: RansacEstimator::new(config.ransac, self.max_sigma),
            recovery_radius: tuning::recovery_radius(self.max_sigma),
            min_matches: config.matching.min_matches,
            max_rms_error_px: config.max_rms_error_px,
        }
    }

    /// The pair registered: matched, fitted by the configured model, and held to the RMS gate.
    ///
    /// # Errors
    ///
    /// Too few matches, every model's failure, or a fit past the RMS gate.
    pub(super) fn register(&self) -> Result<RegistrationResult, RegistrationError> {
        let config = self.reference.config();
        let start = Instant::now();
        let matches = self.matches();
        tracing::debug!(
            triangle_ms = start.elapsed().as_secs_f64() * 1000.0,
            match_count = matches.len(),
            "Triangle matching complete"
        );
        if matches.len() < config.matching.min_matches {
            return Err(RegistrationError::NoMatchingPatterns {
                found: matches.len(),
                required: config.matching.min_matches,
            });
        }
        let fit = self
            .search(&matches)
            .fit(config.model, self.reference.origin())?;
        tracing::debug!(
            register_ms = start.elapsed().as_secs_f64() * 1000.0,
            "Registration complete"
        );
        let rms_error_px = fit.result.rms_error_px();
        if rms_error_px > config.max_rms_error_px {
            return Err(RegistrationError::AccuracyTooLow {
                rms_error: rms_error_px,
                max_allowed: config.max_rms_error_px,
            });
        }
        Ok(fit.result)
    }
}

/// `stars` checked as `catalog` under `config` — every star readable, and as many stars and
/// unsaturated stars as the gates ask — and its brightest `max_stars` as the tree triangle matching
/// forms its triangles over, and RANSAC samples their matches from.
pub(super) fn check_catalog(
    config: &RegistrationConfig,
    stars: &[Star],
    catalog: RegistrationCatalog,
) -> Result<KdTree, RegistrationError> {
    validate_catalog(stars, catalog)?;
    let required = config.model.required_stars();
    if stars.len() < required {
        return Err(RegistrationError::TooFewStars {
            catalog,
            found: stars.len(),
            required,
        });
    }
    // The final fit reads no saturated star, so each catalog needs as many unsaturated ones as the
    // fit needs pairs.
    let usable = stars.iter().filter(|star| !star.saturated).count();
    if usable < config.matching.min_matches {
        return Err(RegistrationError::TooFewUnsaturatedStars {
            catalog,
            found: usable,
            required: config.matching.min_matches,
        });
    }
    Ok(KdTree::build(
        brightest_idxs(stars, config.matching.max_stars)
            .into_iter()
            .map(|idx| stars[idx].pos)
            .collect(),
    )
    .expect("the star gates leave at least three stars in each set"))
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

/// The indices of the `count` brightest of `stars`, by flux, ties to the earlier, in their input
/// order: a selection in `O(n)`, so a catalog in any order matches by its brightest stars, and one
/// sorted brightest first keeps its order.
fn brightest_idxs(stars: &[Star], count: usize) -> Vec<usize> {
    let mut idxs: Vec<usize> = (0..stars.len()).collect();
    if count < stars.len() {
        idxs.select_nth_unstable_by(count, |&a, &b| {
            stars[b].flux.total_cmp(&stars[a].flux).then(a.cmp(&b))
        });
        idxs.truncate(count);
        idxs.sort_unstable();
    }
    idxs
}

/// The median FWHM over both catalogs' unsaturated stars, whose width is the seeing's; over every
/// star where all are saturated.
pub(super) fn median_fwhm(ref_stars: &[Star], target_stars: &[Star]) -> f64 {
    let stars = || ref_stars.iter().chain(target_stars);
    let mut fwhms: Vec<f32> = stars().filter(|s| !s.saturated).map(|s| s.fwhm).collect();
    if fwhms.is_empty() {
        fwhms.extend(stars().map(|s| s.fwhm));
    }
    f64::from(statistics::median_mut(&mut fwhms))
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use crate::registration::register::PreparedPair;

    impl PreparedPair<'_> {
        /// The noise scale RANSAC scores at, from the stars' median FWHM.
        pub(crate) const fn max_sigma(&self) -> f64 {
            self.max_sigma
        }
    }
}

#[cfg(test)]
mod tests {
    use glam::DVec2;

    use crate::registration::register::brightest_idxs;
    use crate::star_detection::star::Star;

    /// The brightest stars by flux, in input order whatever order the catalog came in: of fluxes
    /// 1, 5, 3, 5, 2, the two brightest are the 5s at 1 and 3, the three brightest add the 3 at 2,
    /// and a count past the catalog takes it whole.
    #[test]
    fn the_brightest_stars_are_selected_in_input_order() {
        let stars: Vec<Star> = [1.0, 5.0, 3.0, 5.0, 2.0]
            .into_iter()
            .map(|flux| Star::at(DVec2::ZERO).with_flux(flux))
            .collect();
        assert_eq!(brightest_idxs(&stars, 2), [1, 3]);
        assert_eq!(brightest_idxs(&stars, 3), [1, 2, 3]);
        assert_eq!(brightest_idxs(&stars, 9), [0, 1, 2, 3, 4]);
    }
}
