use crate::stacking::registration::distortion::sip::*;
use crate::stacking::registration::transform::Transform;

/// Matched reference and target points.
#[derive(Debug)]
struct PointPairs {
    reference: Vec<DVec2>,
    target: Vec<DVec2>,
}

/// Generate barrel/pincushion distortion point pairs on a grid.
///
/// Distortion model: target = p + (p - center) * k * |p - center|^2
/// - k > 0: barrel distortion (points pushed outward)
/// - k < 0: pincushion distortion (points pulled inward)
fn make_radial_distortion_points(
    center: DVec2,
    k: f64,
    grid_step: usize,
    extent: usize,
) -> PointPairs {
    let mut ref_points = Vec::new();
    let mut target_points = Vec::new();
    for y in (0..=extent).step_by(grid_step) {
        for x in (0..=extent).step_by(grid_step) {
            let p = DVec2::new(x as f64, y as f64);
            let d = p - center;
            let r2 = d.length_squared();
            ref_points.push(p);
            target_points.push(p + d * k * r2);
        }
    }
    PointPairs {
        reference: ref_points,
        target: target_points,
    }
}

/// Compute RMS of a slice of residuals.
fn rms(residuals: &[f64]) -> f64 {
    (residuals.iter().map(|r| r * r).sum::<f64>() / residuals.len() as f64).sqrt()
}

fn fit_sip(
    ref_points: &[DVec2],
    target_points: &[DVec2],
    transform: &Transform,
    config: &SipConfig,
) -> SipFitResult {
    SipPolynomial::fit_from_transform(ref_points, target_points, transform, config).unwrap()
}

mod basis;
mod correction;
mod fitting;
mod reference;
mod results;
mod solvers;
