//! Full pipeline benchmark: CFA master creation -> calibration -> registration -> stacking.

use std::time::Instant;

use crate::raw::load_raw_cfa;
use crate::testing::{calibration_dir, calibration_image_paths, init_tracing};
use crate::{
    AstroImage, CalibrationMasters, DEFAULT_HOT_PIXEL_SIGMA, FrameType, Normalization,
    ProgressCallback, RegistrationConfig, StackConfig, Star, StarDetectionConfig, StarDetector,
    register, stack_with_progress, warp,
};

#[test]
#[ignore]
fn diag_dark_pixel_distribution() {
    use crate::calibration_masters::defect_map::DefectMap;

    init_tracing();

    let dark_paths = calibration_image_paths("Darks").unwrap();
    let bias_paths = calibration_image_paths("Bias").unwrap();

    let empty: Vec<String> = Vec::new();
    let masters = CalibrationMasters::from_raw_files(
        &dark_paths,
        &empty,
        &bias_paths,
        &empty,
        DEFAULT_HOT_PIXEL_SIGMA,
    )
    .expect("Failed to create masters");

    let dark = masters.master_dark.as_ref().unwrap();
    let pixels = dark.data.pixels();
    let n = pixels.len();

    // Sort all pixels for percentile analysis
    let mut sorted = pixels.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());

    println!("\n=== Master Dark Pixel Distribution ({n} pixels) ===");
    println!("  min:    {:.8}", sorted[0]);
    println!("  p0.1%:  {:.8}", sorted[n / 1000]);
    println!("  p1%:    {:.8}", sorted[n / 100]);
    println!("  p5%:    {:.8}", sorted[n * 5 / 100]);
    println!("  p25%:   {:.8}", sorted[n / 4]);
    println!("  median: {:.8}", sorted[n / 2]);
    println!("  p75%:   {:.8}", sorted[n * 3 / 4]);
    println!("  p95%:   {:.8}", sorted[n * 95 / 100]);
    println!("  p99%:   {:.8}", sorted[n * 99 / 100]);
    println!("  p99.9%: {:.8}", sorted[n * 999 / 1000]);
    println!("  max:    {:.8}", sorted[n - 1]);

    // Count above the threshold from the trace
    let threshold = 0.000535;
    let above = sorted.iter().filter(|&&v| v > threshold).count();
    println!(
        "\n  Above threshold {threshold:.6}: {above} ({:.4}%)",
        100.0 * above as f64 / n as f64
    );

    // Cumulative histogram
    let bins = [
        0.0, 0.00001, 0.00002, 0.00003, 0.00004, 0.00005, 0.0001, 0.0002, 0.0005, 0.001, 0.005,
        0.01, 0.1, 1.0,
    ];
    println!("\n  Cumulative histogram:");
    for &b in &bins {
        let count = sorted.partition_point(|v| *v <= b);
        println!(
            "    <= {b:.6}: {count} ({:.2}%)",
            100.0 * count as f64 / n as f64
        );
    }

    // Per-color breakdown
    let cfa = dark.metadata.cfa_type.as_ref().unwrap();
    let w = dark.data.width();
    for color in 0..3u8 {
        let mut color_pixels: Vec<f32> = pixels
            .iter()
            .enumerate()
            .filter(|&(i, _)| cfa.color_at(i % w, i / w) == color)
            .map(|(_, &v)| v)
            .collect();
        color_pixels.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let cn = color_pixels.len();
        if cn == 0 {
            continue;
        }
        let cmed = color_pixels[cn / 2];
        let cmad = {
            let mut devs: Vec<f32> = color_pixels.iter().map(|v| (v - cmed).abs()).collect();
            devs.sort_by(|a, b| a.partial_cmp(b).unwrap());
            devs[cn / 2]
        };
        let csigma = cmad * 1.4826;
        let floor_sigma = csigma.max(cmed * 0.1).max(5e-4);
        let upper = cmed + 5.0 * floor_sigma;
        let cabove = color_pixels.iter().filter(|&&v| v > upper).count();
        println!(
            "\n  Color {color}: {cn} pixels, median={cmed:.8}, MAD={cmad:.8}, \
             computed_sigma={csigma:.8}, floor_sigma={floor_sigma:.8}"
        );
        println!(
            "    upper_threshold={upper:.8}, above: {cabove} ({:.4}%)",
            100.0 * cabove as f64 / cn as f64
        );
        println!(
            "    p95={:.8} p99={:.8} p99.9={:.8} max={:.8}",
            color_pixels[cn * 95 / 100],
            color_pixels[cn * 99 / 100],
            color_pixels[cn * 999 / 1000],
            color_pixels[cn - 1]
        );

        // What if we used computed_sigma without floor?
        if csigma > 0.0 {
            let upper_no_floor = cmed + 5.0 * csigma;
            let above_no_floor = color_pixels.iter().filter(|&&v| v > upper_no_floor).count();
            println!(
                "    WITHOUT floor: upper={upper_no_floor:.8}, above: {above_no_floor} ({:.4}%)",
                100.0 * above_no_floor as f64 / cn as f64
            );
        }
    }

    // Also report the DefectMap stats
    let defect_map = masters.defect_map.as_ref().unwrap();
    println!(
        "\n  DefectMap: hot={}, cold={}, total={} ({:.4}%)",
        defect_map.hot_count(),
        defect_map.cold_count(),
        defect_map.count(),
        defect_map.percentage()
    );
}

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
    let masters = CalibrationMasters::from_raw_files(
        &dark_paths,
        &flat_paths,
        &bias_paths,
        &empty,
        DEFAULT_HOT_PIXEL_SIGMA,
    )
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
            let warp_start = Instant::now();
            let mut warped = (*img).clone();
            warp(img, &mut warped, &result.warp_transform(), &reg_config);
            let warp_ms = warp_start.elapsed().as_secs_f64() * 1000.0;
            println!(
                "  {} inliers, RMS={:.3}px, reg={:.1}ms, warp={:.1}ms",
                result.num_inliers, result.rms_error, result.elapsed_ms, warp_ms,
            );
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
