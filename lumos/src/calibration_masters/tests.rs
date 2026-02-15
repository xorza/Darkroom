use crate::astro_image::cfa::{CfaImage, CfaType};
use crate::calibration_masters::DEFAULT_SIGMA_THRESHOLD;
use crate::calibration_masters::DefectMap;
use crate::common::Buffer2;
use crate::{AstroImageMetadata, CalibrationMasters};

/// Helper to create a CfaImage filled with a constant value.
fn constant_cfa(width: usize, height: usize, value: f32, cfa_type: CfaType) -> CfaImage {
    CfaImage {
        data: Buffer2::new_filled(width, height, value),
        metadata: AstroImageMetadata {
            cfa_type: Some(cfa_type),
            ..Default::default()
        },
    }
}

#[test]
fn test_new_constructor() {
    let dark = constant_cfa(4, 4, 0.1, CfaType::Mono);
    let flat = constant_cfa(4, 4, 0.8, CfaType::Mono);

    let masters = CalibrationMasters::from_images(
        Some(dark),
        Some(flat),
        None,
        None,
        DEFAULT_SIGMA_THRESHOLD,
    );

    assert!(masters.master_dark.is_some());
    assert!(masters.master_flat.is_some());
    assert!(masters.master_bias.is_none());
    // Defect map derived from dark
    assert!(masters.defect_map.is_some());
}

#[test]
fn test_new_no_dark_no_hot_pixels() {
    let flat = constant_cfa(4, 4, 0.8, CfaType::Mono);

    let masters =
        CalibrationMasters::from_images(None, Some(flat), None, None, DEFAULT_SIGMA_THRESHOLD);

    assert!(masters.master_dark.is_none());
    assert!(masters.master_flat.is_some());
    assert!(masters.defect_map.is_none());
}

#[test]
fn test_calibrate_dark_subtraction() {
    let dark = constant_cfa(4, 4, 0.1, CfaType::Mono);
    let masters =
        CalibrationMasters::from_images(Some(dark), None, None, None, DEFAULT_SIGMA_THRESHOLD);

    let mut light = constant_cfa(4, 4, 0.5, CfaType::Mono);
    masters.calibrate(&mut light);

    // 0.5 - 0.1 = 0.4
    for &v in &light.data {
        assert!((v - 0.4).abs() < 1e-6, "Expected 0.4, got {v}");
    }
}

#[test]
fn test_calibrate_bias_only() {
    // No dark → bias is subtracted instead
    let bias = constant_cfa(4, 4, 0.05, CfaType::Mono);
    let masters =
        CalibrationMasters::from_images(None, None, Some(bias), None, DEFAULT_SIGMA_THRESHOLD);

    let mut light = constant_cfa(4, 4, 0.5, CfaType::Mono);
    masters.calibrate(&mut light);

    // 0.5 - 0.05 = 0.45
    for &v in &light.data {
        assert!((v - 0.45).abs() < 1e-6, "Expected 0.45, got {v}");
    }
}

#[test]
fn test_calibrate_dark_takes_priority_over_bias() {
    // When both dark and bias exist, only dark is subtracted
    let dark = constant_cfa(4, 4, 0.1, CfaType::Mono);
    let bias = constant_cfa(4, 4, 0.05, CfaType::Mono);
    let masters = CalibrationMasters::from_images(
        Some(dark),
        None,
        Some(bias),
        None,
        DEFAULT_SIGMA_THRESHOLD,
    );

    let mut light = constant_cfa(4, 4, 0.5, CfaType::Mono);
    masters.calibrate(&mut light);

    // Dark subtracted: 0.5 - 0.1 = 0.4 (not 0.5 - 0.05)
    for &v in &light.data {
        assert!((v - 0.4).abs() < 1e-6, "Expected 0.4, got {v}");
    }
}

