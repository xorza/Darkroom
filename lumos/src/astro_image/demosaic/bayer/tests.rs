//! Tests for Bayer CFA demosaicing.

use super::{BayerImage, CfaPattern, demosaic_bilinear, scalar};

// Test CFA patterns
#[test]
fn test_cfa_rggb_pattern() {
    let cfa = CfaPattern::Rggb;
    // Row 0: R G R G ...
    assert_eq!(cfa.color_at(0, 0), 0); // R
    assert_eq!(cfa.color_at(0, 1), 1); // G
    assert_eq!(cfa.color_at(0, 2), 0); // R
    assert_eq!(cfa.color_at(0, 3), 1); // G
    // Row 1: G B G B ...
    assert_eq!(cfa.color_at(1, 0), 1); // G
    assert_eq!(cfa.color_at(1, 1), 2); // B
    assert_eq!(cfa.color_at(1, 2), 1); // G
    assert_eq!(cfa.color_at(1, 3), 2); // B
}

#[test]
fn test_cfa_bggr_pattern() {
    let cfa = CfaPattern::Bggr;
    // Row 0: B G B G ...
    assert_eq!(cfa.color_at(0, 0), 2); // B
    assert_eq!(cfa.color_at(0, 1), 1); // G
    // Row 1: G R G R ...
    assert_eq!(cfa.color_at(1, 0), 1); // G
    assert_eq!(cfa.color_at(1, 1), 0); // R
}

#[test]
fn test_cfa_grbg_pattern() {
    let cfa = CfaPattern::Grbg;
    // Row 0: G R G R ...
    assert_eq!(cfa.color_at(0, 0), 1); // G
    assert_eq!(cfa.color_at(0, 1), 0); // R
    // Row 1: B G B G ...
    assert_eq!(cfa.color_at(1, 0), 2); // B
    assert_eq!(cfa.color_at(1, 1), 1); // G
}

#[test]
fn test_cfa_gbrg_pattern() {
    let cfa = CfaPattern::Gbrg;
    // Row 0: G B G B ...
    assert_eq!(cfa.color_at(0, 0), 1); // G
    assert_eq!(cfa.color_at(0, 1), 2); // B
    // Row 1: R G R G ...
    assert_eq!(cfa.color_at(1, 0), 0); // R
    assert_eq!(cfa.color_at(1, 1), 1); // G
}

#[test]
fn test_red_in_row() {
    // RGGB: Red in row 0, 2, 4, ...
    assert!(CfaPattern::Rggb.red_in_row(0));
    assert!(!CfaPattern::Rggb.red_in_row(1));
    assert!(CfaPattern::Rggb.red_in_row(2));

    // BGGR: Red in row 1, 3, 5, ...
    assert!(!CfaPattern::Bggr.red_in_row(0));
    assert!(CfaPattern::Bggr.red_in_row(1));
    assert!(!CfaPattern::Bggr.red_in_row(2));

    // GRBG: Red in row 0, 2, 4, ...
    assert!(CfaPattern::Grbg.red_in_row(0));
    assert!(!CfaPattern::Grbg.red_in_row(1));

    // GBRG: Red in row 1, 3, 5, ...
    assert!(!CfaPattern::Gbrg.red_in_row(0));
    assert!(CfaPattern::Gbrg.red_in_row(1));
}

#[test]
fn test_pattern_2x2() {
    assert_eq!(CfaPattern::Rggb.pattern_2x2(), [0, 1, 1, 2]);
    assert_eq!(CfaPattern::Bggr.pattern_2x2(), [2, 1, 1, 0]);
    assert_eq!(CfaPattern::Grbg.pattern_2x2(), [1, 0, 2, 1]);
    assert_eq!(CfaPattern::Gbrg.pattern_2x2(), [1, 2, 0, 1]);
}

// Test BayerImage validation
#[test]
#[should_panic(expected = "Output dimensions must be non-zero")]
fn test_bayer_image_zero_width() {
    let data = vec![0.0f32; 4];
    BayerImage::with_margins(&data, 2, 2, 0, 2, 0, 0, CfaPattern::Rggb);
}

#[test]
#[should_panic(expected = "Output dimensions must be non-zero")]
fn test_bayer_image_zero_height() {
    let data = vec![0.0f32; 4];
    BayerImage::with_margins(&data, 2, 2, 2, 0, 0, 0, CfaPattern::Rggb);
}

#[test]
#[should_panic(expected = "Data length")]
fn test_bayer_image_wrong_data_length() {
    let data = vec![0.0f32; 3]; // Should be 4
    BayerImage::with_margins(&data, 2, 2, 2, 2, 0, 0, CfaPattern::Rggb);
}

#[test]
#[should_panic(expected = "Top margin")]
fn test_bayer_image_margin_exceeds_height() {
    let data = vec![0.0f32; 4];
    BayerImage::with_margins(&data, 2, 2, 2, 2, 1, 0, CfaPattern::Rggb);
}

#[test]
#[should_panic(expected = "Left margin")]
fn test_bayer_image_margin_exceeds_width() {
    let data = vec![0.0f32; 4];
    BayerImage::with_margins(&data, 2, 2, 2, 2, 0, 1, CfaPattern::Rggb);
}

