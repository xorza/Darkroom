use crate::io::astro_image::cfa::{CfaImage, CfaType};
use crate::io::raw::demosaic::bayer::CfaPattern;
use crate::stacking::calibration_masters::defect_map::DefectMap;
use crate::stacking::calibration_masters::weighted_budget;
use crate::stacking::calibration_masters::{
    CACHE_MAGIC, CACHE_VERSION, CalibrationError, DEFAULT_SIGMA_THRESHOLD,
};
use crate::stacking::combine::error::Error;
use crate::testing::constant_cfa;
use crate::{
    AstroImageMetadata, CalibrationComponent, CalibrationMasters, CalibrationSet, DefectSummary,
};
use common::CancelToken;
use imaginarium::Buffer2;

#[test]
fn weighted_budget_never_overcommits() {
    // The frame-weighted split is the memory-safety guarantee for concurrent role loading: the
    // per-role shares must sum to at most the whole budget, a bigger role must get a bigger share,
    // an empty role nothing, and a degenerate total the whole budget (no divide-by-zero).
    let avail = 30_000_000_000u64;
    let counts = [15usize, 15, 20, 0];
    let total: usize = counts.iter().sum();

    let sum: u64 = counts
        .iter()
        .map(|&n| weighted_budget(avail, n, total))
        .sum();
    assert!(sum <= avail, "weighted shares overcommit: {sum} > {avail}");

    assert!(
        weighted_budget(avail, 20, total) > weighted_budget(avail, 15, total),
        "more frames → larger share"
    );
    assert_eq!(
        weighted_budget(avail, 0, total),
        0,
        "empty role gets nothing"
    );
    assert_eq!(
        weighted_budget(avail, 5, 0),
        avail,
        "degenerate total → whole budget"
    );
}

#[test]
#[should_panic(expected = "already-calibrated frame")]
fn test_calibrate_twice_panics() {
    // A second calibrate() would subtract the dark / divide the flat twice — must crash, not
    // silently corrupt.
    let masters =
        CalibrationMasters::from_images(CalibrationSet::default(), 5.0, CancelToken::never())
            .unwrap();
    let mut light = constant_cfa(4, 4, 0.5, CfaType::Mono);
    masters.calibrate(&mut light).unwrap();
    assert!(light.metadata.calibrated);
    masters.calibrate(&mut light).unwrap();
}

fn masters_with_component(
    component: CalibrationComponent,
    cfa_type: Option<CfaType>,
) -> CalibrationMasters {
    let mut master = constant_cfa(2, 2, 1.0, CfaType::Mono);
    master.metadata.cfa_type = cfa_type;
    let mut images = CalibrationSet::default();
    match component {
        CalibrationComponent::Dark => images.dark = Some(master),
        CalibrationComponent::Flat => images.flat = Some(master),
        CalibrationComponent::Bias => images.bias = Some(master),
        CalibrationComponent::FlatDark => images.flat_dark = Some(master),
        CalibrationComponent::Defects => panic!("defect maps do not carry CFA metadata"),
    }
    CalibrationMasters::from_images(images, DEFAULT_SIGMA_THRESHOLD, CancelToken::never()).unwrap()
}

