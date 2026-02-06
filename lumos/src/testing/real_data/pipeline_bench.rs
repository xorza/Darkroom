//! Full pipeline benchmark: master creation -> calibration -> registration -> stacking.

use std::time::Instant;

use crate::testing::{calibration_dir, calibration_image_paths, init_tracing};
use crate::{
    AstroImage, CalibrationMasters, FrameType, Normalization, ProgressCallback, RegistrationConfig,
    StackConfig, StarDetectionConfig, StarDetector, TransformType, register, stack_with_progress,
    warp,
};

#[test]
#[ignore] // Requires LUMOS_CALIBRATION_DIR
fn bench_full_pipeline() {
    init_tracing();

    let Some(cal_dir) = calibration_dir() else {
        return;
    };

    let total_start = Instant::now();

    // =========================================================================
    // Step 1: Create master calibration frames
    // =========================================================================
    println!("\n--- Step 1: Creating master calibration frames ---");
    let step_start = Instant::now();

    let masters = CalibrationMasters::create(
        &cal_dir,
        StackConfig::sigma_clipped(2.5),
        ProgressCallback::default(),
    )
    .unwrap();

    if let Some(ref dark) = masters.master_dark {
        println!("  Master dark: {}x{}", dark.width(), dark.height());
    }
    if let Some(ref flat) = masters.master_flat {
        println!("  Master flat: {}x{}", flat.width(), flat.height());
    }
    if let Some(ref bias) = masters.master_bias {
        println!("  Master bias: {}x{}", bias.width(), bias.height());
    }
    if let Some(ref hp) = masters.hot_pixel_map {
        println!("  Hot pixels: {} ({:.4}%)", hp.count, hp.percentage());
    }
    println!("  Elapsed: {:?}", step_start.elapsed());

    // =========================================================================
    // Step 2: Calibrate light frames
    // =========================================================================
    println!("\n--- Step 2: Calibrating light frames ---");
    let step_start = Instant::now();

    let light_paths = calibration_image_paths("Lights").unwrap();
    assert!(!light_paths.is_empty(), "No light frames found");
    println!("  Loading and calibrating {} lights...", light_paths.len());

    let mut calibrated: Vec<AstroImage> = light_paths
        .iter()
        .map(|p| {
            let mut img = AstroImage::from_file(p).unwrap();
            masters.calibrate(&mut img);
            img
        })
        .collect();

    println!("  Elapsed: {:?}", step_start.elapsed());

    // =========================================================================
    // Step 3: Detect stars
    // =========================================================================
    println!("\n--- Step 3: Detecting stars ---");
    let step_start = Instant::now();

    let det_config = StarDetectionConfig {
        edge_margin: 20,
        min_snr: 10.0,
        ..Default::default()
    };
    let mut detector = StarDetector::from_config(det_config);

    let all_stars: Vec<_> = calibrated
        .iter()
        .enumerate()
        .map(|(i, img)| {
            let result = detector.detect(img);
            println!(
                "  Frame {}: {} stars, median FWHM={:.2}, median SNR={:.1}",
                i,
                result.stars.len(),
                result.diagnostics.median_fwhm,
                result.diagnostics.median_snr,
            );
            result.stars
        })
        .collect();

    println!("  Elapsed: {:?}", step_start.elapsed());

    // =========================================================================
    // Step 4: Register lights to reference (best precision)
    // =========================================================================
    println!("\n--- Step 4: Registering lights (best precision) ---");
    let step_start = Instant::now();

    let reg_config = RegistrationConfig {
        transform_type: TransformType::Homography,
        max_stars: 500,
        min_matches: 20,
        ratio_tolerance: 0.005,
        ransac_iterations: 5000,
        confidence: 0.9999,
        sip_enabled: true,
        ..Default::default()
    };

    let ref_stars = &all_stars[0];

    for (i, (img, stars)) in calibrated
        .iter_mut()
        .zip(all_stars.iter())
        .enumerate()
        .skip(1)
    {
        match register(ref_stars, stars, &reg_config) {
            Ok(result) => {
                println!(
                    "  Frame {}: {} inliers, RMS={:.3}px, {:.1}ms",
                    i, result.num_inliers, result.rms_error, result.elapsed_ms,
                );
                let mut warped = img.clone();
                warp(img, &mut warped, &result.transform, &reg_config);
                *img = warped;
            }
            Err(e) => {
                panic!("Registration failed for frame {}: {}", i, e);
            }
        }
    }

    println!("  Elapsed: {:?}", step_start.elapsed());

    // =========================================================================
    // Step 5: Stack registered lights
    // =========================================================================
    println!("\n--- Step 5: Stacking registered lights ---");
    let step_start = Instant::now();

    // Save calibrated+registered images to temp dir for stacking API
    let tmp_dir = tempfile::tempdir().unwrap();
    let registered_paths: Vec<_> = calibrated
        .iter()
        .enumerate()
        .map(|(i, img)| {
            let path = tmp_dir.path().join(format!("registered_{:04}.tiff", i));
            let imag: imaginarium::Image = img.clone().into();
            imag.save_file(&path).unwrap();
            path
        })
        .collect();

    let stack_config = StackConfig {
        normalization: Normalization::Global,
        ..StackConfig::sigma_clipped(2.5)
    };

    let stacked = stack_with_progress(
        &registered_paths,
        FrameType::Light,
        stack_config,
        ProgressCallback::default(),
    )
    .unwrap();

    println!(
        "  Stacked result: {}x{}x{}",
        stacked.width(),
        stacked.height(),
        stacked.channels(),
    );
    println!("  Elapsed: {:?}", step_start.elapsed());

    // Save result
    let output_path = common::test_utils::test_output_path("real_data/pipeline_result.tiff");
    let img: imaginarium::Image = stacked.into();
    img.save_file(&output_path).unwrap();
    println!("\n  Saved: {}", output_path.display());

    println!("\n=== Total pipeline time: {:?} ===", total_start.elapsed());
}
