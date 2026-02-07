use crate::stacking::hot_pixels::HotPixelMap;
use crate::stacking::{ProgressCallback, StackConfig};
use crate::testing::{calibration_dir, calibration_masters_dir, init_tracing};
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

#[test]
fn test_calibrate_bias_subtraction() {
    let mut light = AstroImage::from_pixels(
        ImageDimensions::new(2, 2, 1),
        vec![100.0, 200.0, 150.0, 250.0],
    );
    let bias = AstroImage::from_pixels(ImageDimensions::new(2, 2, 1), vec![5.0, 5.0, 5.0, 5.0]);

    let masters = CalibrationMasters {
        master_dark: None,
        master_flat: None,
        master_bias: Some(bias),
        hot_pixel_map: None,
        config: StackConfig::default(),
    };

    masters.calibrate(&mut light);
    assert_eq!(light.channel(0).pixels(), &[95.0, 195.0, 145.0, 245.0]);
}

#[test]
fn test_calibrate_dark_subtraction() {
    let mut light = AstroImage::from_pixels(
        ImageDimensions::new(2, 2, 1),
        vec![100.0, 200.0, 150.0, 250.0],
    );
    let dark = AstroImage::from_pixels(ImageDimensions::new(2, 2, 1), vec![10.0, 20.0, 15.0, 25.0]);

    let masters = CalibrationMasters {
        master_dark: Some(dark),
        master_flat: None,
        master_bias: None,
        hot_pixel_map: None,
        config: StackConfig::default(),
    };

    masters.calibrate(&mut light);
    assert_eq!(light.channel(0).pixels(), &[90.0, 180.0, 135.0, 225.0]);
}

#[test]
fn test_calibrate_flat_correction() {
    let mut light = AstroImage::from_pixels(
        ImageDimensions::new(2, 2, 1),
        vec![100.0, 200.0, 150.0, 250.0],
    );
    let flat = AstroImage::from_pixels(ImageDimensions::new(2, 2, 1), vec![0.8, 1.0, 1.2, 1.0]);

    let masters = CalibrationMasters {
        master_dark: None,
        master_flat: Some(flat),
        master_bias: None,
        hot_pixel_map: None,
        config: StackConfig::default(),
    };

    masters.calibrate(&mut light);

    assert!((light.channel(0)[0] - 125.0).abs() < 0.01);
    assert!((light.channel(0)[1] - 200.0).abs() < 0.01);
    assert!((light.channel(0)[2] - 125.0).abs() < 0.01);
    assert!((light.channel(0)[3] - 250.0).abs() < 0.01);
}

#[test]
fn test_calibrate_full() {
    // Realistic calibration scenario:
    //   bias = 100 (readout offset)
    //   thermal = 20 (dark current)
    //   signal per pixel = [500, 1000, 700, 1200]
    //   vignetting per pixel = [0.8, 1.0, 1.2, 1.0]
    //
    // Raw frames:
    //   light = signal * vignetting + bias + thermal
    //   dark = bias + thermal = 120
    //   flat = K * vignetting + bias (K=1000 for flat illumination level)
    //   bias = 100
    //
    // Calibration formula: (L - D) / ((F - O) / mean(F - O))
    //   L - D = signal * vignetting
    //   F - O = K * vignetting = [800, 1000, 1200, 1000]
    //   mean(F - O) = 1000
    //   normalized flat = [0.8, 1.0, 1.2, 1.0]
    //   result = signal * vignetting / (K * vignetting / mean(K * vignetting))
    //          = signal * mean(K * vignetting) / K = signal
    let light_vals: Vec<f32> = [500.0, 1000.0, 700.0, 1200.0]
        .iter()
        .zip([0.8, 1.0, 1.2, 1.0].iter())
        .map(|(s, v)| s * v + 120.0) // signal*vignetting + bias + thermal
        .collect();
    let mut light = AstroImage::from_pixels(ImageDimensions::new(2, 2, 1), light_vals);

    let bias = AstroImage::from_pixels(ImageDimensions::new(2, 2, 1), vec![100.0; 4]);
    let dark = AstroImage::from_pixels(ImageDimensions::new(2, 2, 1), vec![120.0; 4]);
    // flat = K * vignetting + bias = 1000 * [0.8, 1.0, 1.2, 1.0] + 100
    let flat = AstroImage::from_pixels(
        ImageDimensions::new(2, 2, 1),
        vec![900.0, 1100.0, 1300.0, 1100.0],
    );

    let masters = CalibrationMasters {
        master_dark: Some(dark),
        master_flat: Some(flat),
        master_bias: Some(bias),
        hot_pixel_map: None,
        config: StackConfig::default(),
    };

    masters.calibrate(&mut light);

    // Result should be pure signal (vignetting perfectly cancelled)
    assert!(
        (light.channel(0)[0] - 500.0).abs() < 0.1,
        "got {}",
        light.channel(0)[0]
    );
    assert!(
        (light.channel(0)[1] - 1000.0).abs() < 0.1,
        "got {}",
        light.channel(0)[1]
    );
    assert!(
        (light.channel(0)[2] - 700.0).abs() < 0.1,
        "got {}",
        light.channel(0)[2]
    );
    assert!(
        (light.channel(0)[3] - 1200.0).abs() < 0.1,
        "got {}",
        light.channel(0)[3]
    );
}