#[test]
fn calibrate_rejects_missing_and_mismatched_cfa_before_mutation() {
    #[derive(Debug)]
    struct Case {
        component: CalibrationComponent,
        light: Option<CfaType>,
        master: Option<CfaType>,
        expected: CalibrationError,
    }

    let xtrans_a = CfaType::XTrans([[0; 6]; 6]);
    let mut xtrans_b_pattern = [[0; 6]; 6];
    xtrans_b_pattern[0][0] = 1;
    let xtrans_b = CfaType::XTrans(xtrans_b_pattern);
    let cases = [
        Case {
            component: CalibrationComponent::Flat,
            light: None,
            master: Some(CfaType::Mono),
            expected: CalibrationError::MissingLightCfaPattern,
        },
        Case {
            component: CalibrationComponent::Flat,
            light: Some(CfaType::Mono),
            master: None,
            expected: CalibrationError::MissingMasterCfaPattern {
                component: CalibrationComponent::Flat,
            },
        },
        Case {
            component: CalibrationComponent::Dark,
            light: Some(CfaType::Mono),
            master: Some(CfaType::Bayer(CfaPattern::Rggb)),
            expected: CalibrationError::CfaPatternMismatch {
                component: CalibrationComponent::Dark,
                light: CfaType::Mono,
                master: CfaType::Bayer(CfaPattern::Rggb),
            },
        },
        Case {
            component: CalibrationComponent::Flat,
            light: Some(CfaType::Bayer(CfaPattern::Rggb)),
            master: Some(CfaType::Bayer(CfaPattern::Bggr)),
            expected: CalibrationError::CfaPatternMismatch {
                component: CalibrationComponent::Flat,
                light: CfaType::Bayer(CfaPattern::Rggb),
                master: CfaType::Bayer(CfaPattern::Bggr),
            },
        },
        Case {
            component: CalibrationComponent::Bias,
            light: Some(CfaType::Bayer(CfaPattern::Rggb)),
            master: Some(xtrans_a.clone()),
            expected: CalibrationError::CfaPatternMismatch {
                component: CalibrationComponent::Bias,
                light: CfaType::Bayer(CfaPattern::Rggb),
                master: xtrans_a.clone(),
            },
        },
        Case {
            component: CalibrationComponent::FlatDark,
            light: Some(xtrans_a.clone()),
            master: Some(xtrans_b.clone()),
            expected: CalibrationError::CfaPatternMismatch {
                component: CalibrationComponent::FlatDark,
                light: xtrans_a,
                master: xtrans_b,
            },
        },
    ];

    for case in cases {
        let masters = masters_with_component(case.component, case.master);
        let mut light = constant_cfa(2, 2, 0.5, CfaType::Mono);
        light.metadata.cfa_type = case.light.clone();
        let original_data = light.data.to_vec();

        assert_eq!(masters.calibrate(&mut light), Err(case.expected));
        assert_eq!(light.data.pixels(), original_data);
        assert_eq!(light.metadata.cfa_type, case.light);
        assert!(!light.metadata.calibrated);
    }
}

#[test]
fn test_from_files_all_empty_yields_no_masters() {
    // Empty frame sets must produce a `None` for every master (no file I/O path).
    let empty: Vec<std::path::PathBuf> = Vec::new();
    let masters = CalibrationMasters::from_files(
        CalibrationSet {
            dark: &empty,
            flat: &empty,
            bias: &empty,
            flat_dark: &empty,
        },
        DEFAULT_SIGMA_THRESHOLD,
    )
    .unwrap();

    assert_eq!(masters.components().collect::<Vec<_>>(), Vec::new());
    assert_eq!(masters.defect_summary(), None);
}

#[test]
fn test_new_constructor() {
    let dark = constant_cfa(4, 4, 0.1, CfaType::Mono);
    let flat = constant_cfa(4, 4, 0.8, CfaType::Mono);
    let bias = constant_cfa(4, 4, 0.02, CfaType::Mono);
    let flat_dark = constant_cfa(4, 4, 0.03, CfaType::Mono);

    let masters = CalibrationMasters::from_images(
        CalibrationSet {
            dark: Some(dark),
            flat: Some(flat),
            bias: Some(bias),
            flat_dark: Some(flat_dark),
        },
        DEFAULT_SIGMA_THRESHOLD,
        CancelToken::never(),
    )
    .unwrap();

    assert_eq!(
        masters.components().collect::<Vec<_>>(),
        vec![
            CalibrationComponent::Dark,
            CalibrationComponent::Flat,
            CalibrationComponent::Bias,
            CalibrationComponent::FlatDark,
            CalibrationComponent::Defects,
        ]
    );
    assert_eq!(
        masters
            .components()
            .map(|component| component.to_string())
            .collect::<Vec<_>>(),
        vec!["dark", "flat", "bias", "flat-dark", "defects"]
    );
    assert_eq!(
        masters.defect_summary(),
        Some(DefectSummary {
            hot_pixels: 0,
            cold_pixels: 0,
            percentage: 0.0,
        })
    );
}

