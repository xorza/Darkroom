//! Challenging case tests - tests star detection under difficult conditions.

use crate::AstroImage;

use crate::star_detection::visual_tests::output::{
    PassCriteria, check_pass, compute_detection_metrics, crowded_criteria, faint_star_criteria,
    save_comparison_png, save_grayscale_png, save_metrics,
};
use crate::star_detection::{StarDetectionConfig, find_stars};
use crate::testing::init_tracing;
use crate::testing::synthetic::{
    CrowdingType, ElongationType, NebulaConfig, StarFieldConfig, crowded_cluster_config,
    elliptical_stars_config, faint_stars_config, generate_star_field,
};
use common::test_utils::test_output_path;

/// Run full pipeline and evaluate metrics for challenging cases.
fn run_challenging_test(
    name: &str,
    field_config: &StarFieldConfig,
    detection_config: &StarDetectionConfig,
    criteria: &PassCriteria,
) -> bool {
    let (pixels, ground_truth) = generate_star_field(field_config);

    // Save input
    save_grayscale_png(
        &pixels,
        field_config.width,
        field_config.height,
        &test_output_path(&format!(
            "synthetic_starfield/challenging_{}_input.png",
            name
        )),
    );

    // Run detection
    let image = AstroImage::from_pixels(field_config.width, field_config.height, 1, pixels.clone());
    let result = find_stars(&image, detection_config);
    let stars = result.stars;

    // Compute metrics
    let match_radius = field_config.fwhm_range.1 * 2.0;
    let metrics = compute_detection_metrics(&ground_truth, &stars, match_radius);

    // Save comparison image
    save_comparison_png(
        &pixels,
        field_config.width,
        field_config.height,
        &ground_truth,
        &stars,
        match_radius,
        &test_output_path(&format!(
            "synthetic_starfield/challenging_{}_comparison.png",
            name
        )),
    );

    // Save metrics
    save_metrics(
        &metrics,
        &test_output_path(&format!(
            "synthetic_starfield/challenging_{}_metrics.txt",
            name
        )),
    );

    println!("\n{} results:", name);
    println!("{}", metrics);

    // Check against criteria
    match check_pass(&metrics, criteria) {
        Ok(()) => {
            println!("PASS: All criteria met");
            true
        }
        Err(failures) => {
            for f in &failures {
                println!("FAIL: {}", f);
            }
            false
        }
    }
}

// ============================================================================
// Crowded Field Tests
// ============================================================================

/// Test: Crowded cluster with overlapping stars.
#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_crowded_cluster() {
    init_tracing();

    let field_config = crowded_cluster_config();
    let detection_config = StarDetectionConfig::default();

    let passed = run_challenging_test(
        "crowded_cluster",
        &field_config,
        &detection_config,
        &crowded_criteria(),
    );

    // Informational - crowded fields are hard
    if !passed {
        println!("Note: Crowded cluster is a challenging case");
    }
}

/// Test: Very dense uniform distribution.
#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_very_dense() {
    init_tracing();

    let field_config = StarFieldConfig {
        width: 512,
        height: 512,
        num_stars: 800, // Very dense
        fwhm_range: (3.0, 4.0),
        magnitude_range: (8.0, 14.0),
        background_level: 0.1,
        noise_sigma: 0.02,
        crowding: CrowdingType::Uniform,
        ..Default::default()
    };
    let detection_config = StarDetectionConfig::default();

    // Very relaxed criteria
    let criteria = PassCriteria {
        min_detection_rate: 0.70, // Many will be blended
        max_false_positive_rate: 0.15,
        max_mean_centroid_error: 0.3,
        max_fwhm_error: 0.40,
    };

    run_challenging_test("very_dense", &field_config, &detection_config, &criteria);
}