#[test]
fn test_bayer_image_valid() {
    let data = vec![0.0f32; 16];
    let bayer = BayerImage::with_margins(&data, 4, 4, 2, 2, 1, 1, CfaPattern::Rggb);
    assert_eq!(bayer.raw_width, 4);
    assert_eq!(bayer.raw_height, 4);
    assert_eq!(bayer.width, 2);
    assert_eq!(bayer.height, 2);
    assert_eq!(bayer.top_margin, 1);
    assert_eq!(bayer.left_margin, 1);
}

// Test demosaicing
#[test]
fn test_demosaic_output_size() {
    // 4x4 Bayer -> 4x4x3 RGB
    let data = vec![0.5f32; 16];
    let bayer = BayerImage::with_margins(&data, 4, 4, 4, 4, 0, 0, CfaPattern::Rggb);
    let rgb = demosaic_bilinear(&bayer);
    assert_eq!(rgb.len(), 4 * 4 * 3);
}

#[test]
fn test_demosaic_with_margins() {
    // 6x6 raw with 1px margin -> 4x4 output
    let data = vec![0.5f32; 36];
    let bayer = BayerImage::with_margins(&data, 6, 6, 4, 4, 1, 1, CfaPattern::Rggb);
    let rgb = demosaic_bilinear(&bayer);
    assert_eq!(rgb.len(), 4 * 4 * 3);
}

#[test]
fn test_demosaic_uniform_gray() {
    // Uniform gray input should produce uniform gray output
    let data = vec![0.5f32; 16];
    let bayer = BayerImage::with_margins(&data, 4, 4, 4, 4, 0, 0, CfaPattern::Rggb);
    let rgb = demosaic_bilinear(&bayer);

    for &v in &rgb {
        assert!((v - 0.5).abs() < 0.01, "Expected ~0.5, got {}", v);
    }
}

#[test]
fn test_demosaic_preserves_red_at_red_pixel() {
    // Create a pattern where red pixels have value 1.0, others 0.0
    // RGGB pattern: (0,0) is red
    let mut data = vec![0.0f32; 16];
    // Set red pixels (0,0), (0,2), (2,0), (2,2) to 1.0
    data[0] = 1.0; // (0,0)
    data[2] = 1.0; // (0,2)
    data[8] = 1.0; // (2,0)
    data[10] = 1.0; // (2,2)

    let bayer = BayerImage::with_margins(&data, 4, 4, 4, 4, 0, 0, CfaPattern::Rggb);
    let rgb = demosaic_bilinear(&bayer);

    // At (0,0), R channel should be 1.0
    assert!((rgb[0] - 1.0).abs() < 0.01, "Red at (0,0) should be 1.0");
    // G and B at (0,0) should be 0.0 (interpolated from neighbors)
    assert!(rgb[1].abs() < 0.01, "Green at (0,0) should be ~0.0");
    assert!(rgb[2].abs() < 0.01, "Blue at (0,0) should be ~0.0");
}

#[test]
fn test_demosaic_2x2_rggb() {
    // Minimal 2x2 RGGB pattern
    // R=1.0, G1=0.5, G2=0.5, B=0.0
    let data = vec![1.0, 0.5, 0.5, 0.0];
    let bayer = BayerImage::with_margins(&data, 2, 2, 2, 2, 0, 0, CfaPattern::Rggb);
    let rgb = demosaic_bilinear(&bayer);

    assert_eq!(rgb.len(), 12); // 2x2x3

    // (0,0) is R: R=1.0, G=interpolated, B=interpolated
    assert!((rgb[0] - 1.0).abs() < 0.01);

    // (0,1) is G: G=0.5
    assert!((rgb[4] - 0.5).abs() < 0.01);

    // (1,1) is B: B=0.0
    assert!((rgb[11] - 0.0).abs() < 0.01);
}

#[test]
fn test_demosaic_edge_interpolation() {
    // Test that edge pixels don't panic and produce reasonable values
    let data = vec![0.5f32; 4]; // 2x2 minimum
    let bayer = BayerImage::with_margins(&data, 2, 2, 2, 2, 0, 0, CfaPattern::Rggb);
    let rgb = demosaic_bilinear(&bayer);

    // All values should be around 0.5 since input is uniform
    for &v in &rgb {
        assert!((0.0..=1.0).contains(&v), "Value {} out of range", v);
    }
}

#[test]
fn test_demosaic_all_cfa_patterns() {
    // Test all CFA patterns produce valid output
    let data = vec![0.5f32; 16];

    for cfa in [
        CfaPattern::Rggb,
        CfaPattern::Bggr,
        CfaPattern::Grbg,
        CfaPattern::Gbrg,
    ] {
        let bayer = BayerImage::with_margins(&data, 4, 4, 4, 4, 0, 0, cfa);
        let rgb = demosaic_bilinear(&bayer);
        assert_eq!(rgb.len(), 48);
        for &v in &rgb {
            assert!(
                (v - 0.5).abs() < 0.01,
                "CFA {:?}: expected ~0.5, got {}",
                cfa,
                v
            );
        }
    }
}