#[test]
fn test_from_images_rejects_cancelled_operation() {
    let cancel = CancelToken::new();
    cancel.cancel();

    let result = CalibrationMasters::from_images(
        CalibrationSet {
            dark: Some(constant_cfa(4, 4, 0.1, CfaType::Mono)),
            ..Default::default()
        },
        DEFAULT_SIGMA_THRESHOLD,
        cancel,
    );

    assert!(matches!(result, Err(Error::Cancelled)));
}

#[test]
fn test_new_no_dark_no_hot_pixels() {
    let flat = constant_cfa(4, 4, 0.8, CfaType::Mono);

    let masters = CalibrationMasters::from_images(
        CalibrationSet {
            flat: Some(flat),
            ..Default::default()
        },
        DEFAULT_SIGMA_THRESHOLD,
        CancelToken::never(),
    )
    .unwrap();

    assert_eq!(
        masters.components().collect::<Vec<_>>(),
        vec![CalibrationComponent::Flat, CalibrationComponent::Defects]
    );
    assert_eq!(
        masters.defect_summary(),
        Some(DefectSummary {
            hot_pixels: 0,
            cold_pixels: 0,
            percentage: 0.0,
        })
    );
}

#[test]
fn test_calibrate_dark_subtraction() {
    let dark = constant_cfa(4, 4, 0.1, CfaType::Mono);
    let masters = CalibrationMasters::from_images(
        CalibrationSet {
            dark: Some(dark),
            ..Default::default()
        },
        DEFAULT_SIGMA_THRESHOLD,
        CancelToken::never(),
    )
    .unwrap();

    let mut light = constant_cfa(4, 4, 0.5, CfaType::Mono);
    masters.calibrate(&mut light).unwrap();

    // 0.5 - 0.1 = 0.4
    for &v in &light.data {
        assert!((v - 0.4).abs() < 1e-6, "Expected 0.4, got {v}");
    }
}