#[test]
fn test_calibrate_rgb_with_rgb_masters() {
    let mut light = AstroImage::from_pixels(
        ImageDimensions::new(2, 2, 3),
        vec![
            100.0, 100.0, 100.0, 200.0, 200.0, 200.0, 150.0, 150.0, 150.0, 250.0, 250.0, 250.0,
        ],
    );

    let bias = AstroImage::from_pixels(ImageDimensions::new(2, 2, 3), vec![5.0; 12]);

    let masters = CalibrationMasters {
        master_dark: None,
        master_flat: None,
        master_bias: Some(bias),
        hot_pixel_map: None,
        config: StackConfig::default(),
    };

    masters.calibrate(&mut light);

    assert_eq!(light.channel(0)[0], 95.0);
    assert_eq!(light.channel(1)[0], 95.0);
    assert_eq!(light.channel(2)[0], 95.0);
    assert_eq!(light.channel(0)[1], 195.0);
}

#[test]
#[should_panic(expected = "don't match")]
fn test_calibrate_rgb_with_grayscale_bias_panics() {
    let mut light = AstroImage::from_pixels(ImageDimensions::new(2, 2, 3), vec![100.0; 12]);
    let bias = AstroImage::from_pixels(ImageDimensions::new(2, 2, 1), vec![5.0; 4]);

    let masters = CalibrationMasters {
        master_dark: None,
        master_flat: None,
        master_bias: Some(bias),
        hot_pixel_map: None,
        config: StackConfig::default(),
    };

    masters.calibrate(&mut light);
}

#[test]
#[should_panic(expected = "don't match")]
fn test_calibrate_grayscale_with_rgb_dark_panics() {
    let mut light = AstroImage::from_pixels(ImageDimensions::new(2, 2, 1), vec![100.0; 4]);
    let dark = AstroImage::from_pixels(ImageDimensions::new(2, 2, 3), vec![10.0; 12]);

    let masters = CalibrationMasters {
        master_dark: Some(dark),
        master_flat: None,
        master_bias: None,
        hot_pixel_map: None,
        config: StackConfig::default(),
    };

    masters.calibrate(&mut light);
}

