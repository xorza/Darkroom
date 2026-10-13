//! Sub-pixel shift-recovery accuracy on forward-model frames.
//!
//! Registration and stacking depend on recovering sub-pixel translations between exposures.
//! These render a star field through the realistic [`Camera`], re-render it shifted by a known
//! [`Observation`] translation, and verify the detector's centroids recover that shift to well
//! under 0.1 px — grading per-frame completeness through [`metrics`] against captured truth.

use glam::DVec2;

use crate::internals::synthetic::camera::Camera;
use crate::internals::synthetic::metrics::{match_catalogs, score_detection};
use crate::internals::synthetic::observe::{Observation, render};
use crate::internals::synthetic::scene::{BackgroundField, Placement, Scatter, Scene};
use crate::math::size2us::Size2us;
use crate::registration::transform::Transform;
use crate::star_detection::tests::{
    MATCH_RADIUS, detected_positions, synthetic_config, truth_positions,
};

/// Median per-axis shift between two matched detection catalogs (`shifted − reference`).
/// The median over many stars averages down per-star centroid scatter.
fn median_shift(reference: &[DVec2], shifted: &[DVec2]) -> DVec2 {
    let pairs = match_catalogs(reference, shifted, MATCH_RADIUS);
    assert!(pairs.len() >= 10, "too few matched pairs: {}", pairs.len());
    let mut dx: Vec<f64> = pairs
        .iter()
        .map(|&(r, s)| shifted[s].x - reference[r].x)
        .collect();
    let mut dy: Vec<f64> = pairs
        .iter()
        .map(|&(r, s)| shifted[s].y - reference[r].y)
        .collect();
    dx.sort_by(|a, b| a.partial_cmp(b).unwrap());
    dy.sort_by(|a, b| a.partial_cmp(b).unwrap());
    DVec2::new(dx[dx.len() / 2], dy[dy.len() / 2])
}

#[test]
fn subpixel_shift_recovered_across_offsets() {
    let scene = Scene::scatter(
        Size2us::new(256, 256),
        Scatter {
            count: 24,
            flux: (8.0, 14.0),
            placement: Placement::Uniform { margin: 16.0 },
            seed: 42,
        },
        BackgroundField::Uniform { level: 0.1 },
    );
    let camera = Camera::realistic(4.0);
    let config = synthetic_config();

    let reference = render(&scene, &camera, &Observation::reference(3));
    let ref_det = detected_positions(&reference, &config);
    let comp = score_detection(&truth_positions(&reference), &ref_det, MATCH_RADIUS).completeness();
    assert!(comp >= 0.95, "reference completeness {comp:.3}");

    let shifts = [
        DVec2::new(0.15, 0.23),
        DVec2::new(0.1, 0.0),
        DVec2::new(0.0, 0.1),
        DVec2::new(0.25, 0.25),
        DVec2::new(0.33, 0.17),
        DVec2::new(0.5, 0.5),
        DVec2::new(-0.2, 0.3),
    ];
    for (i, shift) in shifts.into_iter().enumerate() {
        // Each shift renders with its own noise.
        let seed = 100 + i as u64;
        let shifted = Observation {
            transform: Transform::translation(shift),
            ..Observation::reference(seed)
        };
        let shifted_det = detected_positions(&render(&scene, &camera, &shifted), &config);
        let recovered = median_shift(&ref_det, &shifted_det);
        assert!(
            (recovered - shift).length() < 0.1,
            "shift {shift:?} recovered {recovered:?} (error {:.4})",
            (recovered - shift).length()
        );
    }
}