#[test]
fn test_calibrate_flat_correction() {
    // Flat with vignetting: [0.4, 0.8, 0.8, 0.4], mean = 0.6
    // normalized = [0.667, 1.333, 1.333, 0.667]
    // light = [0.3, 0.3, 0.3, 0.3]
    // result = light / normalized
    let flat = CfaImage {
        data: Buffer2::new(2, 2, vec![0.4, 0.8, 0.8, 0.4]),
        metadata: AstroImageMetadata {
            cfa_type: Some(CfaType::Mono),
            ..Default::default()
        },
    };

    let masters =
        CalibrationMasters::from_images(None, Some(flat), None, None, DEFAULT_SIGMA_THRESHOLD);

    let mut light = constant_cfa(2, 2, 0.3, CfaType::Mono);
    masters.calibrate(&mut light);

    // 0.3 / (0.4/0.6) = 0.3 / 0.6667 = 0.45
    assert!((light.data[0] - 0.45).abs() < 1e-4, "got {}", light.data[0]);
    // 0.3 / (0.8/0.6) = 0.3 / 1.3333 = 0.225
    assert!(
        (light.data[1] - 0.225).abs() < 1e-4,
        "got {}",
        light.data[1]
    );
}

#[test]
fn test_calibrate_full_pipeline() {
    // Full CFA calibration: dark subtraction + flat division
    // signal = [0.3, 0.6], vignetting = [0.8, 1.0]
    // bias = 0.05, thermal = 0.02, dark = 0.07
    // light = signal * vignetting + dark = [0.31, 0.67]
    // flat = K * vignetting + bias (K=0.8)
    //      = [0.8*0.8+0.05, 0.8*1.0+0.05] = [0.69, 0.85]
    let signal = [0.3_f32, 0.6];
    let vignetting = [0.8_f32, 1.0];
    let dark_val = 0.07_f32;
    let bias_val = 0.05_f32;
    let k = 0.8_f32;

    let light_pixels: Vec<f32> = signal
        .iter()
        .zip(&vignetting)
        .map(|(s, v)| s * v + dark_val)
        .collect();
    let flat_pixels: Vec<f32> = vignetting.iter().map(|v| k * v + bias_val).collect();

    let dark = constant_cfa(2, 1, dark_val, CfaType::Mono);
    let flat = CfaImage {
        data: Buffer2::new(2, 1, flat_pixels),
        metadata: AstroImageMetadata {
            cfa_type: Some(CfaType::Mono),
            ..Default::default()
        },
    };
    let bias = constant_cfa(2, 1, bias_val, CfaType::Mono);

    let masters = CalibrationMasters::from_images(
        Some(dark),
        Some(flat),
        Some(bias),
        None,
        DEFAULT_SIGMA_THRESHOLD,
    );

    let mut light = CfaImage {
        data: Buffer2::new(2, 1, light_pixels),
        metadata: AstroImageMetadata {
            cfa_type: Some(CfaType::Mono),
            ..Default::default()
        },
    };
    masters.calibrate(&mut light);

    // After dark subtraction: signal * vignetting
    // After flat division with bias: signal (vignetting cancelled)
    // mean(flat - bias) = mean(K * vignetting) = 0.8 * 0.9 = 0.72
    // normalized_flat[0] = (0.69 - 0.05) / 0.72 = 0.64 / 0.72 = 0.8889
    // normalized_flat[1] = (0.85 - 0.05) / 0.72 = 0.80 / 0.72 = 1.1111
    // After dark sub: [0.3*0.8, 0.6*1.0] = [0.24, 0.60]
    // After flat div: [0.24/0.8889, 0.60/1.1111] = [0.27, 0.54]
    // = signal * mean(K * vignetting) / K = signal * 0.72/0.8 = signal * 0.9
    let expected_0 = signal[0] * 0.9;
    let expected_1 = signal[1] * 0.9;
    assert!(
        (light.data[0] - expected_0).abs() < 1e-4,
        "Expected {expected_0}, got {}",
        light.data[0]
    );
    assert!(
        (light.data[1] - expected_1).abs() < 1e-4,
        "Expected {expected_1}, got {}",
        light.data[1]
    );
}