#[test]
#[cfg_attr(not(feature = "real-data"), ignore)]
fn test_calibrate_light_from_env() {
    init_tracing();

    let Some(cal_dir) = calibration_dir() else {
        eprintln!("LUMOS_CALIBRATION_DIR not set, skipping test");
        return;
    };

    let lights_dir = cal_dir.join("Lights");
    if !lights_dir.exists() {
        eprintln!("Lights directory not found, skipping test");
        return;
    }

    let files = common::file_utils::astro_image_files(&lights_dir);
    let Some(first_file) = files.first() else {
        eprintln!("No image files in Lights, skipping test");
        return;
    };

    let start = std::time::Instant::now();
    println!("Loading light frame: {:?}", first_file);
    let mut light = AstroImage::from_file(first_file).expect("Failed to load light frame");
    println!("  Load light: {:?}", start.elapsed());

    let original_dimensions = light.dimensions();

    println!(
        "Loaded light frame: {}x{}x{}",
        light.width(),
        light.height(),
        light.channels()
    );

    let Some(masters_dir) = calibration_masters_dir() else {
        eprintln!("calibration_masters directory not found, skipping test");
        return;
    };

    let start = std::time::Instant::now();
    let masters = CalibrationMasters::load(&masters_dir, StackConfig::default()).unwrap();
    println!("  Load masters: {:?}", start.elapsed());

    println!(
        "Loaded masters: dark={}, flat={}, bias={}",
        masters.master_dark.is_some(),
        masters.master_flat.is_some(),
        masters.master_bias.is_some()
    );

    if let Some(ref hot_map) = masters.hot_pixel_map {
        println!(
            "  Hot pixels: {} ({:.4}%)",
            hot_map.count,
            hot_map.percentage()
        );
    }

    let start = std::time::Instant::now();
    masters.calibrate(&mut light);
    println!("  Calibrate: {:?}", start.elapsed());

    println!(
        "Calibrated frame: {}x{}x{}",
        light.width(),
        light.height(),
        light.channels()
    );
    println!("Mean: {}", light.mean());

    assert_eq!(light.dimensions(), original_dimensions);

    let start = std::time::Instant::now();
    let output_path = common::test_utils::test_output_path("calibrated_light.tiff");
    let img: imaginarium::Image = light.into();
    img.save_file(&output_path).unwrap();
    println!("  Save: {:?}", start.elapsed());

    println!("Saved calibrated image to: {:?}", output_path);
    assert!(output_path.exists());
}

#[test]
fn test_calibrate_flat_with_bias() {
    // Flat + bias correction (no dark):
    //   1. Bias subtracted from light: L' = L - O
    //   2. Flat normalized with bias:  (F - O) / mean(F - O)
    //   3. Result: L' / normalized_flat
    //
    // signal = [500, 1000, 600, 1000], vignetting = [0.8, 1.0, 1.2, 1.0], bias = 100
    // light = signal * vignetting + bias = [500, 1100, 820, 1100]
    // flat = K * vignetting + bias = 1000 * v + 100 = [900, 1100, 1300, 1100]
    //
    // Step 1: L' = L - O = [400, 1000, 720, 1000]  (= signal * vignetting)
    // Step 2: F - O = [800, 1000, 1200, 1000], mean = 1000
    //         normalized = [0.8, 1.0, 1.2, 1.0]
    // Step 3: L' / normalized = [500, 1000, 600, 1000] = signal
    let signal = [500.0_f32, 1000.0, 600.0, 1000.0];
    let vignetting = [0.8_f32, 1.0, 1.2, 1.0];
    let bias_val = 100.0_f32;

    let light_vals: Vec<f32> = signal
        .iter()
        .zip(&vignetting)
        .map(|(s, v)| s * v + bias_val)
        .collect();
    let mut light = AstroImage::from_pixels(ImageDimensions::new(2, 2, 1), light_vals);

    let flat_vals: Vec<f32> = vignetting.iter().map(|v| 1000.0 * v + bias_val).collect();
    let flat = AstroImage::from_pixels(ImageDimensions::new(2, 2, 1), flat_vals);
    let bias = AstroImage::from_pixels(ImageDimensions::new(2, 2, 1), vec![bias_val; 4]);

    let masters = CalibrationMasters {
        master_dark: None,
        master_flat: Some(flat),
        master_bias: Some(bias),
        hot_pixel_map: None,
        config: StackConfig::default(),
    };

    masters.calibrate(&mut light);

    let ch = light.channel(0).pixels();
    for (i, &expected) in signal.iter().enumerate() {
        assert!(
            (ch[i] - expected).abs() < 0.1,
            "pixel {i}: expected {expected}, got {}",
            ch[i]
        );
    }
}

