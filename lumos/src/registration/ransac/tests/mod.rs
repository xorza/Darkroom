//! Tests for RANSAC module.

use std::f64::consts::PI;

use glam::DVec2;

use crate::registration::ransac::ransac_config::ScaleRange;
use crate::registration::ransac::*;

/// An estimator for `max_sigma` under `config`.
fn estimator(max_sigma: f64, config: RansacConfig) -> RansacEstimator {
    RansacEstimator::new(config, max_sigma)
}

/// `reference` and `target` paired by index.
fn pairs(reference: &[DVec2], target: &[DVec2]) -> PointPairs {
    PointPairs {
        reference: reference.to_vec(),
        target: target.to_vec(),
    }
}

/// [`RansacEstimator::estimate`] with every pair at confidence 1.
fn estimate_uniform(
    estimator: &RansacEstimator,
    pairs: &PointPairs,
    transform_type: TransformType,
) -> Result<RansacResult, RansacFailure> {
    estimator.estimate(pairs, &vec![1.0; pairs.reference.len()], transform_type)
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
mod scoring;
