//! Full pipeline benchmark: CFA master creation -> calibration -> registration -> stacking.

use std::time::Instant;

use crate::raw::load_raw_cfa;
use crate::testing::{calibration_dir, calibration_image_paths, init_tracing};
use crate::{
    AstroImage, CalibrationMasters, FrameType, Normalization, ProgressCallback, RegistrationConfig,
    StackConfig, Star, StarDetectionConfig, StarDetector, register, stack_with_progress, warp,
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
    // Step 1: Create master calibration frames from raw CFA data
    // =========================================================================
    println!("\n--- Step 1: Creating CFA master calibration frames ---");
    let step_start = Instant::now();

    let dark_paths = calibration_image_paths("Darks").unwrap_or_default();
    let flat_paths = calibration_image_paths("Flats").unwrap_or_default();
    let bias_paths = calibration_image_paths("Bias").unwrap_or_default();

    let empty: Vec<String> = Vec::new();
    let masters = CalibrationMasters::from_raw_files(&dark_paths, &flat_paths, &bias_paths, &empty)
        .expect("Failed to create calibration masters");

    println!(
        "  Masters: dark={}, flat={}, bias={}",
        masters.master_dark.is_some(),
        masters.master_flat.is_some(),
        masters.master_bias.is_some(),
    );

    if let Some(ref hp) = masters.defect_map {
        println!("  Hot pixels: {} ({:.4}%)", hp.count(), hp.percentage());
    }
    println!("  Elapsed: {:?}", step_start.elapsed());

    // =========================================================================
    // Step 2: Calibrate light frames (CFA pipeline)
    // =========================================================================
    println!("\n--- Step 2: Calibrating light frames ---");
    let step_start = Instant::now();

    let light_paths = calibration_image_paths("Lights").unwrap();
    assert!(!light_paths.is_empty(), "No light frames found");
    println!("  Loading and calibrating {} lights...", light_paths.len());

    let calibrated: Vec<AstroImage> = common::parallel::par_map_limited(&light_paths, 3, |p| {
        let mut cfa = load_raw_cfa(p).unwrap();
        masters.calibrate(&mut cfa);
        cfa.demosaic()
    });

    println!("  Elapsed: {:?}", step_start.elapsed());

    // Save calibrated lights
    let calibrated_dir = cal_dir.join("calibrated_lights");
    std::fs::create_dir_all(&calibrated_dir).expect("Failed to create calibrated_lights dir");
    for (path, img) in light_paths.iter().zip(calibrated.iter()) {
        let filename = path.file_stem().unwrap().to_string_lossy();
        let out_path = calibrated_dir.join(format!("{}_calibrated.tiff", filename));
        img.save(&out_path)
            .expect("Failed to save calibrated light");
    }
    println!(
        "  Saved {} calibrated lights to {}",
        calibrated.len(),
        calibrated_dir.display()
    );

    // =========================================================================
    // Step 3: Detect stars
    // =========================================================================
    println!("\n--- Step 3: Detecting stars ---");
    let step_start = Instant::now();

    let det_config = StarDetectionConfig::precise_ground();
    let all_stars: Vec<_> = common::parallel::par_map_limited(&calibrated, 3, |img| {
        let mut det = StarDetector::from_config(det_config.clone());
        det.detect(img).stars
    });
    for (i, stars) in all_stars.iter().enumerate() {
        println!("  Frame {}: {} stars", i, stars.len());
    }

    println!("  Elapsed: {:?}", step_start.elapsed());

    // =========================================================================
    // Step 4: Register lights to reference (best precision)
    // =========================================================================
    println!("\n--- Step 4: Registering lights (best precision) ---");
    let step_start = Instant::now();

    let reg_config = RegistrationConfig::precise_wide_field();

    let ref_stars = &all_stars[0];

    // Pair up frames to register (skip reference frame 0)
    let to_register: Vec<(&AstroImage, &[Star])> = calibrated[1..]
        .iter()
        .zip(all_stars[1..].iter())
        .map(|(img, stars)| (img, stars.as_slice()))
        .collect();

    let warped_frames: Vec<AstroImage> =
        common::parallel::par_map_limited(&to_register, 3, |(img, stars)| {
            let result = register(ref_stars, stars, &reg_config)
                .unwrap_or_else(|e| panic!("Registration failed: {e}"));
            println!(
                "  {} inliers, RMS={:.3}px, {:.1}ms",
                result.num_inliers, result.rms_error, result.elapsed_ms,
            );
            let mut warped = (*img).clone();
            warp(img, &mut warped, &result.warp_transform(), &reg_config);
            warped
        });

    println!("  Elapsed: {:?}", step_start.elapsed());

    // Build registered set: reference (unwarped) + warped frames
    let mut registered: Vec<&AstroImage> = Vec::with_capacity(calibrated.len());
    registered.push(&calibrated[0]);
    for warped in &warped_frames {
        registered.push(warped);
    }

    // Save registered lights
    let registered_dir = cal_dir.join("registered_lights");
    std::fs::create_dir_all(&registered_dir).expect("Failed to create registered_lights dir");
    for (i, img) in registered.iter().enumerate() {
        let out_path = registered_dir.join(format!("registered_{:04}.tiff", i));
        img.save(&out_path)
            .expect("Failed to save registered light");
    }
    println!(
        "  Saved {} registered lights to {}",
        registered.len(),
        registered_dir.display()
    );

    // =========================================================================
    // Step 5: Stack registered lights
    // =========================================================================
    println!("\n--- Step 5: Stacking registered lights ---");
    let step_start = Instant::now();

    let registered_paths: Vec<_> = (0..registered.len())
        .map(|i| registered_dir.join(format!("registered_{:04}.tiff", i)))
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

    // Save stacked result
    let output_path = cal_dir.join("stacked_light.tiff");
    let img: imaginarium::Image = stacked.into();
    img.save_file(&output_path).unwrap();
    println!("  Saved: {}", output_path.display());

    println!("\n=== Total pipeline time: {:?} ===", total_start.elapsed());
}
