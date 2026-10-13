//! Star detection under difficult conditions: each test renders a hard forward-model field and
//! runs the whole detector, held to what its truth decides exactly (see [`run_test`]).

use glam::Vec2;

use crate::internals::init_tracing;
use crate::internals::synthetic::backgrounds::{NebulaConfig, Vignette};
use crate::internals::synthetic::camera::PsfModel;
use crate::internals::synthetic::scene::BackgroundField;
use crate::star_detection::star_detection_config::StarDetectionConfig;
use crate::star_detection::tests::pipeline_tests::run_test;
use crate::star_detection::tests::{Placement, Scenario};

#[test]
fn crowded_cluster() {
    init_tracing();
    let frame = Scenario {
        num_stars: 150,
        placement: Placement::Cluster,
        ..Default::default()
    }
    .frame();
    run_test(
        "crowded_cluster",
        "challenging",
        &frame,
        &StarDetectionConfig::default(),
        6,
    );
}

#[test]
fn very_dense() {
    init_tracing();
    let frame = Scenario {
        num_stars: 200,
        ..Default::default()
    }
    .frame();
    run_test(
        "very_dense",
        "challenging",
        &frame,
        &StarDetectionConfig::default(),
        1,
    );
}

#[test]
fn uniform_tracking_error() {
    init_tracing();
    let frame = Scenario {
        num_stars: 30,
        psf: Some(PsfModel::Gaussian {
            fwhm: 4.0,
            eccentricity: 0.5,
            angle: 0.5,
        }),
        ..Default::default()
    }
    .frame();
    run_test(
        "uniform_tracking",
        "challenging",
        &frame,
        &StarDetectionConfig::default(),
        11,
    );
}

#[test]
fn cosmic_rays() {
    init_tracing();
    let frame = Scenario {
        num_stars: 25,
        cosmic_rays: 25,
        ..Default::default()
    }
    .frame();
    run_test(
        "cosmic_rays",
        "challenging",
        &frame,
        &StarDetectionConfig::default(),
        9,
    );
}

#[test]
fn bayer_pattern() {
    init_tracing();
    let frame = Scenario {
        num_stars: 25,
        bayer: true,
        ..Default::default()
    }
    .frame();
    run_test(
        "bayer_pattern",
        "challenging",
        &frame,
        &StarDetectionConfig::default(),
        10,
    );
}

#[test]
fn saturated_stars() {
    init_tracing();
    // Bright sources clip at the well — flux-driven saturation.
    let frame = Scenario {
        num_stars: 30,
        flux: (8.0, 250.0),
        ..Default::default()
    }
    .frame();
    run_test(
        "saturated_stars",
        "challenging",
        &frame,
        &StarDetectionConfig::default(),
        8,
    );
}

#[test]
fn gradient_background() {
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
    run_test(
        "gradient_background",
        "challenging",
        &frame,
        &StarDetectionConfig::default(),
        11,
    );
}

#[test]
fn vignette_background() {
    init_tracing();
    let frame = Scenario {
        num_stars: 25,
        background: BackgroundField::Vignette(Vignette {
            center: 0.2,
            edge: 0.05,
            falloff: 2.0,
        }),
        ..Default::default()
    }
    .frame();
    run_test(
        "vignette_background",
        "challenging",
        &frame,
        &StarDetectionConfig::default(),
        0,
    );
}

#[test]
fn nebula_background() {
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
    run_test(
        "nebula_background",
        "challenging",
        &frame,
        &StarDetectionConfig::default(),
        0,
    );
}

#[test]
fn edge_stars() {
    init_tracing();
    let frame = Scenario {
        num_stars: 30,
        placement: Placement::Uniform { margin: 5.0 },
        ..Default::default()
    }
    .frame();
    run_test(
        "edge_stars",
        "challenging",
        &frame,
        &StarDetectionConfig::default(),
        10,
    );
}

#[test]
fn faint_in_noise() {
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
    let mut extraction_config = StarDetectionConfig::default();
    extraction_config.filter.min_snr = 3.0;
    extraction_config.extraction.sigma_threshold = 2.5;
    run_test(
        "faint_in_noise",
        "challenging",
        &frame,
        &extraction_config,
        8,
    );
}

#[test]
fn very_low_snr() {
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
    let mut extraction_config = StarDetectionConfig::default();
    extraction_config.filter.min_snr = 2.5;
    extraction_config.extraction.sigma_threshold = 2.0;
    run_test("very_low_snr", "challenging", &frame, &extraction_config, 1);
}

#[test]
fn combined_challenges() {
    init_tracing();
    // Cluster + elliptical PSF + cosmic rays + bright/saturated + gradient sky.
    let frame = Scenario {
        num_stars: 50,
        flux: (7.0, 200.0),
        placement: Placement::Cluster,
        psf: Some(PsfModel::Gaussian {
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
    run_test(
        "combined_challenges",
        "challenging",
        &frame,
        &StarDetectionConfig::default(),
        4,
    );
}
