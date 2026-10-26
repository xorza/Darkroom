//! Tests for RANSAC module.

use std::f64::consts::PI;

use glam::DVec2;

use crate::registration::ransac::ransac_config::ScaleRange;
use crate::registration::ransac::*;
use crate::registration::tests::helpers::{apply_all, exact_fit_tolerance, make_grid};

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

mod estimator;
mod math;
mod plausibility;
mod scoring;