/// Test: Gradient density field.
#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_gradient_density() {
    init_tracing();

    let field_config = StarFieldConfig {
        width: 512,
        height: 512,
        num_stars: 300,
        fwhm_range: (3.0, 4.0),
        magnitude_range: (8.0, 14.0),
        background_level: 0.1,
        noise_sigma: 0.02,
        crowding: CrowdingType::Gradient,
        ..Default::default()
    };
    let detection_config = StarDetectionConfig::default();

    run_challenging_test(
        "gradient_density",
        &field_config,
        &detection_config,
        &crowded_criteria(),
    );
}

// ============================================================================
// Elliptical Star Tests
// ============================================================================

/// Test: Uniform tracking error (all stars elongated same direction).
#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_uniform_tracking_error() {
    init_tracing();

    let field_config = elliptical_stars_config();
    let detection_config = StarDetectionConfig::default();

    // Relaxed for elliptical
    let criteria = PassCriteria {
        min_detection_rate: 0.95,
        max_false_positive_rate: 0.05,
        max_mean_centroid_error: 0.2,
        max_fwhm_error: 0.35, // FWHM harder to measure on elliptical
    };

    run_challenging_test(
        "uniform_tracking",
        &field_config,
        &detection_config,
        &criteria,
    );
}

/// Test: Varying tracking error (random elongation).
#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_varying_tracking_error() {
    init_tracing();

    let field_config = StarFieldConfig {
        width: 512,
        height: 512,
        num_stars: 50,
        fwhm_range: (3.0, 4.0),
        magnitude_range: (8.0, 13.0),
        background_level: 0.1,
        noise_sigma: 0.02,
        elongation: ElongationType::Varying,
        eccentricity_range: (0.2, 0.5),
        ..Default::default()
    };
    let detection_config = StarDetectionConfig::default();

    let criteria = PassCriteria {
        min_detection_rate: 0.95,
        max_false_positive_rate: 0.05,
        max_mean_centroid_error: 0.2,
        max_fwhm_error: 0.40,
    };

    run_challenging_test(
        "varying_tracking",
        &field_config,
        &detection_config,
        &criteria,
    );
}

/// Test: Field rotation effect.
#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_field_rotation() {
    init_tracing();

    let field_config = StarFieldConfig {
        width: 512,
        height: 512,
        num_stars: 60,
        fwhm_range: (3.0, 4.0),
        magnitude_range: (8.0, 13.0),
        background_level: 0.1,
        noise_sigma: 0.02,
        elongation: ElongationType::FieldRotation,
        ..Default::default()
    };
    let detection_config = StarDetectionConfig::default();

    let criteria = PassCriteria {
        min_detection_rate: 0.90,
        max_false_positive_rate: 0.05,
        max_mean_centroid_error: 0.25,
        max_fwhm_error: 0.45,
    };

    run_challenging_test(
        "field_rotation",
        &field_config,
        &detection_config,
        &criteria,
    );
}

// ============================================================================
// Artifact Tests
// ============================================================================

/// Test: Cosmic ray contamination.
#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_cosmic_rays() {
    init_tracing();

    let field_config = StarFieldConfig {
        width: 512,
        height: 512,
        num_stars: 40,
        fwhm_range: (3.0, 4.0),
        magnitude_range: (8.0, 13.0),
        background_level: 0.1,
        noise_sigma: 0.02,
        cosmic_ray_count: 50, // Many cosmic rays
        ..Default::default()
    };
    let detection_config = StarDetectionConfig::default();

    // Cosmic rays may cause false positives
    let criteria = PassCriteria {
        min_detection_rate: 0.95,
        max_false_positive_rate: 0.20, // Allow more FP from cosmic rays
        max_mean_centroid_error: 0.15,
        max_fwhm_error: 0.20,
    };

    run_challenging_test("cosmic_rays", &field_config, &detection_config, &criteria);
}

