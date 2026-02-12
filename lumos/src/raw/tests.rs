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
            // Values can exceed 1.0 slightly due to demosaic interpolation
            assert!(pixel <= 2.0, "Pixel value {} is too large", pixel);
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
        20000, // Above maximum -> >1.0
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
    // Above maximum should be >1.0
    assert!(result[4] > 1.0, "Above maximum should be >1.0");
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
