//! NEON specific tests for aarch64
//!
//! These tests verify the NEON SIMD implementations directly,
//! testing various width edge cases and remainder handling.

use super::{
    TEST_WIDTHS, create_test_row_f32, create_test_row_u8, create_test_row_u16, expected_luminance,
    within_tolerance,
};
use crate::common::conversion::conversion_simd::neon;

// =============================================================================
// NEON RGBA <-> RGB tests
// =============================================================================

#[test]
fn test_neon_rgba_to_rgb_various_widths() {
    for &width in &TEST_WIDTHS {
        let src = create_test_row_u8(width, 4);
        let mut dst = vec![0u8; width * 3];

        unsafe {
            neon::convert_rgba_to_rgb_row_neon(&src, &mut dst, width);
        }

        for x in 0..width {
            assert_eq!(
                dst[x * 3],
                src[x * 4],
                "R mismatch at x={} width={}",
                x,
                width
            );
            assert_eq!(
                dst[x * 3 + 1],
                src[x * 4 + 1],
                "G mismatch at x={} width={}",
                x,
                width
            );
            assert_eq!(
                dst[x * 3 + 2],
                src[x * 4 + 2],
                "B mismatch at x={} width={}",
                x,
                width
            );
        }
    }
}

#[test]
fn test_neon_rgb_to_rgba_various_widths() {
    for &width in &TEST_WIDTHS {
        let src = create_test_row_u8(width, 3);
        let mut dst = vec![0u8; width * 4];

        unsafe {
            neon::convert_rgb_to_rgba_row_neon(&src, &mut dst, width);
        }

        for x in 0..width {
            assert_eq!(
                dst[x * 4],
                src[x * 3],
                "R mismatch at x={} width={}",
                x,
                width
            );
            assert_eq!(
                dst[x * 4 + 1],
                src[x * 3 + 1],
                "G mismatch at x={} width={}",
                x,
                width
            );
            assert_eq!(
                dst[x * 4 + 2],
                src[x * 3 + 2],
                "B mismatch at x={} width={}",
                x,
                width
            );
            assert_eq!(dst[x * 4 + 3], 255, "Alpha should be 255 at x={}", x);
        }
    }
}

#[test]
fn test_neon_rgba_rgb_round_trip() {
    for &width in &TEST_WIDTHS {
        let mut src = create_test_row_u8(width, 4);
        // Set alpha to 255 for lossless round-trip
        for x in 0..width {
            src[x * 4 + 3] = 255;
        }

        let mut rgb = vec![0u8; width * 3];
        let mut back = vec![0u8; width * 4];

        unsafe {
            neon::convert_rgba_to_rgb_row_neon(&src, &mut rgb, width);
            neon::convert_rgb_to_rgba_row_neon(&rgb, &mut back, width);
        }

        assert_eq!(src, back, "Round-trip failed at width={}", width);
    }
}

// =============================================================================
// NEON Luminance conversion tests
// =============================================================================

#[test]
fn test_neon_rgba_to_l_various_widths() {
    for &width in &TEST_WIDTHS {
        let src = create_test_row_u8(width, 4);
        let mut dst = vec![0u8; width];

        unsafe {
            neon::convert_rgba_to_l_row_neon(&src, &mut dst, width);
        }

        for x in 0..width {
            let r = src[x * 4];
            let g = src[x * 4 + 1];
            let b = src[x * 4 + 2];
            let expected = expected_luminance(r, g, b);

            assert!(
                within_tolerance(dst[x], expected, 1),
                "Luminance mismatch at x={} width={}: expected {}, got {}",
                x,
                width,
                expected,
                dst[x]
            );
        }
    }
}

#[test]
fn test_neon_rgb_to_l_various_widths() {
    for &width in &TEST_WIDTHS {
        let src = create_test_row_u8(width, 3);
        let mut dst = vec![0u8; width];

        unsafe {
            neon::convert_rgb_to_l_row_neon(&src, &mut dst, width);
        }

        for x in 0..width {
            let r = src[x * 3];
            let g = src[x * 3 + 1];
            let b = src[x * 3 + 2];
            let expected = expected_luminance(r, g, b);

            assert!(
                within_tolerance(dst[x], expected, 1),
                "Luminance mismatch at x={} width={}: expected {}, got {}",
                x,
                width,
                expected,
                dst[x]
            );
        }
    }
}

