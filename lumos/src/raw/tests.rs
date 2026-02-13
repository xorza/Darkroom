use crate::testing::{first_raw_file, init_tracing};

use super::*;

#[test]
fn test_load_raw_invalid_path() {
    let result = load_raw(Path::new("/nonexistent/path/to/file.raf"));
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("Failed to read raw file"));
}

#[test]
fn test_load_raw_invalid_data() {
    // Create a temp file with invalid data
    let temp_dir = std::env::temp_dir();
    let temp_file = temp_dir.join("invalid_raw_test.raf");
    fs::write(&temp_file, b"not a valid raw file").unwrap();

    let result = load_raw(&temp_file);
    assert!(result.is_err());

    // Cleanup
    let _ = fs::remove_file(&temp_file);
}

#[test]
fn test_load_raw_empty_file() {
    let temp_dir = std::env::temp_dir();
    let temp_file = temp_dir.join("empty_raw_test.raf");
    fs::write(&temp_file, b"").unwrap();

    let result = load_raw(&temp_file);
    assert!(result.is_err());

    // Cleanup
    let _ = fs::remove_file(&temp_file);
}

#[test]
#[cfg_attr(not(feature = "real-data"), ignore)]
fn test_load_raw_valid_file() {
    let Some(path) = first_raw_file() else {
        eprintln!("No RAW file found for testing, skipping");
        return;
    };

    init_tracing();

    let result = load_raw(&path);
    assert!(result.is_ok(), "Failed to load {:?}: {:?}", path, result);

    let image = result.unwrap();

    // Validate dimensions
    assert!(image.dimensions().width > 0);
    assert!(image.dimensions().height > 0);
    assert_eq!(image.dimensions().channels, 3); // RGB output

    // Validate pixel values are normalized (check all channels)
    for c in 0..3 {
        for &pixel in image.channel(c) {
            assert!(pixel >= 0.0, "Pixel value {} is negative", pixel);
            // Values can exceed 1.0 slightly due to demosaic interpolation overshoot
            assert!(pixel <= 1.5, "Pixel value {} is too large", pixel);
        }
    }

    // Check mean is reasonable (not all zeros or all ones)
    let mean = image.mean();
    assert!(mean > 0.0, "Mean is zero, image may be all black");
    assert!(mean < 1.0, "Mean is >= 1.0, image may be overexposed");
}

#[test]
#[cfg_attr(not(feature = "real-data"), ignore)]
fn test_load_raw_dimensions_match() {
    let Some(path) = first_raw_file() else {
        eprintln!("No RAW file found for testing, skipping");
        return;
    };

    let image = load_raw(&path).unwrap();

    // Header dimensions should match actual dimensions
    assert_eq!(image.metadata.header_dimensions.len(), 3);
    assert_eq!(
        image.metadata.header_dimensions[0],
        image.dimensions().height
    );
    assert_eq!(
        image.metadata.header_dimensions[1],
        image.dimensions().width
    );
    assert_eq!(
        image.metadata.header_dimensions[2],
        image.dimensions().channels
    );
}

#[test]
fn test_libraw_guard_cleanup() {
    // Test that LibrawGuard properly cleans up
    {
        let inner = unsafe { sys::libraw_init(0) };
        assert!(!inner.is_null());
        let _guard = LibrawGuard(inner);
        // Guard will be dropped here and call libraw_close
    }
    // If we got here without crashing, cleanup worked
}

#[test]
fn test_libraw_guard_null_safe() {
    // Test that LibrawGuard handles null pointer safely
    let _guard = LibrawGuard(std::ptr::null_mut());
    // Should not crash on drop
}

