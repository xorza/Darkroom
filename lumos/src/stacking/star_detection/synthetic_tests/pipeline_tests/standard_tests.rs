//! Standard pipeline tests - full star detection on typical forward-model scenarios.

use super::run_test;
use crate::stacking::star_detection::config::Config;
use crate::stacking::star_detection::synthetic_tests::Scenario;
use crate::stacking::star_detection::test_common::output::metrics::{
    PassCriteria, check_pass, standard_criteria,
};
use crate::testing::init_tracing;
use crate::testing::synthetic::camera::PsfModel;
use crate::testing::synthetic::scene::BackgroundField;

/// Detection config for synthetic images: disable the CFA matched filter so FWHM stays accurate.
fn detection_config() -> Config {
    let mut config = Config::default();
    config.fwhm.expected = 0.0;
    config
}

/// Test: Sparse field with well-separated stars.
#[test]
fn test_pipeline_sparse_field() {
    init_tracing();

    let frame = Scenario {
        num_stars: 15,
        ..Default::default()
    }
    .frame();

    let metrics = run_test("sparse_field", "pipeline", &frame, &detection_config());

    if let Err(failures) = check_pass(&metrics, &standard_criteria()) {
        panic!("Sparse field test failed: {:?}", failures);
    }
}

/// Test: Dense field with many stars.
#[test]
fn test_pipeline_dense_field() {
    init_tracing();

    let frame = Scenario {
        num_stars: 80,
        ..Default::default()
    }
    .frame();

    let metrics = run_test("dense_field", "pipeline", &frame, &detection_config());

    // Crowding blends some neighbours, so the floor is below a sparse field's — but it is a
    // real, enforced regression guard (observed ~81% completeness, 0 false positives).
    let criteria = PassCriteria {
        min_detection_rate: 0.75,
        max_false_positive_rate: 0.05,
        max_mean_centroid_error: 0.20,
        max_fwhm_error: 0.10,
    };
    if let Err(failures) = check_pass(&metrics, &criteria) {
        panic!("Dense field regressed: {failures:?}");
    }
}

/// Test: Moffat profile stars (more realistic PSF).
#[test]
fn test_pipeline_moffat_profile() {
    init_tracing();

    let frame = Scenario {
        // Moffat's broad 8α wings merge close neighbours and elevate the local background, so
        // use fewer, well-separated stars at moderate (unsaturated) brightness — the scenario
        // still exercises detection on a realistic atmospheric PSF.
        num_stars: 15,
        psf: Some(PsfModel::Moffat {
            fwhm: 4.0,
            beta: 2.5,
        }),
        flux: (5.0, 11.0),
        background: BackgroundField::Uniform { level: 0.05 },
        ..Default::default()
    }
    .frame();

    let metrics = run_test("moffat_profile", "pipeline", &frame, &detection_config());

    // Moffat wings: Gaussian-fit FWHM and centroid matching carry some extra error.
    let criteria = PassCriteria {
        min_detection_rate: 0.85,
        max_false_positive_rate: 0.05,
        max_mean_centroid_error: 0.30,
        max_fwhm_error: 0.20,
    };
    if let Err(failures) = check_pass(&metrics, &criteria) {
        panic!("Moffat profile test failed: {:?}", failures);
    }
}

/// Test: Larger PSF (single instrument FWHM; legacy varied per-star).
#[test]
fn test_pipeline_fwhm_range() {
    init_tracing();

    let frame = Scenario {
        num_stars: 25,
        fwhm: 4.5,
        ..Default::default()
    }
    .frame();

    let metrics = run_test("fwhm_range", "pipeline", &frame, &detection_config());

    let criteria = PassCriteria {
        min_detection_rate: 0.90,
        max_false_positive_rate: 0.05,
        max_mean_centroid_error: 0.30,
        max_fwhm_error: 0.30,
    };
    if let Err(failures) = check_pass(&metrics, &criteria) {
        panic!("FWHM range test failed: {:?}", failures);
    }
}

/// Test: Wide dynamic range (bright to faint stars).
#[test]
fn test_pipeline_dynamic_range() {
    init_tracing();

    let frame = Scenario {
        num_stars: 30,
        // Faint end near the detection limit.
        flux: (2.5, 22.0),
        ..Default::default()
    }
    .frame();

    let mut detection_config = detection_config();
    detection_config.filter.min_snr = 5.0;
    let metrics = run_test("dynamic_range", "pipeline", &frame, &detection_config);

    // The faint end sits near the detection limit, so completeness is lower — but enforced as a
    // real floor (observed ~77% completeness, 0 false positives, sub-0.35 px centroids).
    let criteria = PassCriteria {
        min_detection_rate: 0.70,
        max_false_positive_rate: 0.05,
        max_mean_centroid_error: 0.35,
        max_fwhm_error: 0.10,
    };
    if let Err(failures) = check_pass(&metrics, &criteria) {
        panic!("Dynamic range regressed: {failures:?}");
    }
}

/// Test: Low noise (ideal conditions).
#[test]
fn test_pipeline_low_noise() {
    init_tracing();

    let frame = Scenario {
        num_stars: 25,
        // Deep well + low read noise → very clean.
        full_well_e: 120_000.0,
        read_noise_e: 1.0,
        ..Default::default()
    }
    .frame();

    let metrics = run_test("low_noise", "pipeline", &frame, &detection_config());

    let criteria = PassCriteria {
        min_detection_rate: 0.92,
        max_false_positive_rate: 0.02,
        max_mean_centroid_error: 0.25,
        max_fwhm_error: 0.10,
    };
    if let Err(failures) = check_pass(&metrics, &criteria) {
        panic!("Low noise test failed: {:?}", failures);
    }
}
