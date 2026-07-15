//! Full pipeline tests - tests complete star detection on various scenarios.

use crate::stacking::star_detection::config::Config;
use crate::stacking::star_detection::detector::StarDetector;
use crate::stacking::star_detection::test_common::output::image_writer::{
    save_comparison, save_grayscale,
};
use crate::stacking::star_detection::test_common::output::metrics::{
    DetectionMetrics, compute_detection_metrics, save_metrics,
};
use crate::testing::synthetic::observe::SimFrame;
use common::test_utils::test_output_path;

mod challenging_tests;
mod standard_tests;

/// Run full detection on a pre-rendered forward-model frame and return metrics.
///
/// Runs detection on `frame.image`, grades it against `frame.truth.sources`, saves
/// input/comparison/metrics files, and returns the metrics for the caller to validate.
fn run_test(
    name: &str,
    prefix: &str,
    frame: &SimFrame,
    detection_config: &Config,
) -> DetectionMetrics {
    let width = frame.image.width();
    let height = frame.image.height();
    let pixels = frame.image.channel(0).pixels();
    let truth = &frame.truth.sources;

    save_grayscale(
        pixels,
        width,
        height,
        &test_output_path(&format!(
            "synthetic_starfield/{}_{}_input.png",
            prefix, name
        )),
    );

    let mut detector = StarDetector::from_config(detection_config.clone()).unwrap();
    let stars = detector.detect(&frame.image).stars;

    // All forward-model sources share the instrument PSF; match within ~2 FWHM.
    let match_radius = truth.first().map_or(8.0, |s| s.fwhm) * 2.0;
    let metrics = compute_detection_metrics(truth, &stars, match_radius);

    save_comparison(
        pixels,
        width,
        height,
        truth,
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
