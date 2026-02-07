use crate::CalibrationMasters;
use crate::stacking::{
    FrameType, Normalization, ProgressCallback, StackConfig, stack_with_progress,
};
use crate::testing::{calibration_dir, calibration_image_paths, init_tracing};

#[test]
#[ignore] // Requires LUMOS_CALIBRATION_DIR with Bias/, Darks/, Flats/ subdirectories
fn test_create_calibration_masters() {
    init_tracing();

    let Some(cal_dir) = calibration_dir() else {
        return;
    };

    // Report which subdirectories are available
    for subdir in ["Bias", "Darks", "Flats"] {
        let path = cal_dir.join(subdir);
        if path.exists() {
            let count = common::file_utils::astro_image_files(&path).len();
            println!("{subdir}: {count} frames");
        } else {
            println!("{subdir}: not found (skipping)");
        }
    }

    let config = StackConfig::sigma_clipped(3.0);
    let masters = CalibrationMasters::create(&cal_dir, config, ProgressCallback::default())
        .expect("Failed to create calibration masters");

    println!(
        "Created masters: bias={}, dark={}, flat={}, hot_pixels={}",
        masters.master_bias.is_some(),
        masters.master_dark.is_some(),
        masters.master_flat.is_some(),
        masters.hot_pixel_map.is_some(),
    );

    // At least one master should be created
    assert!(
        masters.master_bias.is_some()
            || masters.master_dark.is_some()
            || masters.master_flat.is_some(),
        "No calibration frames found in any subdirectory"
    );

    if let Some(ref bias) = masters.master_bias {
        println!(
            "Master bias: {}x{}x{}",
            bias.width(),
            bias.height(),
            bias.channels()
        );
    }
    if let Some(ref dark) = masters.master_dark {
        println!(
            "Master dark: {}x{}x{} (bias-subtracted: {})",
            dark.width(),
            dark.height(),
            dark.channels(),
            masters.master_bias.is_some()
        );
    }
    if let Some(ref flat) = masters.master_flat {
        println!(
            "Master flat: {}x{}x{}",
            flat.width(),
            flat.height(),
            flat.channels()
        );
    }
    if let Some(ref hp) = masters.hot_pixel_map {
        println!(
            "Hot pixel map: {} pixels ({:.3}%)",
            hp.count,
            hp.percentage()
        );
    }

    // Save masters for use by other tests
    let masters_dir = cal_dir.join("calibration_masters");
    masters
        .save_to_directory(&masters_dir)
        .expect("Failed to save masters");
    println!("Saved masters to {}", masters_dir.display());
}

#[test]
#[ignore] // Requires LUMOS_CALIBRATION_DIR with Lights/ and calibration_masters/
fn test_calibrate_lights() {
    init_tracing();

    let Some(cal_dir) = calibration_dir() else {
        return;
    };

    // Load master frames from calibration_masters directory
    let masters_dir = cal_dir.join("calibration_masters");
    assert!(
        masters_dir.exists(),
        "calibration_masters directory not found — run test_create_calibration_masters first"
    );

    let config = StackConfig::sigma_clipped(3.0);
    let masters = CalibrationMasters::load(&masters_dir, config).unwrap();
    println!(
        "Loaded masters: dark={}, flat={}, bias={}, hot_pixels={}",
        masters.master_dark.is_some(),
        masters.master_flat.is_some(),
        masters.master_bias.is_some(),
        masters.hot_pixel_map.is_some(),
    );
    assert!(
        masters.master_dark.is_some() || masters.master_flat.is_some(),
        "No master dark or flat found"
    );

    if let Some(ref hp) = masters.hot_pixel_map {
        println!(
            "Hot pixel map: {} pixels ({:.3}%)",
            hp.count,
            hp.percentage()
        );
    }

    // Load light frame paths
    let Some(light_paths) = calibration_image_paths("Lights") else {
        eprintln!("No Lights directory found, skipping");
        return;
    };
    assert!(!light_paths.is_empty(), "No light frames found");
    println!("Found {} light frames", light_paths.len());

    // Calibrate and save each light
    let output_dir = cal_dir.join("calibrated_lights");
    std::fs::create_dir_all(&output_dir).expect("Failed to create calibrated_lights dir");

    for (i, path) in light_paths.iter().enumerate() {
        let filename = path.file_stem().unwrap().to_string_lossy();
        println!(
            "[{}/{}] Calibrating {}...",
            i + 1,
            light_paths.len(),
            filename
        );

        let mut light = crate::AstroImage::from_file(path).unwrap();
        masters.calibrate(&mut light);

        let out_path = output_dir.join(format!("{}_calibrated.tiff", filename));
        let img: imaginarium::Image = light.into();
        img.save_file(&out_path)
            .expect("Failed to save calibrated light");
        println!("  Saved: {}", out_path.display());
    }

    println!(
        "Calibrated {} lights to {}",
        light_paths.len(),
        output_dir.display()
    );
}

#[test]
#[ignore] // Requires LUMOS_CALIBRATION_DIR with registered_lights/
fn test_stack_registered_lights() {
    init_tracing();

    let Some(cal_dir) = calibration_dir() else {
        return;
    };

    let registered_dir = cal_dir.join("registered_lights");
    assert!(
        registered_dir.exists(),
        "registered_lights directory not found — run registration tests first"
    );

    let paths =
        common::file_utils::files_with_extensions(&registered_dir, &["tiff", "tif", "fits", "fit"]);
    assert!(!paths.is_empty(), "No registered light frames found");
    println!("Stacking {} registered light frames...", paths.len());

    // Sigma-clipped mean with global normalization — best for registered lights
    let config = StackConfig {
        normalization: Normalization::Global,
        ..StackConfig::sigma_clipped(2.5)
    };

    let stacked = stack_with_progress(
        &paths,
        FrameType::Light,
        config,
        ProgressCallback::default(),
    )
    .expect("Failed to stack registered lights");

    println!(
        "Stacked result: {}x{}x{}",
        stacked.width(),
        stacked.height(),
        stacked.channels()
    );

    let out_path = cal_dir.join("stacked_light.tiff");
    let img: imaginarium::Image = stacked.into();
    img.save_file(&out_path)
        .expect("Failed to save stacked light");
    println!("Saved stacked light: {}", out_path.display());
}
