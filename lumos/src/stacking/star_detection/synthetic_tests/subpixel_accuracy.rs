//! Sub-pixel shift-recovery accuracy on forward-model frames.
//!
//! Registration and stacking depend on recovering sub-pixel translations between exposures.
//! These render a star field through the realistic [`Camera`], re-render it shifted by a known
//! [`Observation`] translation, and verify the detector's centroids recover that shift to well
//! under 0.1 px — grading per-frame completeness through [`metrics`] against captured truth.

use crate::stacking::registration::transform::Transform;
use crate::stacking::star_detection::config::Config;
use crate::stacking::star_detection::synthetic_tests::{
    detected_positions, synthetic_config, truth_positions,
};
use crate::testing::synthetic::camera::Camera;
use crate::testing::synthetic::metrics::{match_catalogs, score_detection};
use crate::testing::synthetic::observe::{Observation, render};
use crate::testing::synthetic::scene::{BackgroundField, Scene};
use glam::DVec2;

const MATCH_RADIUS: f64 = 4.0;

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

/// Render `scene` shifted by `shift` (independent noise via `seed`) and return its detections.
fn shifted_detections(
    scene: &Scene,
    camera: &Camera,
    shift: DVec2,
    seed: u64,
    config: &Config,
) -> Vec<DVec2> {
    let obs = Observation {
        transform: Transform::translation(shift),
        seed,
        ..Observation::reference(seed)
    };
    detected_positions(&render(scene, camera, &obs), config)
}

#[test]
fn subpixel_shift_recovered_to_sub_tenth_pixel() {
    let shift = DVec2::new(0.15, 0.23);
    let scene = Scene::random_field(
        256,
        256,
        24,
        (8.0, 14.0),
        BackgroundField::Uniform { level: 0.1 },
        16.0,
        42,
    );
    let camera = Camera::realistic(4.0);
    let config = synthetic_config();

    let reference = render(&scene, &camera, &Observation::reference(1));
    let ref_det = detected_positions(&reference, &config);
    // The detector recovers essentially every injected star on this bright field.
    let comp = score_detection(&truth_positions(&reference), &ref_det, MATCH_RADIUS).completeness();
    assert!(comp >= 0.95, "reference completeness {comp:.3}");

    let shifted_det = shifted_detections(&scene, &camera, shift, 2, &config);
    let recovered = median_shift(&ref_det, &shifted_det);
    assert!(
        (recovered.x - shift.x).abs() < 0.1,
        "dx {:.4} vs true {:.4}",
        recovered.x,
        shift.x
    );
    assert!(
        (recovered.y - shift.y).abs() < 0.1,
        "dy {:.4} vs true {:.4}",
        recovered.y,
        shift.y
    );
}

#[test]
fn subpixel_shift_recovered_across_offsets() {
    let scene = Scene::random_field(
        256,
        256,
        24,
        (8.0, 14.0),
        BackgroundField::Uniform { level: 0.1 },
        16.0,
        42,
    );
    let camera = Camera::realistic(4.0);
    let config = synthetic_config();

    let reference = render(&scene, &camera, &Observation::reference(3));
    let ref_det = detected_positions(&reference, &config);
    let comp = score_detection(&truth_positions(&reference), &ref_det, MATCH_RADIUS).completeness();
    assert!(comp >= 0.95, "reference completeness {comp:.3}");

    let shifts = [
        DVec2::new(0.1, 0.0),
        DVec2::new(0.0, 0.1),
        DVec2::new(0.25, 0.25),
        DVec2::new(0.33, 0.17),
        DVec2::new(0.5, 0.5),
        DVec2::new(-0.2, 0.3),
    ];
    for (i, shift) in shifts.into_iter().enumerate() {
        let shifted_det = shifted_detections(&scene, &camera, shift, 100 + i as u64, &config);
        let recovered = median_shift(&ref_det, &shifted_det);
        assert!(
            (recovered - shift).length() < 0.1,
            "shift {shift:?} recovered {recovered:?} (error {:.4})",
            (recovered - shift).length()
        );
    }
}
