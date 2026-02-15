//! Full pipeline tests - tests complete star detection on various scenarios.

use crate::{AstroImage, ImageDimensions};

use crate::star_detection::tests::common::output::{
    DetectionMetrics, compute_detection_metrics, save_comparison, save_grayscale, save_metrics,
};
use crate::star_detection::{StarDetector, config::Config};
use crate::testing::synthetic::{StarFieldConfig, generate_star_field};
use common::test_utils::test_output_path;

mod challenging_tests;
mod standard_tests;

/// Run full detection pipeline on a synthetic star field and return metrics.
///
/// Generates the field, runs detection, saves input/comparison/metrics files,
/// and returns the detection metrics for the caller to validate.
fn run_test(
    name: &str,
    prefix: &str,
    field_config: &StarFieldConfig,
    detection_config: &Config,
) -> DetectionMetrics {
    let (pixels, ground_truth) = generate_star_field(field_config);
    let pixels_vec = pixels.into_vec();

    save_grayscale(
        &pixels_vec,
        field_config.width,
        field_config.height,
        &test_output_path(&format!(
            "synthetic_starfield/{}_{}_input.png",
            prefix, name
        )),
    );

    let image = AstroImage::from_pixels(
        ImageDimensions::new(field_config.width, field_config.height, 1),
        pixels_vec.clone(),
    );
    let mut detector = StarDetector::from_config(detection_config.clone());
    let result = detector.detect(&image);
    let stars = result.stars;

    let match_radius = field_config.fwhm_range.1 * 2.0;
    let metrics = compute_detection_metrics(&ground_truth, &stars, match_radius);

    save_comparison(
        &pixels_vec,
        field_config.width,
        field_config.height,
        &ground_truth,
        &stars,
        match_radius,
        &test_output_path(&format!(
            "synthetic_starfield/{}_{}_comparison.png",
            prefix, name
        )),
    );

    save_metrics(
        &metrics,
        &test_output_path(&format!(
            "synthetic_starfield/{}_{}_metrics.txt",
            prefix, name
        )),
    );

    println!("\n{} results:", name);
    println!("{}", metrics);

    metrics
}