#[test]
fn test_demosaic_simd_vs_scalar_consistency() {
    // Test that SIMD and scalar produce identical results for larger images
    let data: Vec<f32> = (0..100).map(|i| (i as f32 % 10.0) / 10.0).collect();
    let bayer = BayerImage::with_margins(&data, 10, 10, 10, 10, 0, 0, CfaPattern::Rggb);

    let rgb_main = demosaic_bilinear(&bayer);
    let rgb_scalar = scalar::demosaic_bilinear_scalar(&bayer);

    assert_eq!(rgb_main.len(), rgb_scalar.len());
    for (i, (&a, &b)) in rgb_main.iter().zip(rgb_scalar.iter()).enumerate() {
        assert!(
            (a - b).abs() < 1e-5,
            "Mismatch at index {}: SIMD={}, scalar={}",
            i,
            a,
            b
        );
    }
}

#[test]
fn test_demosaic_large_image() {
    // Test with a larger image that exercises SIMD paths
    let size = 64;
    let data: Vec<f32> = (0..size * size)
        .map(|i| (i as f32 % 256.0) / 255.0)
        .collect();
    let bayer = BayerImage::with_margins(&data, size, size, size, size, 0, 0, CfaPattern::Rggb);

    let rgb = demosaic_bilinear(&bayer);
    assert_eq!(rgb.len(), size * size * 3);

    // All values should be in valid range
    for &v in &rgb {
        assert!(
            (0.0..=1.5).contains(&v),
            "Value {} out of expected range",
            v
        );
    }
}

#[test]
fn test_demosaic_all_cfa_large() {
    // Test all CFA patterns with larger images to exercise SIMD
    let size = 32;
    let data: Vec<f32> = (0..size * size)
        .map(|i| (i as f32 % 100.0) / 100.0)
        .collect();

    for cfa in [
        CfaPattern::Rggb,
        CfaPattern::Bggr,
        CfaPattern::Grbg,
        CfaPattern::Gbrg,
    ] {
        let bayer = BayerImage::with_margins(&data, size, size, size, size, 0, 0, cfa);

        let rgb_main = demosaic_bilinear(&bayer);
        let rgb_scalar = scalar::demosaic_bilinear_scalar(&bayer);

        for (i, (&a, &b)) in rgb_main.iter().zip(rgb_scalar.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-5,
                "CFA {:?}: Mismatch at index {}: SIMD={}, scalar={}",
                cfa,
                i,
                a,
                b
            );
        }
    }
}

#[test]
fn test_demosaic_parallel_vs_scalar() {
    // Test that parallel processing produces same results as scalar
    let size = 256; // Large enough to trigger parallel processing
    let data: Vec<f32> = (0..size * size)
        .map(|i| (i as f32 % 100.0) / 100.0)
        .collect();

    for cfa in [
        CfaPattern::Rggb,
        CfaPattern::Bggr,
        CfaPattern::Grbg,
        CfaPattern::Gbrg,
    ] {
        let bayer = BayerImage::with_margins(&data, size, size, size, size, 0, 0, cfa);

        let rgb_parallel = demosaic_bilinear(&bayer);
        let rgb_scalar = scalar::demosaic_bilinear_scalar(&bayer);

        assert_eq!(rgb_parallel.len(), rgb_scalar.len());
        for (i, (&a, &b)) in rgb_parallel.iter().zip(rgb_scalar.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-5,
                "CFA {:?}: Mismatch at index {}: parallel={}, scalar={}",
                cfa,
                i,
                a,
                b
            );
        }
    }
}

#[test]
fn test_interpolate_horizontal_edge_cases() {
    let data = vec![0.0, 1.0, 0.0, 1.0];
    let bayer = BayerImage::with_margins(&data, 2, 2, 2, 2, 0, 0, CfaPattern::Rggb);

    // At x=0, should use right neighbor twice
    let h0 = scalar::interpolate_horizontal(&bayer, 0, 0);
    assert!((h0 - 1.0).abs() < 0.01); // (data[1] + data[1]) / 2

    // At x=1, should use left neighbor twice
    let h1 = scalar::interpolate_horizontal(&bayer, 1, 0);
    assert!((h1 - 0.0).abs() < 0.01); // (data[0] + data[0]) / 2
}

#[test]
fn test_interpolate_vertical_edge_cases() {
    let data = vec![0.0, 0.0, 1.0, 1.0];
    let bayer = BayerImage::with_margins(&data, 2, 2, 2, 2, 0, 0, CfaPattern::Rggb);

    // At y=0, should use bottom neighbor twice
    let v0 = scalar::interpolate_vertical(&bayer, 0, 0);
    assert!((v0 - 1.0).abs() < 0.01); // (data[2] + data[2]) / 2

    // At y=1, should use top neighbor twice
    let v1 = scalar::interpolate_vertical(&bayer, 0, 1);
    assert!((v1 - 0.0).abs() < 0.01); // (data[0] + data[0]) / 2
}
