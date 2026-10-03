//! Tests for RANSAC module.

use crate::internals::prelude::*;
use crate::registration::ransac::*;
use crate::registration::triangle::voting::MatchIndices;
use std::f64::consts::PI;

use rand::rngs::SmallRng;

/// The seed a test that names none runs with, so every run draws the same samples.
const TEST_SEED: u64 = 0x5EED;

/// An estimator for `max_sigma`, seeded with [`TEST_SEED`] unless `config` names its own seed.
fn seeded(max_sigma: f64, mut config: RansacConfig) -> RansacEstimator {
    config.seed.get_or_insert(TEST_SEED);
    RansacEstimator::new(config, max_sigma)
}

/// `PointMatch`es pairing index `i` with index `i`, at the given confidences.
fn matches_with_confidence(confidences: &[f64]) -> Vec<PointMatch> {
    confidences
        .iter()
        .enumerate()
        .map(|(i, &confidence)| PointMatch {
            indices: MatchIndices {
                reference: i,
                target: i,
            },
            confidence,
        })
        .collect()
}

/// [`RansacEstimator::estimate`] with every pair at confidence 1.
fn estimate_uniform(
    estimator: &RansacEstimator,
    ref_points: &[DVec2],
    target_points: &[DVec2],
    transform_type: TransformType,
) -> Result<RansacResult, RansacFailure> {
    let matches = matches_with_confidence(&vec![1.0; ref_points.len()]);
    estimator.estimate(&matches, ref_points, target_points, transform_type)
}

/// A `cols × rows` grid at `spacing`, row by row from the origin.
fn make_grid(cols: usize, rows: usize, spacing: f64) -> Vec<DVec2> {
    let mut points = Vec::with_capacity(cols * rows);
    for r in 0..rows {
        for c in 0..cols {
            points.push(DVec2::new(c as f64 * spacing, r as f64 * spacing));
        }
    }
    points
}

fn apply_all(transform: &Transform, points: &[DVec2]) -> Vec<DVec2> {
    points.iter().map(|&p| transform.apply(p)).collect()
}

/// How far an exact fit of exact points may miss them: the solve is Hartley-normalized, which holds
/// its condition number to the hundreds for the point sets here, against coordinates up to 5000 px
/// whose f64 rounding is 9e-13 px — so ~1e-9 px at worst, and 1e-8 leaves margin. Coordinates of a
/// million pixels scale this with them.
fn exact_fit_tolerance(points: &[DVec2]) -> f64 {
    let largest = points
        .iter()
        .fold(1.0f64, |m, p| m.max(p.abs().max_element()));
    1e-8 * (largest / 5000.0).max(1.0)
}

mod estimator;
mod math;
mod plausibility;
mod sampling;
mod scoring;
mod transforms;
