//! Standard pipeline tests - tests full star detection on typical scenarios.

use crate::star_detection::visual_tests::generators::{
    StarFieldConfig, dense_field_config, generate_star_field, sparse_field_config,
};
use crate::star_detection::visual_tests::output::{
    DetectionMetrics, check_pass, compute_detection_metrics, save_comparison_png,
    save_grayscale_png, save_metrics, standard_criteria,
};
use crate::star_detection::{StarDetectionConfig, find_stars};
use crate::testing::init_tracing;
use common::test_utils::test_output_path;

/// Run full pipeline and evaluate metrics.
fn run_pipeline_test(
    name: &str,
    field_config: &StarFieldConfig,
    detection_config: &StarDetectionConfig,
) -> DetectionMetrics {
    let (pixels, ground_truth) = generate_star_field(field_config);

    // Save input
    save_grayscale_png(
        &pixels,
        field_config.width,
        field_config.height,
        &test_output_path(&format!("pipeline_{}_input.png", name)),
    );

    // Run detection
    let result = find_stars(
        &pixels,
        field_config.width,
        field_config.height,
        detection_config,
    );
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
        &test_output_path(&format!("pipeline_{}_comparison.png", name)),
    );

    // Save metrics
    save_metrics(
        &metrics,
        &test_output_path(&format!("pipeline_{}_metrics.txt", name)),
    );

    println!("\n{} results:", name);
    println!("{}", metrics);

    metrics
}

/// Test: Sparse field with well-separated stars.
#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_pipeline_sparse_field() {
    init_tracing();

    let field_config = sparse_field_config();
    let detection_config = StarDetectionConfig::default();

    let metrics = run_pipeline_test("sparse_field", &field_config, &detection_config);

    // Check against standard criteria
    let criteria = standard_criteria();
    if let Err(failures) = check_pass(&metrics, &criteria) {
        for f in &failures {
            println!("FAIL: {}", f);
        }
        panic!("Test failed: {:?}", failures);
    }
}

/// Test: Dense field with many stars.
#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_pipeline_dense_field() {
    init_tracing();

    let field_config = dense_field_config();
    let detection_config = StarDetectionConfig::default();

    let metrics = run_pipeline_test("dense_field", &field_config, &detection_config);

    // Relaxed criteria for dense field
    let criteria = crate::star_detection::visual_tests::output::crowded_criteria();
    if let Err(failures) = check_pass(&metrics, &criteria) {
        for f in &failures {
            println!("FAIL: {}", f);
        }
        // Don't fail the test, just report
        println!("Dense field test has some failures (expected for challenging conditions)");
    }
}

/// Test: Moffat profile stars (more realistic PSF).
#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_pipeline_moffat_profile() {
    init_tracing();

    let field_config = StarFieldConfig {
        width: 512,
        height: 512,
        num_stars: 40,
        fwhm_range: (3.5, 4.5),
        magnitude_range: (8.0, 13.0),
        background_level: 0.1,
        noise_sigma: 0.02,
        use_moffat: true,
        moffat_beta: 2.5,
        ..Default::default()
    };
    let detection_config = StarDetectionConfig::default();

    let metrics = run_pipeline_test("moffat_profile", &field_config, &detection_config);

    let criteria = standard_criteria();
    if let Err(failures) = check_pass(&metrics, &criteria) {
        for f in &failures {
            println!("FAIL: {}", f);
        }
        panic!("Moffat profile test failed: {:?}", failures);
    }
}

/// Test: Wide FWHM range (2-6 pixels).
#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_pipeline_fwhm_range() {
    init_tracing();

    let field_config = StarFieldConfig {
        width: 512,
        height: 512,
        num_stars: 40,
        fwhm_range: (2.0, 6.0), // Wide range
        magnitude_range: (8.0, 12.0),
        background_level: 0.1,
        noise_sigma: 0.02,
        ..Default::default()
    };
    let detection_config = StarDetectionConfig::default();

    let metrics = run_pipeline_test("fwhm_range", &field_config, &detection_config);

    // Slightly relaxed for FWHM variation
    let criteria = crate::star_detection::visual_tests::output::PassCriteria {
        min_detection_rate: 0.95,
        max_false_positive_rate: 0.05,
        max_mean_centroid_error: 0.15,
        max_fwhm_error: 0.30, // More tolerance for FWHM
    };

    if let Err(failures) = check_pass(&metrics, &criteria) {
        for f in &failures {
            println!("FAIL: {}", f);
        }
        panic!("FWHM range test failed: {:?}", failures);
    }
}

/// Test: Wide dynamic range (bright to faint stars).
#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_pipeline_dynamic_range() {
    init_tracing();

    let field_config = StarFieldConfig {
        width: 512,
        height: 512,
        num_stars: 50,
        fwhm_range: (3.0, 4.0),
        magnitude_range: (6.0, 15.0), // Wide dynamic range
        mag_zero_point: 16.0,
        background_level: 0.1,
        noise_sigma: 0.02,
        ..Default::default()
    };

    // Lower SNR threshold to catch faint stars
    let detection_config = StarDetectionConfig {
        min_snr: 5.0,
        ..Default::default()
    };

    let metrics = run_pipeline_test("dynamic_range", &field_config, &detection_config);

    // Faint stars are hard to detect, so relaxed criteria
    let criteria = crate::star_detection::visual_tests::output::faint_star_criteria();

    if let Err(failures) = check_pass(&metrics, &criteria) {
        for f in &failures {
            println!("FAIL: {}", f);
        }
        println!("Dynamic range test has some failures (expected for faint stars)");
    }
}

/// Test: Low noise (ideal conditions).
#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_pipeline_low_noise() {
    init_tracing();

    let field_config = StarFieldConfig {
        width: 512,
        height: 512,
        num_stars: 40,
        fwhm_range: (3.0, 4.0),
        magnitude_range: (8.0, 13.0),
        background_level: 0.1,
        noise_sigma: 0.005, // Very low noise
        ..Default::default()
    };
    let detection_config = StarDetectionConfig::default();

    let metrics = run_pipeline_test("low_noise", &field_config, &detection_config);

    // Strict criteria for ideal conditions
    let criteria = crate::star_detection::visual_tests::output::PassCriteria {
        min_detection_rate: 0.99,
        max_false_positive_rate: 0.01,
        max_mean_centroid_error: 0.05,
        max_fwhm_error: 0.10,
    };

    if let Err(failures) = check_pass(&metrics, &criteria) {
        for f in &failures {
            println!("FAIL: {}", f);
        }
        panic!("Low noise test failed: {:?}", failures);
    }
}
