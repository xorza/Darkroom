//! Tests for RANSAC module.

use crate::stacking::registration::ransac::transforms::{centroid, normalize_points};
use crate::stacking::registration::ransac::*;
use crate::stacking::registration::triangle::voting::PointMatch;
use glam::DVec2;
use std::f64::consts::{PI, SQRT_2};

use rand::rngs::SmallRng;

const TOL: f64 = 1e-6;

fn make_estimator(config: RansacConfig) -> RansacEstimator {
    RansacEstimator::new(config, 1.0)
}

fn estimator_with_max_sigma(max_sigma: f64, config: RansacConfig) -> RansacEstimator {
    RansacEstimator::new(config, max_sigma)
}

fn approx_eq(a: f64, b: f64, eps: f64) -> bool {
    (a - b).abs() < eps
}

/// Create PointMatch objects from paired point arrays with uniform confidence.
fn make_matches(n: usize) -> Vec<PointMatch> {
    (0..n)
        .map(|i| PointMatch {
            ref_idx: i,
            target_idx: i,
            votes: 1,
            confidence: 1.0,
        })
        .collect()
}

/// Create PointMatch objects with custom confidences.
fn make_matches_with_confidence(confidences: &[f64]) -> Vec<PointMatch> {
    confidences
        .iter()
        .enumerate()
        .map(|(i, &c)| PointMatch {
            ref_idx: i,
            target_idx: i,
            votes: 1,
            confidence: c,
        })
        .collect()
}

/// Helper to call estimate with uniform confidences from raw point arrays.
fn estimate_uniform(
    estimator: &RansacEstimator,
    ref_points: &[DVec2],
    target_points: &[DVec2],
    transform_type: TransformType,
) -> Option<RansacResult> {
    let matches = make_matches(ref_points.len());
    estimator.estimate(&matches, ref_points, target_points, transform_type)
}

/// Generate a grid of points for testing.
fn make_grid(cols: usize, rows: usize, spacing: f64) -> Vec<DVec2> {
    let mut points = Vec::with_capacity(cols * rows);
    for r in 0..rows {
        for c in 0..cols {
            points.push(DVec2::new(c as f64 * spacing, r as f64 * spacing));
        }
    }
    points
}

/// Apply a transform to all points.
fn apply_all(transform: &Transform, points: &[DVec2]) -> Vec<DVec2> {
    points.iter().map(|&p| transform.apply(p)).collect()
}

mod estimator;
mod local_optimization;
mod math;
mod plausibility;
mod progressive;
mod scoring;
mod transforms;
