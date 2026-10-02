//! Real data registration tests.
//!
//! These tests load the RAW light frames of the bundled dataset, run star detection, and
//! register them to verify the pipeline end to end.

use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::Instant;

use ::quickbench::quick_bench;
use common::{CancelToken, TempDir};

use crate::io::image::linear::LinearImage;
use crate::io::raw::load_raw_cfa;
use crate::math::size2us::Size2us;
use crate::stacking::registration::config::Config as RegistrationConfig;
use crate::stacking::registration::distortion::sip::{SipConfig, SipPolynomial};
use crate::stacking::registration::register;
use crate::stacking::registration::resample::warp;
use crate::stacking::registration::transform::TransformModel;
use crate::stacking::star_detection::config::Config;
use crate::stacking::star_detection::config::measurement_config::{CentroidMethod, NoiseModel};
use crate::stacking::star_detection::detector::StarDetector;
use crate::testing::real_data::raw_frames;

/// A RAW light, demosaiced without calibration: registration needs its stars, not its noise floor.
fn load_light(path: &Path) -> LinearImage {
    load_raw_cfa(path, &CancelToken::never())
        .expect("load a RAW light")
        .demosaic(&CancelToken::never())
        .expect("demosaic a RAW light")
}

/// The dataset's first two lights.
#[derive(Debug)]
struct TwoLights {
    first: LinearImage,
    second: LinearImage,
}

/// The first and last RAW lights of the dataset, demosaiced: two frames of one field, offset by
/// the drift of a night's sequence.
fn load_two_lights() -> TwoLights {
    let lights = raw_frames("Lights");
    assert!(
        lights.len() >= 2,
        "real-data Lights/ needs two frames to register"
    );
    println!(
        "Loading {} and {}",
        lights[0].display(),
        lights[lights.len() - 1].display()
    );
    TwoLights {
        first: load_light(&lights[0]),
        second: load_light(&lights[lights.len() - 1]),
    }
}