#[test]
fn test_calibrate_full_rgb() {
    // Full RGB calibration: (L - D) / ((F - O) / mean(F - O))
    //
    // Per-channel signals: R=[500,800,600,1000], G=[600,900,700,1100], B=[400,700,500,900]
    // Vignetting: [0.8, 1.0, 1.2, 1.0] (mean = 1.0)
    // bias=100, thermal=20, dark=120
    // flat illumination K=1000
    //
    // light_c = signal_c * vignetting + bias + thermal
    // dark = bias + thermal = 120 (raw, contains bias)
    // flat_c = K * vignetting + bias
    //
    // Result: signal_c * mean(vignetting) = signal_c (since mean(v) = 1.0)
    let dims = ImageDimensions::new(2, 2, 3);
    let vignetting = [0.8_f32, 1.0, 1.2, 1.0];
    let bias_val = 100.0_f32;
    let thermal = 20.0_f32;
    let dark_val = bias_val + thermal;
    let k = 1000.0_f32;

    let r_signal = [500.0_f32, 800.0, 600.0, 1000.0];
    let g_signal = [600.0_f32, 900.0, 700.0, 1100.0];
    let b_signal = [400.0_f32, 700.0, 500.0, 900.0];

    // from_pixels expects interleaved RGB: [R0,G0,B0, R1,G1,B1, ...]
    let mut light_pixels = Vec::with_capacity(12);
    for i in 0..4 {
        let v = vignetting[i];
        light_pixels.push(r_signal[i] * v + dark_val);
        light_pixels.push(g_signal[i] * v + dark_val);
        light_pixels.push(b_signal[i] * v + dark_val);
    }
    let mut light = AstroImage::from_pixels(dims, light_pixels);

    let dark = AstroImage::from_pixels(dims, vec![dark_val; 12]);
    let bias = AstroImage::from_pixels(dims, vec![bias_val; 12]);
    // flat = K * vignetting + bias, same for all channels (interleaved)
    let mut flat_vals = Vec::with_capacity(12);
    for &v in &vignetting {
        let fv = k * v + bias_val;
        flat_vals.extend_from_slice(&[fv, fv, fv]);
    }
    let flat = AstroImage::from_pixels(dims, flat_vals);

    let masters = CalibrationMasters {
        master_dark: Some(dark),
        master_flat: Some(flat),
        master_bias: Some(bias),
        hot_pixel_map: None,
        config: StackConfig::default(),
    };

    masters.calibrate(&mut light);

    // Vignetting perfectly cancelled (mean(v)=1.0), so result = signal
    let r = light.channel(0).pixels();
    for (i, &expected) in r_signal.iter().enumerate() {
        assert!((r[i] - expected).abs() < 0.5, "R[{i}]: got {}", r[i]);
    }

    let g = light.channel(1).pixels();
    for (i, &expected) in g_signal.iter().enumerate() {
        assert!((g[i] - expected).abs() < 0.5, "G[{i}]: got {}", g[i]);
    }

    let b = light.channel(2).pixels();
    for (i, &expected) in b_signal.iter().enumerate() {
        assert!((b[i] - expected).abs() < 0.5, "B[{i}]: got {}", b[i]);
    }
}

#[test]
fn test_calibrate_hot_pixel_correction() {
    // Create a 5x5 dark frame with one extreme hot pixel at (2,2) = index 12.
    // Background ~100, hot pixel = 50000 (clearly >5σ above).
    let dims = ImageDimensions::new(5, 5, 1);
    let mut dark_pixels = vec![100.0_f32; 25];
    dark_pixels[12] = 50000.0; // hot pixel at center

    let dark = AstroImage::from_pixels(dims, dark_pixels);
    let hot_pixel_map = HotPixelMap::from_master_dark(&dark, 5.0);
    assert!(
        hot_pixel_map.count >= 1,
        "Should detect at least 1 hot pixel"
    );

    // Create a light frame. Put a known bad value at the hot pixel location.
    // Neighbors of (2,2): (1,1)=6, (2,1)=7, (3,1)=8, (1,2)=11, (3,2)=13,
    //                     (1,3)=16, (2,3)=17, (3,3)=18
    let mut light_pixels = vec![500.0_f32; 25];
    light_pixels[12] = 99999.0; // corrupted hot pixel in light

    let mut light = AstroImage::from_pixels(dims, light_pixels);

    let masters = CalibrationMasters {
        master_dark: Some(dark),
        master_flat: None,
        master_bias: None,
        hot_pixel_map: Some(hot_pixel_map),
        config: StackConfig::default(),
    };

    masters.calibrate(&mut light);

    // After dark subtraction: most pixels become 400, hot pixel becomes 99999-50000=49999
    // After hot pixel correction: hot pixel replaced by median of 8 neighbors = 400
    let hot_val = light.channel(0)[12];
    assert!(
        (hot_val - 400.0).abs() < 1.0,
        "Hot pixel should be corrected to ~400 (median of neighbors), got {hot_val}"
    );

    // Verify non-hot pixels are just dark-subtracted
    assert!(
        (light.channel(0)[0] - 400.0).abs() < 1e-4,
        "Non-hot pixel should be 500-100=400"
    );
}