#[test]
fn test_sigma_threshold_affects_detection() {
    // 6×6 mono dark: background=100.0, one "warm" pixel at 250.0.
    // Per-color stats (mono, all same channel):
    //   median ≈ 100.0, MAD ≈ 0.0 → floored sigma = max(0.0, 100*0.1, 5e-4) = 10.0
    //   upper(sigma=3) = 100 + 3×10 = 130 → 250 > 130 → detected
    //   upper(sigma=20) = 100 + 20×10 = 300 → 250 < 300 → NOT detected
    let mut pixels = vec![100.0f32; 36];
    pixels[14] = 250.0; // warm pixel at index 14

    let dark_strict = constant_cfa(6, 6, 0.0, CfaType::Mono);
    let dark_loose = constant_cfa(6, 6, 0.0, CfaType::Mono);

    // Build actual CfaImages with our pixel data
    let dark_strict = CfaImage {
        data: Buffer2::new(6, 6, pixels.clone()),
        ..dark_strict
    };
    let dark_loose = CfaImage {
        data: Buffer2::new(6, 6, pixels),
        ..dark_loose
    };

    let masters_strict = CalibrationMasters::from_images(Some(dark_strict), None, None, None, 3.0);
    let masters_loose = CalibrationMasters::from_images(Some(dark_loose), None, None, None, 20.0);

    let strict_count = masters_strict.defect_map.as_ref().unwrap().hot_count();
    let loose_count = masters_loose.defect_map.as_ref().unwrap().hot_count();

    // sigma=3: 250 > 130, warm pixel detected as hot
    assert_eq!(strict_count, 1, "sigma=3 should detect the warm pixel");
    // sigma=20: 250 < 300, warm pixel NOT detected
    assert_eq!(loose_count, 0, "sigma=20 should not detect the warm pixel");
}

#[test]
fn test_defect_detection_zero_median_no_false_positives() {
    // Bias frames can have median=0. Without the absolute sigma floor,
    // every pixel > 0 would be flagged as hot.
    let mut data = vec![0.0f32; 100];
    // Add a few pixels with tiny values (normal bias noise)
    data[10] = 0.0001;
    data[20] = 0.0002;
    data[30] = 0.0001;
    // Add one genuine hot pixel
    data[50] = 0.5;

    let dark = CfaImage {
        data: Buffer2::new(10, 10, data),
        metadata: AstroImageMetadata {
            cfa_type: Some(CfaType::Mono),
            ..Default::default()
        },
    };

    let defect_map = DefectMap::from_master_dark(&dark, 5.0);

    // The tiny values should NOT be flagged as hot
    assert!(
        defect_map.hot_count() <= 1,
        "Expected at most 1 hot pixel (the 0.5 outlier), got {}",
        defect_map.hot_count()
    );
    // The genuine outlier at 0.5 should be detected
    assert!(
        defect_map.hot_indices.contains(&50),
        "Genuine hot pixel at index 50 should be detected"
    );
}

#[test]
fn test_calibrate_hot_pixel_correction() {
    // 6x6 Bayer dark with one hot pixel at (2,2)
    use crate::raw::demosaic::CfaPattern;

    let w = 6;
    let h = 6;
    let pattern = CfaType::Bayer(CfaPattern::Rggb);

    let mut dark_pixels = vec![0.01_f32; w * h];
    dark_pixels[2 * w + 2] = 0.9; // hot pixel at (2,2)

    let dark = CfaImage {
        data: Buffer2::new(w, h, dark_pixels),
        metadata: AstroImageMetadata {
            cfa_type: Some(pattern.clone()),
            ..Default::default()
        },
    };

    let masters =
        CalibrationMasters::from_images(Some(dark), None, None, None, DEFAULT_SIGMA_THRESHOLD);

    assert!(masters.defect_map.is_some());
    let hot_map = masters.defect_map.as_ref().unwrap();
    assert!(hot_map.count() >= 1, "Should detect the hot pixel");

    // Create light with corrupted hot pixel
    let mut light_pixels = vec![0.5_f32; w * h];
    light_pixels[2 * w + 2] = 0.99; // corrupted value at hot pixel location

    let mut light = CfaImage {
        data: Buffer2::new(w, h, light_pixels),
        metadata: AstroImageMetadata {
            cfa_type: Some(pattern),
            ..Default::default()
        },
    };
    masters.calibrate(&mut light);

    // After dark subtraction: normal pixels become ~0.49, hot pixel stays high
    // After hot pixel correction: replaced with median of same-color Bayer neighbors
    let corrected = light.data[2 * w + 2];
    assert!(
        (corrected - 0.49).abs() < 0.02,
        "Hot pixel should be corrected to ~0.49, got {corrected}"
    );
}