#[test]
fn test_neon_luminance_primary_colors() {
    // Test primary colors
    let test_cases = [
        ([255u8, 0, 0, 255], (255.0 * 0.2126).round() as u8), // Red
        ([0, 255, 0, 255], (255.0 * 0.7152).round() as u8),   // Green
        ([0, 0, 255, 255], (255.0 * 0.0722).round() as u8),   // Blue
        ([255, 255, 255, 255], 255),                          // White
        ([0, 0, 0, 255], 0),                                  // Black
    ];

    for (src, expected) in test_cases {
        let mut dst = [0u8; 1];

        unsafe {
            neon::convert_rgba_to_l_row_neon(&src, &mut dst, 1);
        }

        assert!(
            within_tolerance(dst[0], expected, 1),
            "Primary color test failed: {:?} -> expected {}, got {}",
            src,
            expected,
            dst[0]
        );
    }
}

// =============================================================================
// NEON L -> RGB/RGBA expansion tests
// =============================================================================

#[test]
fn test_neon_l_to_rgba_various_widths() {
    for &width in &TEST_WIDTHS {
        let src: Vec<u8> = (0..width).map(|i| (i % 256) as u8).collect();
        let mut dst = vec![0u8; width * 4];

        unsafe {
            neon::convert_l_to_rgba_row_neon(&src, &mut dst, width);
        }

        for x in 0..width {
            let l = src[x];
            assert_eq!(dst[x * 4], l, "R should equal L at x={}", x);
            assert_eq!(dst[x * 4 + 1], l, "G should equal L at x={}", x);
            assert_eq!(dst[x * 4 + 2], l, "B should equal L at x={}", x);
            assert_eq!(dst[x * 4 + 3], 255, "Alpha should be 255 at x={}", x);
        }
    }
}

#[test]
fn test_neon_l_to_rgb_various_widths() {
    for &width in &TEST_WIDTHS {
        let src: Vec<u8> = (0..width).map(|i| (i % 256) as u8).collect();
        let mut dst = vec![0u8; width * 3];

        unsafe {
            neon::convert_l_to_rgb_row_neon(&src, &mut dst, width);
        }

        for x in 0..width {
            let l = src[x];
            assert_eq!(dst[x * 3], l, "R should equal L at x={}", x);
            assert_eq!(dst[x * 3 + 1], l, "G should equal L at x={}", x);
            assert_eq!(dst[x * 3 + 2], l, "B should equal L at x={}", x);
        }
    }
}

// =============================================================================
// NEON LA <-> RGBA tests
// =============================================================================

#[test]
fn test_neon_la_to_rgba_various_widths() {
    for &width in &TEST_WIDTHS {
        let src = create_test_row_u8(width, 2);
        let mut dst = vec![0u8; width * 4];

        unsafe {
            neon::convert_la_to_rgba_row_neon(&src, &mut dst, width);
        }

        for x in 0..width {
            let l = src[x * 2];
            let a = src[x * 2 + 1];
            assert_eq!(dst[x * 4], l, "R should equal L at x={}", x);
            assert_eq!(dst[x * 4 + 1], l, "G should equal L at x={}", x);
            assert_eq!(dst[x * 4 + 2], l, "B should equal L at x={}", x);
            assert_eq!(dst[x * 4 + 3], a, "Alpha should be preserved at x={}", x);
        }
    }
}

#[test]
fn test_neon_rgba_to_la_various_widths() {
    for &width in &TEST_WIDTHS {
        let src = create_test_row_u8(width, 4);
        let mut dst = vec![0u8; width * 2];

        unsafe {
            neon::convert_rgba_to_la_row_neon(&src, &mut dst, width);
        }

        for x in 0..width {
            let r = src[x * 4];
            let g = src[x * 4 + 1];
            let b = src[x * 4 + 2];
            let a = src[x * 4 + 3];
            let expected_l = expected_luminance(r, g, b);

            assert!(
                within_tolerance(dst[x * 2], expected_l, 1),
                "L mismatch at x={}: expected {}, got {}",
                x,
                expected_l,
                dst[x * 2]
            );
            assert_eq!(dst[x * 2 + 1], a, "Alpha should be preserved at x={}", x);
        }
    }
}

