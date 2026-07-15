//! Detection-quality metric curves on forward-model fields.
//!
//! The migrated pipeline tests assert pass/fail thresholds; these assert the *shape* of the
//! detector's response through the `metrics` graders: completeness and reliability are high on a
//! bright field, astrometry is sub-pixel, completeness falls for fainter (lower-SNR) sources, and
//! a source-free noise field yields essentially no false positives, completeness falls under
//! crowding, and the `min_snr` knob gates the faint end.

use super::{Placement, Scenario, detected_positions, synthetic_config, truth_positions};
use crate::stacking::star_detection::detector::StarDetector;
use crate::testing::synthetic::camera::Camera;
use crate::testing::synthetic::metrics::{astrometric_rms, score_detection};
use crate::testing::synthetic::observe::{Observation, render};
use crate::testing::synthetic::scene::{BackgroundField, Scene};

const MATCH_RADIUS: f64 = 4.0;

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
    // Average over several seeds so the bound reflects the threshold, not one lucky noise draw.
    let total: usize = (1u64..=4)
        .map(|seed| {
            let frame = render(
                &scene,
                &Camera::realistic(4.0),
                &Observation::reference(seed),
            );
            StarDetector::from_config(synthetic_config())
                .unwrap()
                .detect(&frame.image)
                .stars
                .len()
        })
        .sum();
    let mean = total as f64 / 4.0;
    // A correct threshold keeps source-free fields essentially empty.
    assert!(
        mean < 0.5,
        "source-free noise fields should average ~0 detections, got mean {mean:.2} over 4 seeds"
    );
}

#[test]
fn completeness_falls_with_crowding() {
    let config = synthetic_config();
    // Same star count and brightness, sparse-uniform vs packed-cluster placement.
    let sparse = Scenario {
        num_stars: 40,
        ..Default::default()
    }
    .frame();
    let crowded = Scenario {
        num_stars: 40,
        placement: Placement::Cluster,
        ..Default::default()
    }
    .frame();
    let sparse_c = score_detection(
        &truth_positions(&sparse),
        &detected_positions(&sparse, &config),
        MATCH_RADIUS,
    )
    .completeness();
    let crowded_c = score_detection(
        &truth_positions(&crowded),
        &detected_positions(&crowded, &config),
        MATCH_RADIUS,
    )
    .completeness();
    println!("sparse completeness {sparse_c:.3}, crowded {crowded_c:.3}");
    assert!(
        sparse_c > 0.8,
        "sparse completeness {sparse_c:.3} should be high"
    );
    assert!(
        crowded_c < sparse_c - 0.1,
        "crowding should lower completeness: crowded {crowded_c:.3} vs sparse {sparse_c:.3}"
    );
}

#[test]
fn min_snr_knob_gates_faint_detections() {
    // A bright→faint field over a shallow (noisy) well, so `min_snr` decides the faint end.
    let frame = Scenario {
        num_stars: 40,
        flux: (1.0, 10.0),
        full_well_e: 2_000.0,
        ..Default::default()
    }
    .frame();
    let mut permissive = synthetic_config();
    permissive.filter.min_snr = 5.0;
    let mut strict = synthetic_config();
    strict.filter.min_snr = 30.0;
    let n_permissive = detected_positions(&frame, &permissive).len();
    let n_strict = detected_positions(&frame, &strict).len();
    assert!(
        n_permissive > n_strict,
        "raising min_snr must reject faint detections: {n_permissive} (snr≥5) vs {n_strict} (snr≥30)"
    );
}
