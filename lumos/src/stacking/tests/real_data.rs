use crate::CalibrationMasters;
use crate::stacking::{
    FrameType, Normalization, ProgressCallback, StackConfig, stack_with_progress,
};
use crate::testing::{calibration_dir, calibration_image_paths, init_tracing};

#[test]
#[ignore] // Requires LUMOS_CALIBRATION_DIR
fn test_create_master_dark_and_flat() {
    init_tracing();

    let Some(cal_dir) = calibration_dir() else {
        return;
    };

    let masters_dir = cal_dir.join("calibration_masters");
    std::fs::create_dir_all(&masters_dir).expect("Failed to create calibration_masters dir");

    // Stack darks
    let Some(dark_paths) = calibration_image_paths("Darks") else {
        eprintln!("No Darks directory found, skipping");
        return;
    };
    assert!(!dark_paths.is_empty(), "No dark frames found");

    println!("Stacking {} dark frames...", dark_paths.len());
    let config = StackConfig::sigma_clipped(3.0);
    let master_dark = stack_with_progress(
        &dark_paths,
        FrameType::Dark,
        config,
        ProgressCallback::default(),
    )
    .expect("Failed to stack darks");

    println!(
        "Master dark: {}x{}x{}",
        master_dark.width(),
        master_dark.height(),
        master_dark.channels()
    );

    let first_dark = crate::AstroImage::from_file(&dark_paths[0]).unwrap();
    assert_eq!(master_dark.dimensions(), first_dark.dimensions());

    let dark_path = masters_dir.join("master_dark_mean.tiff");
    let img: imaginarium::Image = master_dark.into();
    img.save_file(&dark_path)
        .expect("Failed to save master dark");
    println!("Saved master dark: {}", dark_path.display());

    // Stack flats
    let Some(flat_paths) = calibration_image_paths("Flats") else {
        eprintln!("No Flats directory found, skipping flats");
        return;
    };
    assert!(!flat_paths.is_empty(), "No flat frames found");

    println!("Stacking {} flat frames...", flat_paths.len());
    let config = StackConfig::sigma_clipped(2.5);
    let master_flat = stack_with_progress(
        &flat_paths,
        FrameType::Flat,
        config,
        ProgressCallback::default(),
    )
    .expect("Failed to stack flats");

    println!(
        "Master flat: {}x{}x{}",
        master_flat.width(),
        master_flat.height(),
        master_flat.channels()
    );

    let first_flat = crate::AstroImage::from_file(&flat_paths[0]).unwrap();
    assert_eq!(master_flat.dimensions(), first_flat.dimensions());

    let flat_path = masters_dir.join("master_flat_mean.tiff");
    let img: imaginarium::Image = master_flat.into();
    img.save_file(&flat_path)
        .expect("Failed to save master flat");
    println!("Saved master flat: {}", flat_path.display());
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
        "calibration_masters directory not found — run test_create_master_dark_and_flat first"
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