// =============================================================================
// NEON F32 <-> U8 tests
// =============================================================================

#[test]
fn test_neon_f32_to_u8_various_widths() {
    for &width in &TEST_WIDTHS {
        let src = create_test_row_f32(width, 4);
        let mut dst = vec![0u8; width * 4];

        unsafe {
            neon::convert_f32_to_u8_row_neon(&src, &mut dst);
        }

        for i in 0..src.len() {
            let expected = (src[i] * 255.0).round().clamp(0.0, 255.0) as u8;
            assert!(
                within_tolerance(dst[i], expected, 1),
                "F32->U8 mismatch at i={} width={}: expected {}, got {}",
                i,
                width,
                expected,
                dst[i]
            );
        }
    }
}

#[test]
fn test_neon_f32_to_u8_clamping() {
    let src = vec![
        -1.0f32, -0.5, 0.0, 0.5, 1.0, 1.5, 2.0, 100.0, 0.25, 0.75, 0.1, 0.9, 0.0, 1.0, 0.5, 0.5,
    ]; // 16 elements for NEON
    let mut dst = vec![0u8; 16];

    unsafe {
        neon::convert_f32_to_u8_row_neon(&src, &mut dst);
    }

    assert_eq!(dst[0], 0, "Negative should clamp to 0");
    assert_eq!(dst[1], 0, "Negative should clamp to 0");
    assert_eq!(dst[2], 0, "0.0 should be 0");
    assert!(within_tolerance(dst[3], 128, 1), "0.5 should be ~128");
    assert_eq!(dst[4], 255, "1.0 should be 255");
    assert_eq!(dst[5], 255, ">1.0 should clamp to 255");
    assert_eq!(dst[6], 255, ">1.0 should clamp to 255");
    assert_eq!(dst[7], 255, ">1.0 should clamp to 255");
}

// =============================================================================
// NEON U8 <-> U16 tests
// =============================================================================

#[test]
fn test_neon_u8_to_u16_various_widths() {
    for &width in &TEST_WIDTHS {
        let src = create_test_row_u8(width, 4);
        let mut dst = vec![0u16; width * 4];

        unsafe {
            neon::convert_u8_to_u16_row_neon(&src, &mut dst);
        }

        for i in 0..src.len() {
            let expected = (src[i] as u16) * 257;
            assert_eq!(
                dst[i], expected,
                "U8->U16 mismatch at i={} width={}",
                i, width
            );
        }
    }
}

#[test]
fn test_neon_u16_to_u8_various_widths() {
    for &width in &TEST_WIDTHS {
        let src = create_test_row_u16(width, 4);
        let mut dst = vec![0u8; width * 4];

        unsafe {
            neon::convert_u16_to_u8_row_neon(&src, &mut dst);
        }

        for i in 0..dst.len() {
            let expected = (src[i] / 257) as u8;
            assert!(
                within_tolerance(dst[i], expected, 1),
                "U16->U8 mismatch at i={} width={}: expected {}, got {}",
                i,
                width,
                expected,
                dst[i]
            );
        }
    }
}

#[test]
fn test_neon_u8_u16_round_trip() {
    for &width in &TEST_WIDTHS {
        let src = create_test_row_u8(width, 4);
        let mut u16_buf = vec![0u16; width * 4];
        let mut back = vec![0u8; width * 4];

        unsafe {
            neon::convert_u8_to_u16_row_neon(&src, &mut u16_buf);
            neon::convert_u16_to_u8_row_neon(&u16_buf, &mut back);
        }

        assert_eq!(
            src, back,
            "U8->U16->U8 round-trip failed at width={}",
            width
        );
    }
}

#[test]
fn test_neon_u8_u16_boundary_values() {
    let src = vec![0u8, 1, 127, 128, 254, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]; // 16 for NEON
    let mut dst = vec![0u16; 16];

    unsafe {
        neon::convert_u8_to_u16_row_neon(&src, &mut dst);
    }

    assert_eq!(dst[0], 0, "0 -> 0");
    assert_eq!(dst[1], 257, "1 -> 257");
    assert_eq!(dst[2], 127 * 257, "127 -> 32639");
    assert_eq!(dst[3], 128 * 257, "128 -> 32896");
    assert_eq!(dst[4], 254 * 257, "254 -> 65278");
    assert_eq!(dst[5], 65535, "255 -> 65535");
}