#[test]
fn test_normalize_u16_to_f32_parallel() {
    // Test the SIMD normalization function
    let black = 512.0;
    let maximum = 16383.0;
    let inv_range = 1.0 / (maximum - black);

    // Test data with known values
    let input: Vec<u16> = vec![
        0,     // Below black -> 0.0
        512,   // At black -> 0.0
        8447,  // Midpoint -> ~0.5
        16383, // At maximum -> 1.0
        20000, // Above maximum -> clamped to 1.0
    ];

    let result = normalize::normalize_u16_to_f32_parallel(&input, black, inv_range);

    assert_eq!(result.len(), input.len());

    // Below black should be 0
    assert!((result[0] - 0.0).abs() < 1e-6, "Below black should be 0");
    // At black should be 0
    assert!((result[1] - 0.0).abs() < 1e-6, "At black should be 0");
    // Midpoint should be ~0.5
    assert!(
        (result[2] - 0.5).abs() < 0.01,
        "Midpoint should be ~0.5, got {}",
        result[2]
    );
    // At maximum should be 1.0
    assert!(
        (result[3] - 1.0).abs() < 1e-6,
        "At maximum should be 1.0, got {}",
        result[3]
    );
    // Above maximum should be clamped to 1.0
    assert!(
        (result[4] - 1.0).abs() < 1e-6,
        "Above maximum should be clamped to 1.0, got {}",
        result[4]
    );
}

#[test]
fn test_normalize_u16_large_array() {
    // Test with a large array to exercise parallel processing
    let size = 100_000;
    let input: Vec<u16> = (0..size).map(|i| (i % 65536) as u16).collect();
    let black = 0.0;
    let inv_range = 1.0 / 65535.0;

    let result = normalize::normalize_u16_to_f32_parallel(&input, black, inv_range);

    assert_eq!(result.len(), size);

    // Verify no NaN or infinite values
    for (i, &v) in result.iter().enumerate() {
        assert!(!v.is_nan(), "NaN at index {}", i);
        assert!(v.is_finite(), "Infinite at index {}", i);
        assert!(v >= 0.0, "Negative value at index {}", i);
    }

    // Check first and last values
    assert!((result[0] - 0.0).abs() < 1e-6);
    assert!((result[65535] - 1.0).abs() < 1e-4);
}

/// Test the normalize-then-crop pattern used by extract_raw_cfa_pixels.
/// Creates a synthetic raw buffer with margins and verifies active area extraction.
#[test]
fn test_normalize_and_crop_monochrome() {
    let raw_width = 10;
    let raw_height = 8;
    let width = 6;
    let height = 4;
    let top_margin = 2;
    let left_margin = 2;
    let black = 100.0;
    let maximum = 1100.0;
    let inv_range = 1.0 / (maximum - black);

    // Create raw buffer where each pixel encodes its (y, x) position
    let mut raw_data = vec![0u16; raw_width * raw_height];
    for y in 0..raw_height {
        for x in 0..raw_width {
            // Values in active area: 100 (black) to 1100 (max)
            // Margin pixels get value 0 (below black → clamped to 0.0)
            if y >= top_margin
                && y < top_margin + height
                && x >= left_margin
                && x < left_margin + width
            {
                let active_y = y - top_margin;
                let active_x = x - left_margin;
                // Linear ramp: 100 + (active_y * width + active_x) * step
                let step = (maximum - black) / (width * height - 1) as f32;
                raw_data[y * raw_width + x] =
                    (black + (active_y * width + active_x) as f32 * step) as u16;
            }
        }
    }

    // Normalize full buffer (same as process_monochrome does)
    let normalized = normalize::normalize_u16_to_f32_parallel(&raw_data, black, inv_range);
    assert_eq!(normalized.len(), raw_width * raw_height);

    // Extract active area (same as process_monochrome does)
    let mut mono_pixels = vec![0.0f32; width * height];
    for y in 0..height {
        let src_y = top_margin + y;
        let src_start = src_y * raw_width + left_margin;
        mono_pixels[y * width..y * width + width]
            .copy_from_slice(&normalized[src_start..src_start + width]);
    }

    // Verify dimensions
    assert_eq!(mono_pixels.len(), width * height);

    // First active pixel should be ~0.0 (at black level)
    assert!(
        mono_pixels[0].abs() < 0.01,
        "First pixel should be ~0.0, got {}",
        mono_pixels[0]
    );

    // Last active pixel should be ~1.0 (at maximum)
    let last = mono_pixels[width * height - 1];
    assert!(
        (last - 1.0).abs() < 0.01,
        "Last pixel should be ~1.0, got {}",
        last
    );

    // All values should be in [0.0, 1.0] range
    for (i, &v) in mono_pixels.iter().enumerate() {
        assert!(v >= 0.0, "Negative value at index {}: {}", i, v);
        assert!(v <= 1.01, "Value > 1.0 at index {}: {}", i, v);
    }

    // Values should be monotonically non-decreasing (linear ramp)
    for i in 1..mono_pixels.len() {
        assert!(
            mono_pixels[i] >= mono_pixels[i - 1] - 1e-5,
            "Non-monotonic at index {}: {} < {}",
            i,
            mono_pixels[i],
            mono_pixels[i - 1]
        );
    }
}

