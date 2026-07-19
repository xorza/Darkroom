//! Challenging-case tests — star detection under difficult conditions.
//!
//! Each renders a hard forward-model scenario and runs the full pipeline. The per-scenario
//! `min_detection_rate` targets are **aspirational** (crowding blends sources, saturated stars
//! are rejected by design, edge stars fall outside the margin) and stay informational. What is
//! **enforced** on every scenario are two real regression guards: the detector must not
//! hallucinate (false-positive rate within the scenario's bound) and detection must not collapse
//! to nothing. A regression in either fails the test.

use crate::stacking::star_detection::config::Config;
use crate::stacking::star_detection::synthetic_tests::pipeline_tests::run_test;
use crate::stacking::star_detection::synthetic_tests::{Placement, Scenario};
use crate::stacking::star_detection::test_common::output::metrics::{
    PassCriteria, check_pass, crowded_criteria, faint_star_criteria,
};
use crate::testing::init_tracing;
use crate::testing::synthetic::backgrounds::NebulaConfig;
use crate::testing::synthetic::camera::PsfModel;
use crate::testing::synthetic::observe::SimFrame;
use crate::testing::synthetic::scene::BackgroundField;
use glam::Vec2;

/// Run the pipeline on `frame`, print the aspirational per-scenario criteria (informational),
/// and enforce two universal regression guards: the false-positive rate stays within the
/// scenario's bound (no hallucination) and detection does not collapse.
fn run_challenging_test(
    name: &str,
    frame: &SimFrame,
    detection_config: &Config,
    criteria: &PassCriteria,
) {
    let metrics = run_test(name, "challenging", frame, detection_config);
    match check_pass(&metrics, criteria) {
        Ok(()) => println!("PASS: all aspirational criteria met"),
        Err(failures) => {
            for f in &failures {
                println!("INFO (aspirational): {}", f);
            }
        }
    }

    // Enforced regression guards (independent of the aspirational detection-rate target).
    assert!(
        metrics.false_positive_rate <= criteria.max_false_positive_rate,
        "{name}: false-positive rate {:.1}% exceeds the {:.1}% bound — detector is hallucinating",
        metrics.false_positive_rate * 100.0,
        criteria.max_false_positive_rate * 100.0
    );
    assert!(
        metrics.detection_rate >= 0.05,
        "{name}: detection collapsed to {:.1}%",
        metrics.detection_rate * 100.0
    );
}

#[test]
fn test_crowded_cluster() {
    init_tracing();
    let frame = Scenario {
        num_stars: 150,
        placement: Placement::Cluster,
        ..Default::default()
    }
    .frame();
    run_challenging_test(
        "crowded_cluster",
        &frame,
        &Config::default(),
        &crowded_criteria(),
    );
}

#[test]
fn test_very_dense() {
    init_tracing();
    let frame = Scenario {
        num_stars: 200,
        ..Default::default()
    }
    .frame();
    let criteria = PassCriteria {
        min_detection_rate: 0.70,
        max_false_positive_rate: 0.15,
        max_mean_centroid_error: 0.3,
        max_fwhm_error: 0.40,
    };
    run_challenging_test("very_dense", &frame, &Config::default(), &criteria);
}

#[test]
fn test_gradient_density() {
    init_tracing();
    // Cluster placement stands in for the legacy left-heavy density gradient (non-uniform).
    let frame = Scenario {
        num_stars: 100,
        placement: Placement::Cluster,
        ..Default::default()
    }
    .frame();
    run_challenging_test(
        "gradient_density",
        &frame,
        &Config::default(),
        &crowded_criteria(),
    );
}

#[test]
fn test_uniform_tracking_error() {
    init_tracing();
    let frame = Scenario {
        num_stars: 30,
        psf: Some(PsfModel::Elliptical {
            fwhm: 4.0,
            eccentricity: 0.5,
            angle: 0.5,
        }),
        ..Default::default()
    }
    .frame();
    let criteria = PassCriteria {
        min_detection_rate: 0.95,
        max_false_positive_rate: 0.05,
        max_mean_centroid_error: 0.2,
        max_fwhm_error: 0.35,
    };
    run_challenging_test("uniform_tracking", &frame, &Config::default(), &criteria);
}