#[test]
fn test_calibrate_bias_only() {
    // No dark → bias is subtracted instead
    let bias = constant_cfa(4, 4, 0.05, CfaType::Mono);
    let masters = CalibrationMasters::from_images(
        CalibrationSet {
            bias: Some(bias),
            ..Default::default()
        },
        DEFAULT_SIGMA_THRESHOLD,
        CancelToken::never(),
    )
    .unwrap();

    let mut light = constant_cfa(4, 4, 0.5, CfaType::Mono);
    masters.calibrate(&mut light).unwrap();

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
        CalibrationSet {
            dark: Some(dark),
            bias: Some(bias),
            ..Default::default()
        },
        DEFAULT_SIGMA_THRESHOLD,
        CancelToken::never(),
    )
    .unwrap();

    let mut light = constant_cfa(4, 4, 0.5, CfaType::Mono);
    masters.calibrate(&mut light).unwrap();

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

    let masters = CalibrationMasters::from_images(
        CalibrationSet {
            flat: Some(flat),
            ..Default::default()
        },
        DEFAULT_SIGMA_THRESHOLD,
        CancelToken::never(),
    )
    .unwrap();

    let mut light = constant_cfa(2, 2, 0.3, CfaType::Mono);
    masters.calibrate(&mut light).unwrap();

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
        CalibrationSet {
            dark: Some(dark),
            flat: Some(flat),
            bias: Some(bias),
            ..Default::default()
        },
        DEFAULT_SIGMA_THRESHOLD,
        CancelToken::never(),
    )
    .unwrap();

    let mut light = CfaImage {
        data: Buffer2::new(2, 1, light_pixels),
        metadata: AstroImageMetadata {
            cfa_type: Some(CfaType::Mono),
            ..Default::default()
        },
    };
    masters.calibrate(&mut light).unwrap();

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
    // 6×6 mono dark with *real* noise so σ genuinely scales the threshold: 18 px at 90, 18 at 110
    // (one of the 110s replaced by a warm 400). Per-color stats (mono): median ≈ 100, MAD ≈ 10 →
    // sigma ≈ 10·1.4826 ≈ 14.8.
    //   sigma=3:  threshold ≈ 100 + 3·14.8  ≈ 145 → 400 > 145 (only the warm px) → 1 detected
    //   sigma=40: threshold ≈ 100 + 40·14.8 ≈ 693 → 400 < 693                    → 0 detected
    // (No relative σ floor: detectability scales with the real noise, not a fraction of the median.)
    let mut pixels: Vec<f32> = (0..36)
        .map(|i| if i % 2 == 0 { 90.0 } else { 110.0 })
        .collect();
    pixels[15] = 400.0; // index 15 is odd → was 110

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

    let masters_strict = CalibrationMasters::from_images(
        CalibrationSet {
            dark: Some(dark_strict),
            ..Default::default()
        },
        3.0,
        CancelToken::never(),
    )
    .unwrap();
    let masters_loose = CalibrationMasters::from_images(
        CalibrationSet {
            dark: Some(dark_loose),
            ..Default::default()
        },
        40.0,
        CancelToken::never(),
    )
    .unwrap();

    let strict_count = masters_strict.defect_summary().unwrap().hot_pixels;
    let loose_count = masters_loose.defect_summary().unwrap().hot_pixels;

    // sigma=3: 1000 > 199, the very-hot pixel is detected
    assert_eq!(strict_count, 1, "sigma=3 should detect the hot pixel");
    // sigma=40: 1000 < 1296, the hot pixel is below threshold
    assert_eq!(loose_count, 0, "sigma=40 should not detect the hot pixel");
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

    let defect_map = DefectMap::default()
        .detect_hot(&dark, 5.0, &CancelToken::never())
        .unwrap();

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
    use crate::io::raw::demosaic::bayer::CfaPattern;

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

    let masters = CalibrationMasters::from_images(
        CalibrationSet {
            dark: Some(dark),
            ..Default::default()
        },
        DEFAULT_SIGMA_THRESHOLD,
        CancelToken::never(),
    )
    .unwrap();

    assert_eq!(
        masters.defect_summary(),
        Some(DefectSummary {
            hot_pixels: 1,
            cold_pixels: 0,
            percentage: 100.0 / (w * h) as f32,
        })
    );

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
    masters.calibrate(&mut light).unwrap();

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
        CalibrationSet {
            dark: Some(dark),
            flat: Some(flat),
            flat_dark: Some(flat_dark),
            ..Default::default()
        },
        DEFAULT_SIGMA_THRESHOLD,
        CancelToken::never(),
    )
    .unwrap();

    let mut light = CfaImage {
        data: Buffer2::new(2, 1, light_pixels),
        metadata: AstroImageMetadata {
            cfa_type: Some(CfaType::Mono),
            ..Default::default()
        },
    };
    masters.calibrate(&mut light).unwrap();

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
        CalibrationSet {
            flat: Some(flat),
            bias: Some(bias),
            flat_dark: Some(flat_dark),
            ..Default::default()
        },
        DEFAULT_SIGMA_THRESHOLD,
        CancelToken::never(),
    )
    .unwrap();

    let mut light = constant_cfa(2, 2, 0.5, CfaType::Mono);
    masters.calibrate(&mut light).unwrap();

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

