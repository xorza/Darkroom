//! Shared test helpers for synthetic registration tests.

use crate::internals::prelude::*;
use crate::registration::config::RegistrationMatchingConfig;
use crate::registration::transform::Transform;
use crate::registration::{self, Config, RegistrationError, RegistrationResult};
use crate::star_detection::star::Star;

/// FWHM for tight stars: `max_sigma_from_fwhm` gives `σ_max` = 0.67 px, above its 0.5 px floor.
pub(super) const FWHM_TIGHT: f32 = 1.34;
/// FWHM for typical stars: `σ_max` = 1.0 px, a recovery radius of `√χ²₀.₉₉(2)` = 3.03 px.
pub(super) const FWHM_NORMAL: f32 = 2.0;

/// The RANSAC seed a test that names none runs with, so a registration result is repeatable.
const TEST_SEED: u64 = 0x5EED;

/// [`registration::register`] with a seeded RANSAC: a config that names no seed gets
/// `TEST_SEED`, one that does keeps its own.
pub(super) fn register(
    ref_stars: &[Star],
    target_stars: &[Star],
    config: &Config,
) -> Result<RegistrationResult, RegistrationError> {
    let mut config = config.clone();
    config.ransac.seed.get_or_insert(TEST_SEED);
    registration::register(ref_stars, target_stars, &config)
}

pub(super) fn matching_config(min_stars: usize, min_matches: usize) -> RegistrationMatchingConfig {
    RegistrationMatchingConfig {
        min_stars: Some(min_stars),
        min_matches,
        ..Default::default()
    }
}

/// `stars` moved by `transform`, every other field kept.
pub(super) fn map_stars(stars: &[Star], transform: &Transform) -> Vec<Star> {
    stars
        .iter()
        .map(|star| star.with_pos(transform.apply(star.pos)))
        .collect()
}

/// The largest distance between where `found` and `truth` put a point of the box `[low, high]`,
/// over a 9×9 grid spanning it: a fit's error where it is used, corners included, rather than in
/// parameters whose units differ.
pub(super) fn max_deviation(found: &Transform, truth: &Transform, low: DVec2, high: DVec2) -> f64 {
    (0..9)
        .flat_map(|j| (0..9).map(move |i| DVec2::new(f64::from(i), f64::from(j)) / 8.0))
        .map(|t| low + (high - low) * t)
        .map(|p| found.apply(p).distance(truth.apply(p)))
        .fold(0.0, f64::max)
}