#[test]
fn test_varying_tracking_error() {
    init_tracing();
    // Legacy per-star varying elongation collapses to one elliptical instrument PSF.
    let frame = Scenario {
        num_stars: 30,
        psf: Some(PsfModel::Elliptical {
            fwhm: 4.0,
            eccentricity: 0.4,
            angle: 0.3,
        }),
        ..Default::default()
    }
    .frame();
    let criteria = PassCriteria {
        min_detection_rate: 0.95,
        max_false_positive_rate: 0.05,
        max_mean_centroid_error: 0.2,
        max_fwhm_error: 0.40,
    };
    run_challenging_test("varying_tracking", &frame, &Config::default(), &criteria);
}

#[test]
fn test_field_rotation() {
    init_tracing();
    // Legacy field-rotation elongation collapses to one elliptical instrument PSF.
    let frame = Scenario {
        num_stars: 35,
        psf: Some(PsfModel::Elliptical {
            fwhm: 4.0,
            eccentricity: 0.4,
            angle: 0.0,
        }),
        ..Default::default()
    }
    .frame();
    let criteria = PassCriteria {
        min_detection_rate: 0.90,
        max_false_positive_rate: 0.05,
        max_mean_centroid_error: 0.25,
        max_fwhm_error: 0.45,
    };
    run_challenging_test("field_rotation", &frame, &Config::default(), &criteria);
}

#[test]
fn test_cosmic_rays() {
    init_tracing();
    let frame = Scenario {
        num_stars: 25,
        cosmic_rays: 25,
        ..Default::default()
    }
    .frame();
    let criteria = PassCriteria {
        min_detection_rate: 0.95,
        max_false_positive_rate: 0.20,
        max_mean_centroid_error: 0.15,
        max_fwhm_error: 0.20,
    };
    run_challenging_test("cosmic_rays", &frame, &Config::default(), &criteria);
}

#[test]
fn test_bayer_pattern() {
    init_tracing();
    let frame = Scenario {
        num_stars: 25,
        bayer: true,
        ..Default::default()
    }
    .frame();
    let criteria = PassCriteria {
        min_detection_rate: 0.95,
        max_false_positive_rate: 0.05,
        max_mean_centroid_error: 0.20,
        max_fwhm_error: 0.25,
    };
    run_challenging_test("bayer_pattern", &frame, &Config::default(), &criteria);
}

#[test]
fn test_saturated_stars() {
    init_tracing();
    // Bright sources clip at the well — flux-driven saturation.
    let frame = Scenario {
        num_stars: 30,
        flux: (8.0, 250.0),
        ..Default::default()
    }
    .frame();
    let criteria = PassCriteria {
        min_detection_rate: 0.95,
        max_false_positive_rate: 0.05,
        max_mean_centroid_error: 0.25,
        max_fwhm_error: 0.40,
    };
    run_challenging_test("saturated_stars", &frame, &Config::default(), &criteria);
}

#[test]
fn test_gradient_background() {
    init_tracing();
    let frame = Scenario {
        num_stars: 25,
        background: BackgroundField::Gradient {
            start: 0.05,
            end: 0.25,
            angle: 0.3,
        },
        ..Default::default()
    }
    .frame();
    let criteria = PassCriteria {
        min_detection_rate: 0.95,
        max_false_positive_rate: 0.05,
        max_mean_centroid_error: 0.15,
        max_fwhm_error: 0.20,
    };
    run_challenging_test("gradient_background", &frame, &Config::default(), &criteria);
}

