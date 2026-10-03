use crate::internals::synthetic::distortion::{RadialField, RadialPairs};
use crate::registration::distortion::sip::*;
use crate::registration::transform::{Transform, WarpTransform};

/// How far an exact fit may miss: a field in the polynomial's span is recovered but for rounding.
/// Hartley-normalized monomials of a 1000 px grid keep the design's condition number in the
/// thousands, and corrections up to 35 px round at 8e-15 px, so the fit is good to ~1e-11 px; 1e-9
/// holds it.
const EXACT_FIT_PX: f64 = 1e-9;

/// The barrel field every case starts from: `d·1e-7·|d|²` about (500, 500), so 25 px per axis at
/// the corners of the `[0, 1000]²` grid.
fn barrel() -> RadialField {
    RadialField::new(DVec2::new(500.0, 500.0), 1e-7)
}

/// The RMS of `residuals`.
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

/// An order-`order` fit of `field` about its own centre.
fn fit_field(field: &RadialField, order: usize, clip_iterations: usize) -> SipFitResult {
    let RadialPairs { reference, target } = field.pairs();
    let config = SipConfig {
        order,
        reference_point: Some(field.centre),
        clip_iterations,
        ..Default::default()
    };
    fit_sip(&reference, &target, &field.transform, &config)
}

mod basis;
mod correction;
mod fitting;
mod reference;
mod results;