#[test]
fn register_two_lights() {
    let TwoLights {
        first: img1,
        second: img2,
    } = load_two_lights();

    println!(
        "Image 1: {}x{} ({} ch)",
        img1.width(),
        img1.height(),
        img1.channels()
    );
    println!(
        "Image 2: {}x{} ({} ch)",
        img2.width(),
        img2.height(),
        img2.channels()
    );

    // Detect stars with precise Moffat centroids
    let star_config = Config::precise_ground();
    let mut detector = StarDetector::from_config(star_config).unwrap();

    let result1 = detector.detect(&img1);
    let result2 = detector.detect(&img2);

    println!("Stars in image 1: {}", result1.stars.len());
    println!("Stars in image 2: {}", result2.stars.len());

    assert!(
        result1.stars.len() >= 10,
        "Expected at least 10 stars in image 1, found {}",
        result1.stars.len()
    );
    assert!(
        result2.stars.len() >= 10,
        "Expected at least 10 stars in image 2, found {}",
        result2.stars.len()
    );

    // Register image 2 to image 1 WITHOUT SIP first (baseline).
    let reg_config = RegistrationConfig {
        transform_type: TransformModel::Auto,
        sip: None,
        ..RegistrationConfig::default()
    };

    let result =
        register(&result1.stars, &result2.stars, &reg_config).expect("Registration should succeed");

    let baseline_rms = result.rms_error();

    println!("Registration result:");
    println!("  Matched stars: {}", result.num_inliers());
    println!("  RMS error:     {baseline_rms:.4} pixels");
    println!("  Elapsed:       {:.1} ms", result.elapsed_ms());

    let t = result.transform().translation_components();
    println!("  Translation:   ({:.2}, {:.2})", t.x, t.y);
    println!(
        "  Rotation:      {:.4} rad ({:.2} deg)",
        result.transform().rotation_angle(),
        result.transform().rotation_angle().to_degrees()
    );
    println!("  Scale:         {:.6}", result.transform().scale_factor());

    // Now fit SIP on the SAME inliers from the SAME RANSAC run for fair comparison.
    // Reconstruct inlier positions from match indices.
    let max_stars = reg_config.matching.max_stars;
    let ref_positions: Vec<glam::DVec2> = result1
        .stars
        .iter()
        .take(max_stars)
        .map(|s| s.pos)
        .collect();
    let target_positions: Vec<glam::DVec2> = result2
        .stars
        .iter()
        .take(max_stars)
        .map(|s| s.pos)
        .collect();

    let inlier_ref: Vec<glam::DVec2> = result
        .matched_stars()
        .iter()
        .map(|star_match| ref_positions[star_match.reference])
        .collect();
    let inlier_target: Vec<glam::DVec2> = result
        .matched_stars()
        .iter()
        .map(|star_match| target_positions[star_match.target])
        .collect();

    let sip_config = SipConfig {
        order: 4,
        reference_point: None,
        ..Default::default()
    };

    let sip = SipPolynomial::fit_from_transform(
        &inlier_ref,
        &inlier_target,
        &result.transform(),
        &sip_config,
    )
    .unwrap();

    let corrected_residuals = sip.polynomial.compute_corrected_residuals(
        &inlier_ref,
        &inlier_target,
        &result.transform(),
    );
    let sip_rms = (corrected_residuals.iter().map(|r| r * r).sum::<f64>()
        / corrected_residuals.len() as f64)
        .sqrt();

    let improvement = (baseline_rms - sip_rms) / baseline_rms * 100.0;

    println!("\nSIP correction (order 4) on same inliers:");
    println!("  Baseline RMS:      {baseline_rms:.4} pixels");
    println!("  With SIP RMS:      {sip_rms:.4} pixels");
    println!("  Improvement:       {improvement:.1}%");
    println!(
        "  Max SIP correction: {:.4} pixels",
        sip.polynomial
            .max_correction(Size2us::new(img1.width(), img1.height()), 50.0)
    );

    assert!(
        sip_rms <= baseline_rms + 1e-10,
        "SIP should not worsen RMS: baseline={baseline_rms:.4}, sip={sip_rms:.4}"
    );

    assert!(
        result.num_inliers() >= 5,
        "Expected at least 5 inlier matches, got {}",
        result.num_inliers()
    );
    assert!(
        result.rms_error() < 1.5,
        "Expected RMS error < 1.5 pixels, got {:.4}",
        result.rms_error()
    );

    // Warp img2 to align with img1 and measure time
    let warp_start = Instant::now();
    let warped = warp(&img2, &result.warp_transform(), &reg_config.warp).image;
    let warp_elapsed = warp_start.elapsed();

    println!(
        "\nWarp result: {}x{} image warped in {:.1} ms",
        warped.width(),
        warped.height(),
        warp_elapsed.as_secs_f64() * 1000.0
    );
}

/// Every light and the path it came from.
#[derive(Debug)]
struct Lights {
    images: Vec<LinearImage>,
    paths: Vec<PathBuf>,
}

/// Every RAW light of the dataset, demosaiced, with its path.
fn load_all_lights() -> Lights {
    let paths = raw_frames("Lights");
    let images = paths.iter().map(|path| load_light(path)).collect();
    Lights { images, paths }
}