/// Test that margin pixels (outside active area) are zero after normalization
/// when raw values are below black level.
#[test]
fn test_normalize_below_black_clamped() {
    let black = 500.0;
    let inv_range = 1.0 / 1000.0;

    // All values below black
    let input: Vec<u16> = vec![0, 100, 200, 499];
    let result = normalize::normalize_u16_to_f32_parallel(&input, black, inv_range);

    for (i, &v) in result.iter().enumerate() {
        assert!(
            v == 0.0,
            "Value below black should be 0.0, got {} at index {}",
            v,
            i
        );
    }
}

/// Test process_unknown_libraw_fallback 16-bit normalization formula.
/// We can't call the function directly (needs libraw instance), but we can
/// verify the normalization math it uses: (v as f32) / 65535.0
#[test]
fn test_fallback_16bit_normalization() {
    let test_cases: &[(u16, f32)] = &[
        (0, 0.0),
        (1, 1.0 / 65535.0),
        (32767, 32767.0 / 65535.0),
        (65535, 1.0),
    ];

    for &(input, expected) in test_cases {
        let result = (input as f32) / 65535.0;
        assert!(
            (result - expected).abs() < 1e-6,
            "16-bit norm({}) = {}, expected {}",
            input,
            result,
            expected
        );
    }
}

/// Test process_unknown_libraw_fallback 8-bit normalization formula.
#[test]
fn test_fallback_8bit_normalization() {
    let test_cases: &[(u8, f32)] = &[(0, 0.0), (1, 1.0 / 255.0), (127, 127.0 / 255.0), (255, 1.0)];

    for &(input, expected) in test_cases {
        let result = (input as f32) / 255.0;
        assert!(
            (result - expected).abs() < 1e-6,
            "8-bit norm({}) = {}, expected {}",
            input,
            result,
            expected
        );
    }
}

// ---------------------------------------------------------------
// consolidate_black_levels tests
// ---------------------------------------------------------------

/// Uniform black: all cblack zero, scalar black only.
#[test]
fn test_consolidate_black_levels_uniform() {
    let cblack = [0u32; 4104];
    // No per-channel, no spatial pattern
    let bl = consolidate_black_levels(&cblack, 512, 16383, 0x94949494);

    assert_eq!(bl.common, 512.0);
    assert_eq!(bl.per_channel, [512.0; 4]);
    assert_eq!(bl.channel_delta_norm, [0.0; 4]);
    let expected_inv = 1.0 / (16383.0 - 512.0);
    assert!((bl.inv_range - expected_inv).abs() < 1e-10);
}

