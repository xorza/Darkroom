//! Full pipeline benchmark: CFA master creation -> calibration -> registration -> stacking.

use std::time::Instant;

use common::CancelToken;

use crate::concurrency;
use crate::io::image::cfa::CfaImage;
use crate::io::image::linear::LinearImage;
use crate::io::raw::load_raw_cfa;
use crate::stacking::combine::cache::CfaCache;
use crate::stacking::combine::stack::run_stacking;
use crate::testing::{calibration_dir, calibration_image_paths, init_tracing};
use crate::{
    CalibrationComponent, CalibrationMasters, CalibrationSet, DEFAULT_SIGMA_THRESHOLD,
    Normalization, ProgressCallback, RegistrationConfig, StackConfig, Star, StarDetectionConfig,
    StarDetector, register, stack, warp,
};

#[test]
#[ignore = "real-data pipeline benchmark; run explicitly with --ignored"]
fn bench_full_pipeline() {
    init_tracing();

    let cal_dir = calibration_dir();

    let total_start = Instant::now();

    println!("\n--- Step 1: Creating CFA master calibration frames ---");
    let step_start = Instant::now();

    let dark_paths = calibration_image_paths("Darks").unwrap_or_default();
    let flat_paths = calibration_image_paths("Flats").unwrap_or_default();
    let bias_paths = calibration_image_paths("Bias").unwrap_or_default();

    // Time each master separately to find the bottleneck
    let stack_cfa =
        |name: &str, paths: &[std::path::PathBuf], config: StackConfig| -> Option<CfaImage> {
            if paths.is_empty() {
                println!("  {name}: no frames, skipping");
                return None;
            }
            let config = if paths.len() < 8 {
                StackConfig {
                    normalization: config.normalization,
                    ..StackConfig::median()
                }
            } else {
                config
            };

            let t0 = Instant::now();
            let cache = CfaCache::from_paths(
                paths,
                &config.cache,
                ProgressCallback::default(),
                CancelToken::never(),
            )
            .unwrap();
            let load_ms = t0.elapsed().as_secs_f64() * 1000.0;

            let t1 = Instant::now();
            let result = run_stacking(&cache, &config);
            let stack_ms = t1.elapsed().as_secs_f64() * 1000.0;

            println!(
                "  {name}: {} frames, load={load_ms:.0}ms, stack={stack_ms:.0}ms, total={:.0}ms",
                paths.len(),
                t0.elapsed().as_secs_f64() * 1000.0
            );
            Some(result)
        };

    let dark = stack_cfa("Dark", &dark_paths, StackConfig::dark());
    let flat = stack_cfa("Flat", &flat_paths, StackConfig::flat());
    let bias = stack_cfa("Bias", &bias_paths, StackConfig::bias());

    let t_defect = Instant::now();
    let masters = CalibrationMasters::from_images(
        CalibrationSet {
            dark,
            flat,
            bias,
            flat_dark: None,
        },
        DEFAULT_SIGMA_THRESHOLD,
        CancelToken::never(),
    )
    .unwrap();
    println!(
        "  DefectMap: {:.0}ms",
        t_defect.elapsed().as_secs_f64() * 1000.0
    );

    println!(
        "  Masters: dark={}, flat={}, bias={}",
        masters
            .components()
            .any(|component| component == CalibrationComponent::Dark),
        masters
            .components()
            .any(|component| component == CalibrationComponent::Flat),
        masters
            .components()
            .any(|component| component == CalibrationComponent::Bias),
    );

    if let Some(defects) = masters.defect_summary() {
        println!(
            "  Defective pixels: {} ({:.4}%)",
            defects.hot_pixels + defects.cold_pixels,
            defects.percentage
        );
    }
    println!("  Step 1 total: {:?}", step_start.elapsed());

    println!("\n--- Step 2: Calibrating light frames ---");
    let step_start = Instant::now();

    let light_paths = calibration_image_paths("Lights").unwrap();
    assert!(!light_paths.is_empty(), "No light frames found");
    println!("  Loading and calibrating {} lights...", light_paths.len());

    let calibrated: Vec<LinearImage> = concurrency::try_par_map_limited(&light_paths, 3, |p| {
        let mut cfa = load_raw_cfa(p, &CancelToken::never()).unwrap();
        masters.calibrate(&mut cfa).unwrap();
        Ok::<_, ()>(cfa.demosaic(&CancelToken::never()).unwrap())
    })
    .unwrap();

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

    println!("\n--- Step 3: Detecting stars ---");
    let step_start = Instant::now();

    let det_config = StarDetectionConfig::precise_ground();
    let all_stars: Vec<_> = concurrency::try_par_map_limited(&calibrated, 3, |img| {
        let mut det = StarDetector::from_config(det_config.clone()).unwrap();
        Ok::<_, ()>(det.detect(img).stars)
    })
    .unwrap();
    for (i, stars) in all_stars.iter().enumerate() {
        println!("  Frame {}: {} stars", i, stars.len());
    }

    println!("  Elapsed: {:?}", step_start.elapsed());

    println!("\n--- Step 4: Registering lights (best precision) ---");
    let step_start = Instant::now();

    let reg_config = RegistrationConfig::precise_wide_field();

    let ref_stars = &all_stars[0];

    // Pair up frames to register (skip reference frame 0)
    let to_register: Vec<(&LinearImage, &[Star])> = calibrated[1..]
        .iter()
        .zip(all_stars[1..].iter())
        .map(|(img, stars)| (img, stars.as_slice()))
        .collect();

    let warped_frames: Vec<LinearImage> =
        concurrency::try_par_map_limited(&to_register, 3, |(img, stars)| {
            let result = register(ref_stars, stars, &reg_config)
                .unwrap_or_else(|e| panic!("Registration failed: {e}"));
            let warp_start = Instant::now();
            let warped = warp(img, &result.warp_transform(), &reg_config.warp).image;
            let warp_ms = warp_start.elapsed().as_secs_f64() * 1000.0;
            println!(
                "  {} inliers, RMS={:.3}px, reg={:.1}ms, warp={:.1}ms",
                result.num_inliers(),
                result.rms_error(),
                result.elapsed_ms(),
                warp_ms,
            );
            Ok::<_, ()>(warped)
        })
        .unwrap();

    println!("  Elapsed: {:?}", step_start.elapsed());

    // Build registered set: reference (unwarped) + warped frames
    let mut registered: Vec<&LinearImage> = Vec::with_capacity(calibrated.len());
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

    println!("\n--- Step 5: Stacking registered lights ---");
    let step_start = Instant::now();

    let registered_paths: Vec<_> = (0..registered.len())
        .map(|i| registered_dir.join(format!("registered_{:04}.tiff", i)))
        .collect();

    let stack_config = StackConfig {
        normalization: Normalization::Global,
        ..StackConfig::sigma_clipped(2.5)
    };

    let stacked = stack(
        &registered_paths,
        stack_config,
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .unwrap()
    .image;

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