#[test]
fn test_calibrate_flat_dark() {
    // Flat dark is subtracted from flat instead of bias during normalization.
    // Simulates narrowband scenario: flat exposure accumulates dark current.
    //
    // Setup:
    //   signal = [0.3, 0.6], vignetting = [0.8, 1.0]
    //   dark = 0.07 (light dark), flat_dark = 0.03 (flat dark, shorter exposure)
    //   K = 0.8 (flat illumination level)
    //   light = signal * vignetting + dark
    //   flat = K * vignetting + flat_dark
    let signal = [0.3_f32, 0.6];
    let vignetting = [0.8_f32, 1.0];
    let dark_val = 0.07_f32;
    let flat_dark_val = 0.03_f32;
    let k = 0.8_f32;

    let light_pixels: Vec<f32> = signal
        .iter()
        .zip(&vignetting)
        .map(|(s, v)| s * v + dark_val)
        .collect();
    let flat_pixels: Vec<f32> = vignetting.iter().map(|v| k * v + flat_dark_val).collect();

    let dark = constant_cfa(2, 1, dark_val, CfaType::Mono);
    let flat = CfaImage {
        data: Buffer2::new(2, 1, flat_pixels),
        metadata: AstroImageMetadata {
            cfa_type: Some(CfaType::Mono),
            ..Default::default()
        },
    };
    let flat_dark = constant_cfa(2, 1, flat_dark_val, CfaType::Mono);

    let masters = CalibrationMasters::from_images(
        Some(dark),
        Some(flat),
        None,
        Some(flat_dark),
        DEFAULT_SIGMA_THRESHOLD,
    );

    let mut light = CfaImage {
        data: Buffer2::new(2, 1, light_pixels),
        metadata: AstroImageMetadata {
            cfa_type: Some(CfaType::Mono),
            ..Default::default()
        },
    };
    masters.calibrate(&mut light);

    // After dark subtraction: signal * vignetting = [0.24, 0.60]
    // flat - flat_dark = K * vignetting = [0.64, 0.80]
    // mean(flat - flat_dark) = 0.72
    // normalized_flat = [0.64/0.72, 0.80/0.72] = [0.8889, 1.1111]
    // result = [0.24/0.8889, 0.60/1.1111] = [0.27, 0.54] = signal * 0.9
    let scale = k * (vignetting[0] + vignetting[1]) / 2.0 / k; // mean(vignetting)/1 = 0.9
    let expected_0 = signal[0] * scale;
    let expected_1 = signal[1] * scale;
    assert!(
        (light.data[0] - expected_0).abs() < 1e-4,
        "Expected {expected_0}, got {}",
        light.data[0]
    );
    assert!(
        (light.data[1] - expected_1).abs() < 1e-4,
        "Expected {expected_1}, got {}",
        light.data[1]
    );
}

#[test]
fn test_flat_dark_takes_priority_over_bias() {
    // When both flat dark and bias exist, flat dark is used for flat normalization
    let flat_pixels = vec![0.8_f32, 0.6, 0.6, 0.8];
    let flat = CfaImage {
        data: Buffer2::new(2, 2, flat_pixels),
        metadata: AstroImageMetadata {
            cfa_type: Some(CfaType::Mono),
            ..Default::default()
        },
    };
    let bias = constant_cfa(2, 2, 0.05, CfaType::Mono);
    let flat_dark = constant_cfa(2, 2, 0.10, CfaType::Mono);

    let masters = CalibrationMasters::from_images(
        None,
        Some(flat),
        Some(bias),
        Some(flat_dark),
        DEFAULT_SIGMA_THRESHOLD,
    );

    let mut light = constant_cfa(2, 2, 0.5, CfaType::Mono);
    masters.calibrate(&mut light);

    // Bias subtracted from light: 0.5 - 0.05 = 0.45
    // flat - flat_dark = [0.7, 0.5, 0.5, 0.7], mean = 0.6
    // normalized = [1.1667, 0.8333, 0.8333, 1.1667]
    // If bias were used for flat instead: flat - bias = [0.75, 0.55, 0.55, 0.75], mean = 0.65
    // Verify flat dark is used for flat normalization (not bias)
    let expected_0 = 0.45 / (0.7 / 0.6);
    let expected_1 = 0.45 / (0.5 / 0.6);
    assert!(
        (light.data[0] - expected_0).abs() < 1e-4,
        "Expected {expected_0}, got {}",
        light.data[0]
    );
    assert!(
        (light.data[1] - expected_1).abs() < 1e-4,
        "Expected {expected_1}, got {}",
        light.data[1]
    );
}