/// Per-channel cblack[0..3] nonzero, no spatial pattern.
#[test]
fn test_consolidate_black_levels_per_channel() {
    let mut cblack = [0u32; 4104];
    cblack[0] = 10; // R
    cblack[1] = 5; // G1
    cblack[2] = 15; // B
    cblack[3] = 5; // G2
    // No spatial pattern (cblack[4]==0, cblack[5]==0)

    let bl = consolidate_black_levels(&cblack, 100, 4096, 0x94949494);

    // Common minimum across channels is 5, moved to black: 100+5=105
    assert_eq!(bl.common, 105.0);
    // Per-channel: cblack[c]-5 + 105
    assert_eq!(bl.per_channel[0], 110.0); // R: 10-5+105
    assert_eq!(bl.per_channel[1], 105.0); // G1: 5-5+105
    assert_eq!(bl.per_channel[2], 115.0); // B: 15-5+105
    assert_eq!(bl.per_channel[3], 105.0); // G2: 5-5+105

    let inv = 1.0 / (4096.0 - 105.0);
    assert!((bl.inv_range - inv).abs() < 1e-10);
    // delta_norm[c] = (per_channel[c] - common) * inv_range
    assert!((bl.channel_delta_norm[0] - 5.0 * inv).abs() < 1e-6);
    assert!(bl.channel_delta_norm[1].abs() < 1e-10);
    assert!((bl.channel_delta_norm[2] - 10.0 * inv).abs() < 1e-6);
    assert!(bl.channel_delta_norm[3].abs() < 1e-10);
}

/// Bayer 2x2 spatial pattern folded into per-channel values.
#[test]
fn test_consolidate_black_levels_bayer_2x2_fold() {
    let mut cblack = [0u32; 4104];
    // 2x2 spatial pattern
    cblack[4] = 2;
    cblack[5] = 2;
    // Pattern values at spatial positions:
    cblack[6] = 4; // (0,0)
    cblack[7] = 8; // (0,1)
    cblack[8] = 12; // (1,0)
    cblack[9] = 16; // (1,1)

    // RGGB Bayer pattern filter
    // FC mapping for RGGB: (0,0)=R=0, (0,1)=G=1, (1,0)=G->G2=3, (1,1)=B=2
    // Folding: cblack[0]+=4(R), cblack[1]+=8(G1), cblack[3]+=12(G2), cblack[2]+=16(B)
    // After fold: cblack = [4, 8, 16, 12]
    // Common min = 4, subtract: cblack = [0, 4, 12, 8], black = 200+4 = 204
    let filters = 0x94949494u32;
    let bl = consolidate_black_levels(&cblack, 200, 16383, filters);

    assert_eq!(bl.common, 204.0);
    assert_eq!(bl.per_channel[0], 204.0); // R: 0 + 204
    assert_eq!(bl.per_channel[1], 208.0); // G1: 4 + 204
    assert_eq!(bl.per_channel[2], 216.0); // B: 12 + 204
    assert_eq!(bl.per_channel[3], 212.0); // G2: 8 + 204

    let inv = 1.0 / (16383.0 - 204.0);
    assert!((bl.inv_range - inv).abs() < 1e-10);
    assert!(bl.channel_delta_norm[0].abs() < 1e-10); // R: no delta
    assert!((bl.channel_delta_norm[1] - 4.0 * inv).abs() < 1e-6); // G1
    assert!((bl.channel_delta_norm[2] - 12.0 * inv).abs() < 1e-6); // B
    assert!((bl.channel_delta_norm[3] - 8.0 * inv).abs() < 1e-6); // G2
}

/// X-Trans 1x1 spatial pattern folded into all channels.
#[test]
fn test_consolidate_black_levels_xtrans_1x1_fold() {
    let mut cblack = [0u32; 4104];
    cblack[4] = 1;
    cblack[5] = 1;
    cblack[6] = 20; // Added to all channels

    // X-Trans filter value (typically 9 for 6x6 pattern)
    let bl = consolidate_black_levels(&cblack, 256, 4096, 9);

    // 1x1 pattern: cblack[6]=20 added to all cblack[0..3]
    // Then common minimum extracted (all equal = 20), moved to black: 256+20=276
    assert_eq!(bl.common, 276.0);
    assert_eq!(bl.per_channel, [276.0; 4]);
    assert_eq!(bl.channel_delta_norm, [0.0; 4]);
}

