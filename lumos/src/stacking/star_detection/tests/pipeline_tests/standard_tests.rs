//! Star detection on typical forward-model fields, held to what their truth decides exactly (see
//! [`run_test`]).

use crate::stacking::star_detection::tests::pipeline_tests::run_test;
use crate::stacking::star_detection::tests::{Scenario, synthetic_config};
use crate::testing::init_tracing;
use crate::testing::synthetic::camera::PsfModel;
use crate::testing::synthetic::scene::BackgroundField;

/// Test: Sparse field with well-separated stars.
#[test]
fn pipeline_sparse_field() {
    init_tracing();

    let frame = Scenario {
        num_stars: 15,
        ..Default::default()
    }
    .frame();

    run_test("sparse_field", "pipeline", &frame, &synthetic_config(), 12);
}

/// Test: Dense field with many stars.
#[test]
fn pipeline_dense_field() {
    init_tracing();

    let frame = Scenario {
        num_stars: 80,
        ..Default::default()
    }
    .frame();

    run_test("dense_field", "pipeline", &frame, &synthetic_config(), 19);
}

/// Test: Moffat profile stars (more realistic PSF).
#[test]
fn pipeline_moffat_profile() {
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

    run_test("moffat_profile", "pipeline", &frame, &synthetic_config(), 8);
}

/// Test: a wider PSF, 4.5 px.
#[test]
fn pipeline_wider_psf() {
    init_tracing();

    let frame = Scenario {
        num_stars: 25,
        fwhm: 4.5,
        ..Default::default()
    }
    .frame();

    run_test("wider_psf", "pipeline", &frame, &synthetic_config(), 13);
}

/// Test: Wide dynamic range (bright to faint stars).
#[test]
fn pipeline_dynamic_range() {
    init_tracing();

    let frame = Scenario {
        num_stars: 30,
        // Faint end near the detection limit.
        flux: (2.5, 22.0),
        ..Default::default()
    }
    .frame();

    run_test("dynamic_range", "pipeline", &frame, &synthetic_config(), 16);
}

/// Test: Low noise (ideal conditions).
#[test]
fn pipeline_low_noise() {
    init_tracing();

    let frame = Scenario {
        num_stars: 25,
        // Deep well + low read noise → very clean.
        full_well_e: 120_000.0,
        read_noise_e: 1.0,
        ..Default::default()
    }
    .frame();

    run_test("low_noise", "pipeline", &frame, &synthetic_config(), 13);
}