// =============================================================================
// NEON U16 <-> F32 tests
// =============================================================================

#[test]
fn test_neon_u16_to_f32_various_widths() {
    for &width in &TEST_WIDTHS {
        let src = create_test_row_u16(width, 1);
        let mut dst = vec![0.0f32; width];

        unsafe {
            neon::convert_u16_to_f32_row_neon(&src, &mut dst);
        }

        for i in 0..width {
            let expected = src[i] as f32 / 65535.0;
            assert!(
                (dst[i] - expected).abs() < 0.0001,
                "U16->F32 mismatch at i={} width={}: expected {}, got {}",
                i,
                width,
                expected,
                dst[i]
            );
        }
    }
}

#[test]
fn test_neon_f32_to_u16_various_widths() {
    for &width in &TEST_WIDTHS {
        let src = create_test_row_f32(width, 1);
        let mut dst = vec![0u16; width];

        unsafe {
            neon::convert_f32_to_u16_row_neon(&src, &mut dst);
        }

        for i in 0..width {
            let expected = (src[i] * 65535.0).round() as u16;
            assert!(
                (dst[i] as i32 - expected as i32).abs() <= 1,
                "F32->U16 mismatch at i={} width={}: expected {}, got {}",
                i,
                width,
                expected,
                dst[i]
            );
        }
    }
}

// =============================================================================
// Edge case tests
// =============================================================================

#[test]
fn test_neon_single_pixel() {
    // Test all NEON functions with single pixel
    let rgba = [100u8, 150, 200, 255];
    let rgb = [100u8, 150, 200];
    let la = [128u8, 200];
    let l = [128u8];

    let mut dst_rgb = [0u8; 3];
    let mut dst_rgba = [0u8; 4];
    let mut dst_l = [0u8; 1];
    let mut dst_la = [0u8; 2];

    unsafe {
        neon::convert_rgba_to_rgb_row_neon(&rgba, &mut dst_rgb, 1);
        neon::convert_rgb_to_rgba_row_neon(&rgb, &mut dst_rgba, 1);
        neon::convert_rgba_to_l_row_neon(&rgba, &mut dst_l, 1);
        neon::convert_l_to_rgba_row_neon(&l, &mut dst_rgba, 1);
        neon::convert_la_to_rgba_row_neon(&la, &mut dst_rgba, 1);
        neon::convert_rgba_to_la_row_neon(&rgba, &mut dst_la, 1);
    }

    assert_eq!(dst_rgb, [100, 150, 200]);
}

#[test]
fn test_neon_simd_boundary_widths() {
    // Test exact NEON boundaries: 16 pixels for most operations, 8 for some
    let widths = [7, 8, 9, 15, 16, 17, 23, 24, 25, 31, 32, 33];

    for &width in &widths {
        let src = create_test_row_u8(width, 4);
        let mut dst = vec![0u8; width * 3];

        unsafe {
            neon::convert_rgba_to_rgb_row_neon(&src, &mut dst, width);
        }

        // Verify first and last pixels
        assert_eq!(dst[0], src[0], "First pixel R at width={}", width);

        let last = width - 1;
        assert_eq!(
            dst[last * 3],
            src[last * 4],
            "Last pixel R at width={}",
            width
        );
        assert_eq!(
            dst[last * 3 + 1],
            src[last * 4 + 1],
            "Last pixel G at width={}",
            width
        );
        assert_eq!(
            dst[last * 3 + 2],
            src[last * 4 + 2],
            "Last pixel B at width={}",
            width
        );
    }
}

#[test]
fn test_neon_large_image() {
    // Test with a large width
    let width = 1920;
    let src = create_test_row_u8(width, 4);
    let mut dst = vec![0u8; width * 3];

    unsafe {
        neon::convert_rgba_to_rgb_row_neon(&src, &mut dst, width);
    }

    // Spot check
    for &x in &[0, 100, 500, 1000, 1500, 1919] {
        assert_eq!(dst[x * 3], src[x * 4], "R at x={}", x);
        assert_eq!(dst[x * 3 + 1], src[x * 4 + 1], "G at x={}", x);
        assert_eq!(dst[x * 3 + 2], src[x * 4 + 2], "B at x={}", x);
    }
}