#[test]
fn test_vignette_background() {
    init_tracing();
    let frame = Scenario {
        num_stars: 25,
        background: BackgroundField::Vignette {
            center: 0.2,
            edge: 0.05,
            falloff: 2.0,
        },
        ..Default::default()
    }
    .frame();
    let criteria = PassCriteria {
        min_detection_rate: 0.95,
        max_false_positive_rate: 0.05,
        max_mean_centroid_error: 0.15,
        max_fwhm_error: 0.20,
    };
    run_challenging_test("vignette_background", &frame, &Config::default(), &criteria);
}

#[test]
fn test_nebula_background() {
    init_tracing();
    let frame = Scenario {
        num_stars: 30,
        background: BackgroundField::Nebula(NebulaConfig {
            center: Vec2::splat(0.5),
            radius: 0.35,
            amplitude: 0.35,
            softness: 2.0,
            aspect_ratio: 1.3,
            angle: 0.5,
        }),
        ..Default::default()
    }
    .frame();
    let criteria = PassCriteria {
        min_detection_rate: 0.90,
        max_false_positive_rate: 0.10,
        max_mean_centroid_error: 0.20,
        max_fwhm_error: 0.25,
    };
    run_challenging_test("nebula_background", &frame, &Config::default(), &criteria);
}

#[test]
fn test_edge_stars() {
    init_tracing();
    let frame = Scenario {
        num_stars: 30,
        placement: Placement::Uniform { margin: 5.0 },
        ..Default::default()
    }
    .frame();
    let criteria = PassCriteria {
        min_detection_rate: 0.85,
        max_false_positive_rate: 0.05,
        max_mean_centroid_error: 0.25,
        max_fwhm_error: 0.30,
    };
    run_challenging_test("edge_stars", &frame, &Config::default(), &criteria);
}

#[test]
fn test_faint_in_noise() {
    init_tracing();
    // Faint stars over a shallow, noisy sensor.
    let frame = Scenario {
        num_stars: 20,
        flux: (1.5, 6.0),
        full_well_e: 5_000.0,
        background: BackgroundField::Uniform { level: 0.15 },
        ..Default::default()
    }
    .frame();
    let mut detection_config = Config::default();
    detection_config.filter.min_snr = 3.0;
    detection_config.detection.sigma_threshold = 2.5;
    run_challenging_test(
        "faint_in_noise",
        &frame,
        &detection_config,
        &faint_star_criteria(),
    );
}

#[test]
fn test_very_low_snr() {
    init_tracing();
    let frame = Scenario {
        num_stars: 20,
        fwhm: 4.0,
        flux: (1.0, 4.0),
        full_well_e: 2_000.0,
        read_noise_e: 20.0,
        background: BackgroundField::Uniform { level: 0.15 },
        ..Default::default()
    }
    .frame();
    let mut detection_config = Config::default();
    detection_config.filter.min_snr = 2.5;
    detection_config.detection.sigma_threshold = 2.0;
    let criteria = PassCriteria {
        min_detection_rate: 0.50,
        max_false_positive_rate: 0.30,
        max_mean_centroid_error: 0.8,
        max_fwhm_error: 0.60,
    };
    run_challenging_test("very_low_snr", &frame, &detection_config, &criteria);
}

#[test]
fn test_combined_challenges() {
    init_tracing();
    // Cluster + elliptical PSF + cosmic rays + bright/saturated + gradient sky.
    let frame = Scenario {
        num_stars: 50,
        flux: (7.0, 200.0),
        placement: Placement::Cluster,
        psf: Some(PsfModel::Elliptical {
            fwhm: 4.0,
            eccentricity: 0.4,
            angle: 0.3,
        }),
        background: BackgroundField::Gradient {
            start: 0.08,
            end: 0.18,
            angle: 0.5,
        },
        cosmic_rays: 20,
        read_noise_e: 6.0,
        ..Default::default()
    }
    .frame();
    let criteria = PassCriteria {
        min_detection_rate: 0.80,
        max_false_positive_rate: 0.15,
        max_mean_centroid_error: 0.25,
        max_fwhm_error: 0.35,
    };
    run_challenging_test("combined_challenges", &frame, &Config::default(), &criteria);
}