// ---------------------------------------------------------------
// compute_wb_multipliers tests
// ---------------------------------------------------------------

#[test]
fn test_compute_wb_multipliers_normal() {
    let cam_mul = [2.0, 1.0, 1.5, 1.0];
    let result = compute_wb_multipliers(cam_mul).unwrap();

    // Min is 1.0, so normalized = [2.0, 1.0, 1.5, 1.0]
    assert!((result[0] - 2.0).abs() < 1e-6);
    assert!((result[1] - 1.0).abs() < 1e-6);
    assert!((result[2] - 1.5).abs() < 1e-6);
    assert!((result[3] - 1.0).abs() < 1e-6);
}

#[test]
fn test_compute_wb_multipliers_three_channel() {
    // cam_mul[3]==0 → should copy from cam_mul[1]
    let cam_mul = [2.0, 1.0, 1.5, 0.0];
    let result = compute_wb_multipliers(cam_mul).unwrap();

    assert!((result[0] - 2.0).abs() < 1e-6);
    assert!((result[1] - 1.0).abs() < 1e-6);
    assert!((result[2] - 1.5).abs() < 1e-6);
    assert!((result[3] - 1.0).abs() < 1e-6); // copied from [1]
}

#[test]
fn test_compute_wb_multipliers_normalization() {
    let cam_mul = [4.0, 2.0, 3.0, 2.0];
    let result = compute_wb_multipliers(cam_mul).unwrap();

    // Min is 2.0, so all divided by 2.0
    assert!((result[0] - 2.0).abs() < 1e-6);
    assert!((result[1] - 1.0).abs() < 1e-6);
    assert!((result[2] - 1.5).abs() < 1e-6);
    assert!((result[3] - 1.0).abs() < 1e-6);
}

#[test]
fn test_compute_wb_multipliers_all_zeros() {
    let cam_mul = [0.0; 4];
    assert!(compute_wb_multipliers(cam_mul).is_none());
}

#[test]
fn test_compute_wb_multipliers_negative() {
    let cam_mul = [2.0, -1.0, 1.5, 1.0];
    assert!(compute_wb_multipliers(cam_mul).is_none());
}

#[test]
fn test_compute_wb_multipliers_nan() {
    let cam_mul = [2.0, f32::NAN, 1.5, 1.0];
    assert!(compute_wb_multipliers(cam_mul).is_none());
}

// ---------------------------------------------------------------
// fc (filter channel) tests
// ---------------------------------------------------------------

#[test]
fn test_fc_rggb() {
    // RGGB Bayer pattern: 0x94949494
    let filters = 0x94949494u32;

    // (0,0)=R=0, (0,1)=G=1, (1,0)=G=1, (1,1)=B=2
    assert_eq!(fc(filters, 0, 0), 0); // R
    assert_eq!(fc(filters, 0, 1), 1); // G
    assert_eq!(fc(filters, 1, 0), 1); // G
    assert_eq!(fc(filters, 1, 1), 2); // B

    // Pattern repeats
    assert_eq!(fc(filters, 2, 0), 0);
    assert_eq!(fc(filters, 2, 1), 1);
    assert_eq!(fc(filters, 3, 0), 1);
    assert_eq!(fc(filters, 3, 1), 2);
}

// ---------------------------------------------------------------
// apply_channel_corrections tests
// ---------------------------------------------------------------

#[test]
fn test_apply_channel_corrections_identity() {
    let mut data = vec![0.5f32; 4];
    let delta = [0.0; 4];
    let wb = [1.0; 4];

    apply_channel_corrections(&mut data, 2, 0x94949494, &delta, &wb);

    // No change expected
    for &v in &data {
        assert!((v - 0.5).abs() < 1e-6);
    }
}

