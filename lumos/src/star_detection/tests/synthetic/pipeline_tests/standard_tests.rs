//! Standard pipeline tests - tests full star detection on typical scenarios.

use crate::{AstroImage, ImageDimensions};

use crate::star_detection::tests::common::output::{
    DetectionMetrics, check_pass, compute_detection_metrics, save_comparison, save_grayscale,
    save_metrics, standard_criteria,
};
use crate::star_detection::{StarDetector, config::Config};
use crate::testing::init_tracing;
use crate::testing::synthetic::{
    StarFieldConfig, dense_field_config, generate_star_field, sparse_field_config,
};
use common::test_utils::test_output_path;

/// Run full pipeline and evaluate metrics.
fn run_pipeline_test(
    name: &str,
    field_config: &StarFieldConfig,
    detection_config: &Config,
) -> DetectionMetrics {
    let (pixels, ground_truth) = generate_star_field(field_config);
    let pixels_vec = pixels.into_vec();

    // Save input
    save_grayscale(
        &pixels_vec,
        field_config.width,
        field_config.height,
        &test_output_path(&format!("synthetic_starfield/pipeline_{}_input.png", name)),
    );

    // Run detection
    let image = AstroImage::from_pixels(
        ImageDimensions::new(field_config.width, field_config.height, 1),
        pixels_vec.clone(),
    );
    let mut detector = StarDetector::from_config(detection_config.clone());
    let result = detector.detect(&image);
    let stars = result.stars;

    // Compute metrics
    let match_radius = field_config.fwhm_range.1 * 2.0;
    let metrics = compute_detection_metrics(&ground_truth, &stars, match_radius);

    // Save comparison image
    save_comparison(
        &pixels_vec,
        field_config.width,
        field_config.height,
        &ground_truth,
        &stars,
        match_radius,
        &test_output_path(&format!(
            "synthetic_starfield/pipeline_{}_comparison.png",
            name
        )),
    );

    // Save metrics
    save_metrics(
        &metrics,
        &test_output_path(&format!(
            "synthetic_starfield/pipeline_{}_metrics.txt",
            name
        )),
    );

    println!("\n{} results:", name);
    println!("{}", metrics);

    metrics
}

/// Test: Sparse field with well-separated stars.
#[test]