#[quick_bench(warmup_iters = 0, iters = 1)]
fn bench_register_and_warp_all(b: ::quickbench::Bencher) {
    let Lights { images, paths } = load_all_lights();
    // The warped frames go to a fresh directory, never into the dataset.
    let output_dir = TempDir::new("lumos-registered-lights");
    println!(
        "Loaded {} lights; writing under {}",
        images.len(),
        output_dir.path().display()
    );

    b.bench(|| {
        let star_config = Config::precise_ground();
        let mut detector = StarDetector::from_config(star_config).unwrap();

        // Detect stars in all frames
        let detections: Vec<_> = images.iter().map(|img| detector.detect(img)).collect();

        let reg_config = RegistrationConfig::default();
        let ref_stars = &detections[0].stars;

        println!(
            "Reference: {:?} ({} stars)",
            paths[0].file_name().unwrap(),
            ref_stars.len()
        );

        // Save reference frame as-is
        let tiff_name =
            |path: &PathBuf| format!("{}.tiff", path.file_stem().unwrap().to_string_lossy());
        let ref_output = output_dir.join(tiff_name(&paths[0]));
        images[0]
            .save(&ref_output)
            .expect("Failed to save reference frame");

        // Register and warp each subsequent frame
        for i in 1..images.len() {
            let name = paths[i].file_name().unwrap();
            let target_stars = &detections[i].stars;

            let result = match register(ref_stars, target_stars, &reg_config) {
                Ok(r) => r,
                Err(e) => {
                    println!("  {name:?}: FAILED ({e:?}), skipping");
                    continue;
                }
            };

            println!(
                "  {:?}: {} inliers, RMS {:.4} px, {:.1} ms",
                name,
                result.num_inliers(),
                result.rms_error(),
                result.elapsed_ms(),
            );

            let warped = warp(&images[i], &result.warp_transform(), &reg_config.warp).image;

            let output_path = output_dir.join(tiff_name(&paths[i]));
            warped
                .save(&output_path)
                .expect("Failed to save warped frame");
        }

        println!(
            "Saved {} registered frames to {:?}",
            images.len(),
            output_dir
        );
    });
}

#[quick_bench(warmup_iters = 3, iters = 30)]
fn bench_register_stars(b: ::quickbench::Bencher) {
    let TwoLights {
        first: img1,
        second: img2,
    } = load_two_lights();

    // Pre-detect stars (not part of the benchmark)
    let star_config = Config::default();
    let mut detector = StarDetector::from_config(star_config).unwrap();
    let result1 = detector.detect(&img1);
    let result2 = detector.detect(&img2);

    let reg_config = RegistrationConfig::default();

    b.bench(|| {
        black_box(register(
            black_box(&result1.stars),
            black_box(&result2.stars),
            &reg_config,
        ))
    });
}

/// PR1 validation: inverse-variance-weighted PSF fitting should not worsen (and ideally
/// improves) registration RMS vs unweighted, by producing lower-variance sub-pixel
/// centroids. Runs the pair of lights through `GaussianFit` with and without a
/// `NoiseModel`, registers each, and compares.
#[test]
fn weighted_fit_registration_rms() {
    let TwoLights {
        first: img1,
        second: img2,
    } = load_two_lights();

    // 30,000 e-/normalized unit is representative of physical gain × the 14-bit signal range.
    let noise_model = NoiseModel::from_normalized(30_000.0, 30.0);

    let register_with = |noise: Option<NoiseModel>| -> (f64, usize) {
        let mut config = Config::precise_ground();
        config.measurement.centroid_method = CentroidMethod::GaussianFit;
        config.measurement.noise_model = noise;
        let mut detector = StarDetector::from_config(config).unwrap();
        let s1 = detector.detect(&img1).stars;
        let s2 = detector.detect(&img2).stars;
        let mut reg_config = RegistrationConfig {
            transform_type: TransformModel::Auto,
            sip: None,
            ..RegistrationConfig::default()
        };
        // Seeded, so the two runs differ only in their centroids.
        reg_config.ransac.seed = Some(0x5EED);
        let r = register(&s1, &s2, &reg_config).expect("registration should succeed");
        (r.rms_error(), r.num_inliers())
    };

    let (unweighted_rms, unweighted_n) = register_with(None);
    let (weighted_rms, weighted_n) = register_with(Some(noise_model));

    println!("PR1 weighted-fit registration:");
    println!("  unweighted: RMS {unweighted_rms:.4} px, {unweighted_n} matches");
    println!("  weighted:   RMS {weighted_rms:.4} px, {weighted_n} matches");

    // Weighting must not meaningfully worsen registration. The two catalogs differ, so RANSAC keeps
    // different inlier sets; 5% is the margin "not meaningfully worse" allows on that.
    assert!(
        weighted_rms <= unweighted_rms * 1.05,
        "weighted RMS {weighted_rms:.4} should be ≤ unweighted {unweighted_rms:.4} ×1.05"
    );
}