#[test]
fn test_apply_channel_corrections_delta_only() {
    // 2x2 RGGB: positions (0,0)=R, (0,1)=G, (1,0)=G, (1,1)=B
    let mut data = vec![0.5f32; 4];
    let delta = [0.1, 0.0, 0.05, 0.0]; // R has delta=0.1, B has delta=0.05
    let wb = [1.0; 4];

    apply_channel_corrections(&mut data, 2, 0x94949494, &delta, &wb);

    assert!(
        (data[0] - 0.4).abs() < 1e-6,
        "R: 0.5-0.1=0.4, got {}",
        data[0]
    );
    assert!((data[1] - 0.5).abs() < 1e-6, "G: no delta, got {}", data[1]);
    assert!((data[2] - 0.5).abs() < 1e-6, "G: no delta, got {}", data[2]);
    assert!(
        (data[3] - 0.45).abs() < 1e-6,
        "B: 0.5-0.05=0.45, got {}",
        data[3]
    );
}

#[test]
fn test_apply_channel_corrections_wb_only() {
    let mut data = vec![0.5f32; 4];
    let delta = [0.0; 4];
    let wb = [2.0, 1.0, 1.5, 1.0]; // R×2, G×1, B×1.5

    apply_channel_corrections(&mut data, 2, 0x94949494, &delta, &wb);

    assert!(
        (data[0] - 1.0).abs() < 1e-6,
        "R: 0.5*2=1.0, got {}",
        data[0]
    );
    assert!(
        (data[1] - 0.5).abs() < 1e-6,
        "G: 0.5*1=0.5, got {}",
        data[1]
    );
    assert!(
        (data[2] - 0.5).abs() < 1e-6,
        "G: 0.5*1=0.5, got {}",
        data[2]
    );
    assert!(
        (data[3] - 0.75).abs() < 1e-6,
        "B: 0.5*1.5=0.75, got {}",
        data[3]
    );
}

#[test]
fn test_apply_channel_corrections_clamps_negative() {
    let mut data = vec![0.05f32; 4];
    let delta = [0.1, 0.0, 0.0, 0.0]; // R delta bigger than value
    let wb = [1.0; 4];

    apply_channel_corrections(&mut data, 2, 0x94949494, &delta, &wb);

    // R at (0,0): (0.05 - 0.1).max(0.0) = 0.0
    assert_eq!(data[0], 0.0, "Should clamp to 0.0");
}

#[test]
fn test_apply_channel_corrections_delta_and_wb_combined() {
    // 2x2 RGGB: (0,0)=R, (0,1)=G, (1,0)=G, (1,1)=B
    let mut data = vec![0.5f32; 4];
    let delta = [0.1, 0.0, 0.05, 0.0]; // R delta=0.1, B delta=0.05
    let wb = [2.0, 1.0, 1.5, 1.0]; // R×2, G×1, B×1.5

    apply_channel_corrections(&mut data, 2, 0x94949494, &delta, &wb);

    // R at (0,0): (0.5-0.1)*2.0 = 0.8
    assert!(
        (data[0] - 0.8).abs() < 1e-6,
        "R: (0.5-0.1)*2=0.8, got {}",
        data[0]
    );
    // G at (0,1): (0.5-0.0)*1.0 = 0.5
    assert!(
        (data[1] - 0.5).abs() < 1e-6,
        "G: no change, got {}",
        data[1]
    );
    // G at (1,0): (0.5-0.0)*1.0 = 0.5
    assert!(
        (data[2] - 0.5).abs() < 1e-6,
        "G: no change, got {}",
        data[2]
    );
    // B at (1,1): (0.5-0.05)*1.5 = 0.675
    assert!(
        (data[3] - 0.675).abs() < 1e-6,
        "B: (0.5-0.05)*1.5=0.675, got {}",
        data[3]
    );
}