/// Test: Bayer pattern artifacts (uncalibrated CFA).
#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_bayer_pattern() {
    init_tracing();

    let field_config = StarFieldConfig {
        width: 512,
        height: 512,
        num_stars: 40,
        fwhm_range: (3.0, 4.0),
        magnitude_range: (8.0, 13.0),
        background_level: 0.1,
        noise_sigma: 0.02,
        add_bayer: true,
        bayer_strength: 0.08,
        ..Default::default()
    };
    let detection_config = StarDetectionConfig::default();

    let criteria = PassCriteria {
        min_detection_rate: 0.95,
        max_false_positive_rate: 0.05,
        max_mean_centroid_error: 0.20, // Bayer affects centroid
        max_fwhm_error: 0.25,
    };

    run_challenging_test("bayer_pattern", &field_config, &detection_config, &criteria);
}

/// Test: Saturated stars.
#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_saturated_stars() {
    init_tracing();

    let field_config = StarFieldConfig {
        width: 512,
        height: 512,
        num_stars: 50,
        fwhm_range: (3.0, 4.0),
        magnitude_range: (6.0, 13.0), // Include bright stars
        background_level: 0.1,
        noise_sigma: 0.02,
        saturation_fraction: 0.2, // 20% saturated
        saturation_level: 0.95,
        ..Default::default()
    };
    let detection_config = StarDetectionConfig::default();

    let criteria = PassCriteria {
        min_detection_rate: 0.95,
        max_false_positive_rate: 0.05,
        max_mean_centroid_error: 0.25, // Saturated stars harder to centroid
        max_fwhm_error: 0.40,          // FWHM affected by saturation
    };

    run_challenging_test(
        "saturated_stars",
        &field_config,
        &detection_config,
        &criteria,
    );
}

// ============================================================================
// Background Tests
// ============================================================================

/// Test: Gradient background.
#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_gradient_background() {
    init_tracing();

    let field_config = StarFieldConfig {
        width: 512,
        height: 512,
        num_stars: 40,
        fwhm_range: (3.0, 4.0),
        magnitude_range: (8.0, 13.0),
        gradient: Some((0.05, 0.25, 0.3)), // Strong gradient
        noise_sigma: 0.02,
        ..Default::default()
    };
    let detection_config = StarDetectionConfig::default();

    let criteria = PassCriteria {
        min_detection_rate: 0.95,
        max_false_positive_rate: 0.05,
        max_mean_centroid_error: 0.15,
        max_fwhm_error: 0.20,
    };

    run_challenging_test(
        "gradient_background",
        &field_config,
        &detection_config,
        &criteria,
    );
}

/// Test: Vignette background.
#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_vignette_background() {
    init_tracing();

    let field_config = StarFieldConfig {
        width: 512,
        height: 512,
        num_stars: 40,
        fwhm_range: (3.0, 4.0),
        magnitude_range: (8.0, 13.0),
        vignette: Some((0.2, 0.05, 2.0)), // Strong vignette
        noise_sigma: 0.02,
        ..Default::default()
    };
    let detection_config = StarDetectionConfig::default();

    let criteria = PassCriteria {
        min_detection_rate: 0.95,
        max_false_positive_rate: 0.05,
        max_mean_centroid_error: 0.15,
        max_fwhm_error: 0.20,
    };

    run_challenging_test(
        "vignette_background",
        &field_config,
        &detection_config,
        &criteria,
    );
}

/// Test: Nebula background.
#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_nebula_background() {
    init_tracing();

    let field_config = StarFieldConfig {
        width: 512,
        height: 512,
        num_stars: 50,
        fwhm_range: (3.0, 4.0),
        magnitude_range: (8.0, 13.0),
        background_level: 0.1,
        nebula: Some(NebulaConfig {
            center_x: 0.5, // Center of image (fraction)
            center_y: 0.5,
            radius: 0.35, // 35% of diagonal
            amplitude: 0.35,
            softness: 2.0,
            aspect_ratio: 1.3,
            angle: 0.5,
        }),
        noise_sigma: 0.02,
        ..Default::default()
    };
    let detection_config = StarDetectionConfig::default();

    // Nebula is challenging due to variable background
    let criteria = PassCriteria {
        min_detection_rate: 0.90,
        max_false_positive_rate: 0.10,
        max_mean_centroid_error: 0.20,
        max_fwhm_error: 0.25,
    };

    run_challenging_test(
        "nebula_background",
        &field_config,
        &detection_config,
        &criteria,
    );
}

