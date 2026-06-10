//! Detection-quality metric curves on forward-model fields.
//!
//! The migrated pipeline tests assert pass/fail thresholds; these assert the *shape* of the
//! detector's response through the `metrics` graders: completeness and reliability are high on a
//! bright field, astrometry is sub-pixel, completeness falls for fainter (lower-SNR) sources, and
//! a source-free noise field yields essentially no false positives.

use super::Scenario;
use crate::star_detection::config::Config;
use crate::star_detection::detector::StarDetector;
use crate::testing::synthetic::camera::Camera;
use crate::testing::synthetic::metrics::{astrometric_rms, score_detection};
use crate::testing::synthetic::observe::SimFrame;
use crate::testing::synthetic::observe::{Observation, render};
use crate::testing::synthetic::scene::{BackgroundField, Scene};
use glam::DVec2;

const MATCH_RADIUS: f64 = 4.0;

fn synthetic_config() -> Config {
    // Disable the CFA matched filter for synthetic (already-linear) frames.
    Config {
        expected_fwhm: 0.0,
        min_snr: 5.0,
        ..Config::default()
    }
}

fn truth_positions(frame: &SimFrame) -> Vec<DVec2> {
    frame.truth.sources.iter().map(|s| s.pos).collect()
}

fn detected_positions(frame: &SimFrame, config: &Config) -> Vec<DVec2> {
    StarDetector::from_config(config.clone())
        .detect(&frame.image)
        .stars
        .iter()
        .map(|s| s.pos)
        .collect()
}

#[test]
fn completeness_and_reliability_high_for_bright_field() {
    let frame = Scenario {
        num_stars: 20,
        flux: (8.0, 14.0),
        ..Default::default()
    }
    .frame();
    let score = score_detection(
        &truth_positions(&frame),
        &detected_positions(&frame, &synthetic_config()),
        MATCH_RADIUS,
    );
    assert!(
        score.completeness() >= 0.95,
        "completeness {:.3} should be ≥ 0.95 ({}/{} found)",
        score.completeness(),
        score.matched,
        score.n_truth
    );
    assert!(
        score.reliability() >= 0.95,
        "reliability {:.3} should be ≥ 0.95 ({} detections)",
        score.reliability(),
        score.n_recovered
    );
}

#[test]
fn astrometric_rms_is_subpixel_on_bright_stars() {
    let frame = Scenario {
        num_stars: 20,
        ..Default::default()
    }
    .frame();
    let rms = astrometric_rms(
        &truth_positions(&frame),
        &detected_positions(&frame, &synthetic_config()),
        MATCH_RADIUS,
    );
    assert!(
        rms < 0.3,
        "astrometric RMS {rms:.3} px should be sub-pixel (< 0.3)"
    );
}

#[test]
fn completeness_falls_for_fainter_sources() {
    let config = synthetic_config();

    let bright = Scenario {
        num_stars: 20,
        flux: (10.0, 14.0),
        ..Default::default()
    }
    .frame();
    let faint = Scenario {
        num_stars: 20,
        flux: (0.2, 1.0),
        // Shallow well → the faint sources sit near the noise floor.
        full_well_e: 1_500.0,
        ..Default::default()
    }
    .frame();

    let bright_c = score_detection(
        &truth_positions(&bright),
        &detected_positions(&bright, &config),
        MATCH_RADIUS,
    )
    .completeness();
    let faint_c = score_detection(
        &truth_positions(&faint),
        &detected_positions(&faint, &config),
        MATCH_RADIUS,
    )
    .completeness();

    assert!(
        bright_c > 0.9,
        "bright completeness {bright_c:.3} should be high"
    );
    assert!(
        faint_c < bright_c - 0.2,
        "completeness should fall with SNR: faint {faint_c:.3} vs bright {bright_c:.3}"
    );
}

#[test]
fn negligible_false_positives_on_source_free_noise() {
    let scene = Scene {
        width: 256,
        height: 256,
        sources: vec![],
        background: BackgroundField::Uniform { level: 0.1 },
    };
    let frame = render(&scene, &Camera::realistic(4.0), &Observation::reference(1));
    let detections = StarDetector::from_config(synthetic_config())
        .detect(&frame.image)
        .stars
        .len();
    // The detection threshold should keep noise-only fields essentially empty.
    assert!(
        detections <= 3,
        "source-free noise field should yield ~0 detections, got {detections}"
    );
}