#[test]
fn prepared_master_cache_round_trips_flat_and_calibration_bit_exactly() {
    let cfa_type = CfaType::Bayer(CfaPattern::Rggb);
    let flat = CfaImage {
        data: Buffer2::new(
            4,
            4,
            vec![
                0.5, 0.7, 0.9, 0.7, 0.7, 0.4, 0.7, 0.4, 0.9, 0.7, 0.5, 0.7, 0.7, 0.4, 0.7, 0.4,
            ],
        ),
        metadata: AstroImageMetadata {
            cfa_type: Some(cfa_type.clone()),
            camera_white_balance: Some([2.0, 1.0, 1.5, 1.0]),
            ..Default::default()
        },
    };
    let masters = CalibrationMasters::from_images(
        CalibrationSet {
            dark: Some(constant_cfa(4, 4, 0.05, cfa_type.clone())),
            flat: Some(flat),
            bias: Some(constant_cfa(4, 4, 0.1, cfa_type.clone())),
            flat_dark: None,
        },
        DEFAULT_SIGMA_THRESHOLD,
        CancelToken::never(),
    )
    .unwrap();
    let prepared_bits = masters
        .flat
        .as_ref()
        .unwrap()
        .data
        .iter()
        .map(|value| value.to_bits())
        .collect::<Vec<_>>();

    let mut expected = constant_cfa(4, 4, 0.75, cfa_type.clone());
    masters.calibrate(&mut expected).unwrap();

    let cache_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".tmp");
    std::fs::create_dir_all(&cache_dir).unwrap();
    let path = cache_dir.join(format!(
        "calibration_masters_roundtrip_{}.lcm",
        std::process::id()
    ));
    masters.save(&path).unwrap();
    let mut cache_bytes = std::fs::read(&path).unwrap();
    let loaded = CalibrationMasters::load(&path).unwrap();

    cache_bytes[0] ^= 0xff;
    std::fs::write(&path, &cache_bytes).unwrap();
    assert_eq!(
        CalibrationMasters::load(&path).unwrap_err().kind(),
        std::io::ErrorKind::InvalidData
    );
    cache_bytes[0] ^= 0xff;
    cache_bytes[CACHE_MAGIC.len()..CACHE_MAGIC.len() + size_of::<u32>()]
        .copy_from_slice(&(CACHE_VERSION + 1).to_le_bytes());
    std::fs::write(&path, cache_bytes).unwrap();
    assert_eq!(
        CalibrationMasters::load(&path).unwrap_err().kind(),
        std::io::ErrorKind::InvalidData
    );
    std::fs::remove_file(path).unwrap();

    assert_eq!(
        loaded
            .flat
            .as_ref()
            .unwrap()
            .data
            .iter()
            .map(|value| value.to_bits())
            .collect::<Vec<_>>(),
        prepared_bits
    );
    assert_eq!(
        loaded.flat.as_ref().unwrap().metadata.camera_white_balance,
        Some([2.0, 1.0, 1.5, 1.0])
    );
    assert_eq!(
        loaded.components().collect::<Vec<_>>(),
        masters.components().collect::<Vec<_>>()
    );
    assert_eq!(loaded.defect_summary(), masters.defect_summary());

    let mut actual = constant_cfa(4, 4, 0.75, cfa_type);
    loaded.calibrate(&mut actual).unwrap();
    assert_eq!(
        actual
            .data
            .iter()
            .map(|value| value.to_bits())
            .collect::<Vec<_>>(),
        expected
            .data
            .iter()
            .map(|value| value.to_bits())
            .collect::<Vec<_>>()
    );
}

#[test]
fn ram_bytes_sums_present_frames_and_defects() {
    // A 10×8 mono CFA frame holds 80 f32 pixels = 320 bytes.
    let dark = constant_cfa(10, 8, 0.1, CfaType::Mono);
    assert_eq!(dark.ram_bytes(), 10 * 8 * 4);

    // A defect map counts only its hot + cold index lists (3 usize = 24 bytes on
    // a 64-bit target); an empty/default map is zero.
    let mut defects = DefectMap::default();
    assert_eq!(defects.ram_bytes(), 0);
    defects.hot_indices = vec![1, 2];
    defects.cold_indices = vec![7];
    assert_eq!(defects.ram_bytes(), 3 * std::mem::size_of::<usize>());

    // The bundle sums present roles + the defect map; absent roles add nothing.
    let masters = CalibrationMasters {
        dark: Some(dark),
        flat: Some(constant_cfa(4, 4, 1.0, CfaType::Mono)),
        bias: None,
        flat_dark: None,
        defect_map: Some(defects),
    };
    // 320 (dark: 80·4) + 64 (flat: 16·4) + 24 (defects: 3·8) = 408 bytes.
    assert_eq!(
        masters.ram_bytes(),
        10 * 8 * 4 + 4 * 4 * 4 + 3 * std::mem::size_of::<usize>()
    );
    assert_eq!(
        masters.defect_summary(),
        Some(DefectSummary {
            hot_pixels: 2,
            cold_pixels: 1,
            percentage: 0.0,
        })
    );
}
