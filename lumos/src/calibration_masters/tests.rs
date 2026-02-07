use crate::stacking::{ProgressCallback, StackConfig};
use crate::{AstroImage, CalibrationMasters, FrameType, ImageDimensions};

/// Helper to create a grayscale AstroImage filled with a constant value.
fn constant_image(width: usize, height: usize, value: f32) -> AstroImage {
    let dims = ImageDimensions::new(width, height, 1);
    AstroImage::from_pixels(dims, vec![value; width * height])
}

#[test]
fn test_calibrate_with_raw_dark_and_bias() {
    // Dark is stored raw (bias + thermal). Subtracting it from light removes both.
    // light=1000, dark_raw=500 (bias=100, thermal=400)
    // Calibration: 1000 - 500 = 500 (correct signal)
    // Bias is present but ignored when dark exists (dark already contains bias).
    let mut light = constant_image(4, 4, 1000.0);
    let dark = constant_image(4, 4, 500.0); // raw dark = bias + thermal
    let bias = constant_image(4, 4, 100.0);

    let masters = CalibrationMasters {
        master_dark: Some(dark),
        master_flat: None,
        master_bias: Some(bias),
        hot_pixel_map: None,
        config: StackConfig::default(),
    };

    masters.calibrate(&mut light);

    // Expected: 1000 - 500 = 500
    for y in 0..4 {
        for x in 0..4 {
            let val = light.get_pixel_gray(x, y);
            assert!(
                (val - 500.0).abs() < 1e-4,
                "Expected 500.0 at ({x},{y}), got {val}"
            );
        }
    }
}

#[test]
fn test_calibrate_bias_only_no_dark() {
    // When no dark is available, bias is subtracted alone.
    // light=1000, bias=100 → 900
    let mut light = constant_image(4, 4, 1000.0);
    let bias = constant_image(4, 4, 100.0);

    let masters = CalibrationMasters {
        master_dark: None,
        master_flat: None,
        master_bias: Some(bias),
        hot_pixel_map: None,
        config: StackConfig::default(),
    };

    masters.calibrate(&mut light);

    for y in 0..4 {
        for x in 0..4 {
            let val = light.get_pixel_gray(x, y);
            assert!(
                (val - 900.0).abs() < 1e-4,
                "Expected 900.0 at ({x},{y}), got {val}"
            );
        }
    }
}

#[test]
fn test_calibrate_dark_only_no_bias() {
    // Dark without bias works the same — dark is always raw.
    // light=1000, dark=500 → 500
    let mut light = constant_image(4, 4, 1000.0);
    let dark = constant_image(4, 4, 500.0);

    let masters = CalibrationMasters {
        master_dark: Some(dark),
        master_flat: None,
        master_bias: None,
        hot_pixel_map: None,
        config: StackConfig::default(),
    };

    masters.calibrate(&mut light);

    for y in 0..4 {
        for x in 0..4 {
            let val = light.get_pixel_gray(x, y);
            assert!(
                (val - 500.0).abs() < 1e-4,
                "Expected 500.0 at ({x},{y}), got {val}"
            );
        }
    }
}

#[test]
#[cfg_attr(not(feature = "real-data"), ignore)]
fn test_calibration_masters_from_env() {
    use crate::testing::calibration_dir;

    let Some(cal_dir) = calibration_dir() else {
        return;
    };

    let masters = CalibrationMasters::create(
        &cal_dir,
        StackConfig::default(),
        ProgressCallback::default(),
    )
    .unwrap();

    // At least one master should be created
    assert!(
        masters.master_dark.is_some()
            || masters.master_flat.is_some()
            || masters.master_bias.is_some(),
        "No master frames created"
    );

    // Save to test output
    let output_dir = common::test_utils::test_output_path("calibration_masters");
    masters.save_to_directory(&output_dir).unwrap();

    // Verify files were created
    let config = StackConfig::default();
    if masters.master_dark.is_some() {
        assert!(CalibrationMasters::master_path(&output_dir, FrameType::Dark, &config).exists());
    }
    if masters.master_flat.is_some() {
        assert!(CalibrationMasters::master_path(&output_dir, FrameType::Flat, &config).exists());
    }
    if masters.master_bias.is_some() {
        assert!(CalibrationMasters::master_path(&output_dir, FrameType::Bias, &config).exists());
    }

    // Test loading saved masters
    let loaded = CalibrationMasters::load(&output_dir, StackConfig::default()).unwrap();
    assert_eq!(masters.master_dark.is_some(), loaded.master_dark.is_some());
    assert_eq!(masters.master_flat.is_some(), loaded.master_flat.is_some());
    assert_eq!(masters.master_bias.is_some(), loaded.master_bias.is_some());
}

#[test]
#[cfg_attr(not(feature = "real-data"), ignore)]
fn test_load_masters_from_calibration_masters_subdir() {
    use crate::testing::calibration_masters_dir;

    let Some(masters_dir) = calibration_masters_dir() else {
        return;
    };

    let config = StackConfig::default();
    let masters = CalibrationMasters::load(&masters_dir, config).unwrap();

    // Print what was found
    println!(
        "Loaded from calibration_masters: dark={}, flat={}, bias={}",
        masters.master_dark.is_some(),
        masters.master_flat.is_some(),
        masters.master_bias.is_some()
    );

    // At least one master should exist
    assert!(
        masters.master_dark.is_some()
            || masters.master_flat.is_some()
            || masters.master_bias.is_some(),
        "No master frames found in calibration_masters directory"
    );

    // Verify dimensions if dark exists
    if let Some(ref dark) = masters.master_dark {
        println!(
            "Master dark: {}x{}x{}",
            dark.width(),
            dark.height(),
            dark.channels()
        );
        assert!(dark.width() > 0);
        assert!(dark.height() > 0);
    }

    // Verify dimensions if flat exists
    if let Some(ref flat) = masters.master_flat {
        println!(
            "Master flat: {}x{}x{}",
            flat.width(),
            flat.height(),
            flat.channels()
        );
        assert!(flat.width() > 0);
        assert!(flat.height() > 0);
    }

    // Verify dimensions if bias exists
    if let Some(ref bias) = masters.master_bias {
        println!(
            "Master bias: {}x{}x{}",
            bias.width(),
            bias.height(),
            bias.channels()
        );
        assert!(bias.width() > 0);
        assert!(bias.height() > 0);
    }
}
