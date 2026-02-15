//! Shared test helpers for synthetic registration tests.

use crate::star_detection::Star;
use glam::DVec2;

/// FWHM for tight/compact stars (~max_sigma 0.67).
pub const FWHM_TIGHT: f32 = 1.34;
/// FWHM for normal/typical stars (~max_sigma 1.0).
pub const FWHM_NORMAL: f32 = 2.0;

/// Apply an affine transform to star positions.
/// Affine: [a, b, tx, c, d, ty] where the transform is:
/// x' = a*x + b*y + tx
/// y' = c*x + d*y + ty
pub fn apply_affine(stars: &[Star], params: [f64; 6]) -> Vec<Star> {
    let [a, b, tx, c, d, ty] = params;
    stars
        .iter()
        .map(|s| Star {
            pos: DVec2::new(
                a * s.pos.x + b * s.pos.y + tx,
                c * s.pos.x + d * s.pos.y + ty,
            ),
            ..*s
        })
        .collect()
}

/// Apply a homography (projective transform) to star positions.
/// H = [h0, h1, h2, h3, h4, h5, h6, h7, 1.0]
/// x' = (h0*x + h1*y + h2) / (h6*x + h7*y + 1)
/// y' = (h3*x + h4*y + h5) / (h6*x + h7*y + 1)
pub fn apply_homography(stars: &[Star], params: [f64; 8]) -> Vec<Star> {
    stars
        .iter()
        .map(|s| {
            let w = params[6] * s.pos.x + params[7] * s.pos.y + 1.0;
            let x_prime = (params[0] * s.pos.x + params[1] * s.pos.y + params[2]) / w;
            let y_prime = (params[3] * s.pos.x + params[4] * s.pos.y + params[5]) / w;
            Star {
                pos: DVec2::new(x_prime, y_prime),
                ..*s
            }
        })
        .collect()
}