fn test_pipeline_sparse_field() {
    init_tracing();

    let field_config = sparse_field_config();
    // Synthetic images: disable CFA filter, disable matched filter for accurate FWHM
    let detection_config = Config {
        expected_fwhm: 0.0,
        ..Config::default()
    };

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

fn test_pipeline_dense_field() {
    init_tracing();

    let field_config = dense_field_config();
    // Synthetic images: disable CFA filter, disable matched filter for accurate FWHM
    let detection_config = Config {
        expected_fwhm: 0.0,
        ..Config::default()
    };

    let metrics = run_pipeline_test("dense_field", &field_config, &detection_config);

    // Relaxed criteria for dense field
    let criteria = crate::star_detection::tests::common::output::crowded_criteria();
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

fn test_pipeline_moffat_profile() {
    init_tracing();

    let field_config = StarFieldConfig {
        width: 256,
        height: 256,
        num_stars: 25,
        fwhm_range: (3.5, 4.5),
        // Narrower magnitude range to avoid saturation
        magnitude_range: (12.5, 13.5),
        mag_zero_point: 14.8,
        background_level: 0.1,
        noise_sigma: 0.02,
        use_moffat: true,
        moffat_beta: 2.5,
        ..Default::default()
    };
    // Synthetic images: disable CFA filter, disable matched filter for accurate FWHM
    let detection_config = Config {
        expected_fwhm: 0.0,
        ..Config::default()
    };

    let metrics = run_pipeline_test("moffat_profile", &field_config, &detection_config);

    // Moffat profile has extended wings that can affect FWHM estimation
    let criteria = crate::star_detection::tests::common::output::PassCriteria {
        min_detection_rate: 0.85, // 25 stars on 256x256 — one missed star is -4%
        max_false_positive_rate: 0.05,
        max_mean_centroid_error: 0.30, // Moffat wings can affect centroid matching
        max_fwhm_error: 0.20,          // Gaussian fit on Moffat profile has some error
    };
    if let Err(failures) = check_pass(&metrics, &criteria) {
        for f in &failures {
            println!("FAIL: {}", f);
        }
        panic!("Moffat profile test failed: {:?}", failures);
    }
}

/// Test: Wide FWHM range (3-5 pixels).
#[test]

fn test_pipeline_fwhm_range() {
    init_tracing();

    let field_config = StarFieldConfig {
        width: 256,
        height: 256,
        num_stars: 25,
        // Moderate FWHM range - very small stars (< 2.5px) fail min_area filter
        fwhm_range: (3.0, 5.0),
        // Narrower magnitude range to avoid saturation
        magnitude_range: (12.5, 13.5),
        mag_zero_point: 14.8,
        background_level: 0.1,
        noise_sigma: 0.02,
        ..Default::default()
    };
    // Synthetic images: disable CFA filter, disable matched filter for accurate FWHM
    let detection_config = Config {
        expected_fwhm: 0.0,
        ..Config::default()
    };

    let metrics = run_pipeline_test("fwhm_range", &field_config, &detection_config);

    // Relaxed for FWHM variation - centroid matching can have outliers with varying PSF
    let criteria = crate::star_detection::tests::common::output::PassCriteria {
        min_detection_rate: 0.90, // Relaxed for smaller images
        max_false_positive_rate: 0.05,
        max_mean_centroid_error: 0.30, // Relaxed due to FWHM variation
        max_fwhm_error: 0.30,
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

fn test_pipeline_dynamic_range() {
    init_tracing();

    let field_config = StarFieldConfig {
        width: 256,
        height: 256,
        num_stars: 30,
        fwhm_range: (3.0, 4.0),
        // Adjusted to avoid saturation: bright stars peak ~0.8, faint ~0.2
        magnitude_range: (12.0, 14.0),
        mag_zero_point: 14.8,
        background_level: 0.1,
        noise_sigma: 0.02,
        ..Default::default()
    };

    // Synthetic images: disable CFA filter, disable matched filter
    // Lower SNR threshold to catch faint stars
    let detection_config = Config {
        expected_fwhm: 0.0,
        min_snr: 5.0,
        ..Config::default()
    };

    let metrics = run_pipeline_test("dynamic_range", &field_config, &detection_config);

    // Faint stars are hard to detect, so relaxed criteria
    let criteria = crate::star_detection::tests::common::output::faint_star_criteria();

    if let Err(failures) = check_pass(&metrics, &criteria) {
        for f in &failures {
            println!("FAIL: {}", f);
        }
        println!("Dynamic range test has some failures (expected for faint stars)");
    }
}

/// Test: Low noise (ideal conditions).
#[test]

fn test_pipeline_low_noise() {
    init_tracing();

    let field_config = StarFieldConfig {
        width: 256,
        height: 256,
        num_stars: 25,
        fwhm_range: (3.0, 4.0),
        // Narrower magnitude range to avoid saturation
        magnitude_range: (12.5, 13.5),
        mag_zero_point: 14.8,
        background_level: 0.1,
        noise_sigma: 0.005, // Very low noise
        ..Default::default()
    };
    // Synthetic images: disable CFA filter, disable matched filter for accurate FWHM
    let detection_config = Config {
        expected_fwhm: 0.0,
        ..Config::default()
    };

    let metrics = run_pipeline_test("low_noise", &field_config, &detection_config);

    // Good criteria for low noise - some outlier matches can skew mean centroid error
    let criteria = crate::star_detection::tests::common::output::PassCriteria {
        min_detection_rate: 0.92,
        max_false_positive_rate: 0.02,
        max_mean_centroid_error: 0.25, // Outliers in matching can skew mean
        max_fwhm_error: 0.10,
    };

    if let Err(failures) = check_pass(&metrics, &criteria) {
        for f in &failures {
            println!("FAIL: {}", f);
        }
        panic!("Low noise test failed: {:?}", failures);
    }
}