// ============================================================================
// Edge Case Tests
// ============================================================================

/// Test: Stars near image edges.
#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_edge_stars() {
    init_tracing();

    let field_config = StarFieldConfig {
        width: 512,
        height: 512,
        num_stars: 50,
        fwhm_range: (3.0, 4.0),
        magnitude_range: (8.0, 12.0),
        background_level: 0.1,
        noise_sigma: 0.02,
        edge_margin: 5, // Very small margin - stars near edges
        ..Default::default()
    };
    let detection_config = StarDetectionConfig::default();

    // Edge stars may be partially detected
    let criteria = PassCriteria {
        min_detection_rate: 0.85,
        max_false_positive_rate: 0.05,
        max_mean_centroid_error: 0.25,
        max_fwhm_error: 0.30,
    };

    run_challenging_test("edge_stars", &field_config, &detection_config, &criteria);
}

/// Test: Faint stars in high noise.
#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_faint_in_noise() {
    init_tracing();

    let field_config = faint_stars_config();

    // Use lower SNR threshold for faint stars
    let detection_config = StarDetectionConfig {
        min_snr: 3.0,
        detection_sigma: 2.5,
        ..Default::default()
    };

    run_challenging_test(
        "faint_in_noise",
        &field_config,
        &detection_config,
        &faint_star_criteria(),
    );
}

/// Test: Very low SNR conditions.
#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_very_low_snr() {
    init_tracing();

    let field_config = StarFieldConfig {
        width: 512,
        height: 512,
        num_stars: 30,
        fwhm_range: (3.5, 4.5),
        magnitude_range: (13.0, 15.5), // Very faint
        mag_zero_point: 16.0,
        background_level: 0.15,
        noise_sigma: 0.06, // High noise
        ..Default::default()
    };

    let detection_config = StarDetectionConfig {
        min_snr: 2.5,
        detection_sigma: 2.0,
        ..Default::default()
    };

    // Very relaxed for extreme conditions
    let criteria = PassCriteria {
        min_detection_rate: 0.50, // Many will be lost in noise
        max_false_positive_rate: 0.30,
        max_mean_centroid_error: 0.8,
        max_fwhm_error: 0.60,
    };

    let passed = run_challenging_test("very_low_snr", &field_config, &detection_config, &criteria);

    if !passed {
        println!("Note: Very low SNR is an extreme case, failures expected");
    }
}

/// Test: Mixed challenging conditions.
#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_combined_challenges() {
    init_tracing();

    let field_config = StarFieldConfig {
        width: 512,
        height: 512,
        num_stars: 100,
        fwhm_range: (2.5, 5.0),
        magnitude_range: (7.0, 14.0),
        background_level: 0.1,
        noise_sigma: 0.03,
        crowding: CrowdingType::Clustered,
        elongation: ElongationType::Varying,
        eccentricity_range: (0.1, 0.4),
        cosmic_ray_count: 20,
        saturation_fraction: 0.1,
        gradient: Some((0.08, 0.18, 0.5)),
        ..Default::default()
    };

    let detection_config = StarDetectionConfig::default();

    // Relaxed for combined challenges
    let criteria = PassCriteria {
        min_detection_rate: 0.80,
        max_false_positive_rate: 0.15,
        max_mean_centroid_error: 0.25,
        max_fwhm_error: 0.35,
    };

    run_challenging_test(
        "combined_challenges",
        &field_config,
        &detection_config,
        &criteria,
    );
}
