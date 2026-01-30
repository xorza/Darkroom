//! AVX2 specific tests for x86_64
//!
//! These tests verify the AVX2 implementations and compare against SSE results
//! to ensure consistency between different SIMD paths.

use super::{
    TEST_WIDTHS, create_test_row_f32, create_test_row_u8, create_test_row_u16, expected_luminance,
    within_tolerance,
};
use crate::common::conversion::conversion_simd::{avx, sse};
use common::cpu_features;

// =============================================================================
// AVX2 RGBA -> RGB tests
// =============================================================================

#[test]
fn test_avx_rgba_to_rgb_various_widths() {
    if !cpu_features::has_avx2() {
        return; // Skip if AVX2 not available
    }

    for &width in &TEST_WIDTHS {
        let src = create_test_row_u8(width, 4);
        let mut dst = vec![0u8; width * 3];

        unsafe {
            avx::convert_rgba_to_rgb_row_avx2(&src, &mut dst, width);
        }

        // Verify each pixel
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
fn test_avx_rgba_to_rgb_matches_sse() {
    if !cpu_features::has_avx2() || !cpu_features::get().ssse3 {
        return;
    }

    for &width in &TEST_WIDTHS {
        let src = create_test_row_u8(width, 4);
        let mut dst_avx = vec![0u8; width * 3];
        let mut dst_sse = vec![0u8; width * 3];

        unsafe {
            avx::convert_rgba_to_rgb_row_avx2(&src, &mut dst_avx, width);
            sse::convert_rgba_to_rgb_row_ssse3(&src, &mut dst_sse, width);
        }

        assert_eq!(
            dst_avx, dst_sse,
            "AVX2 and SSSE3 results differ at width={}",
            width
        );
    }
}

// =============================================================================
// AVX2 RGB -> RGBA tests (falls back to SSSE3)
// =============================================================================

#[test]
fn test_avx_rgb_to_rgba_various_widths() {
    if !cpu_features::has_avx2() {
        return;
    }

    for &width in &TEST_WIDTHS {
        let src = create_test_row_u8(width, 3);
        let mut dst = vec![0u8; width * 4];

        unsafe {
            avx::convert_rgb_to_rgba_row_avx2(&src, &mut dst, width);
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

// =============================================================================
// AVX2 Luminance tests (fall back to SSSE3)
// =============================================================================

#[test]
fn test_avx_rgba_to_l_various_widths() {
    if !cpu_features::has_avx2() {
        return;
    }

    for &width in &TEST_WIDTHS {
        let src = create_test_row_u8(width, 4);
        let mut dst = vec![0u8; width];

        unsafe {
            avx::convert_rgba_to_l_row_avx2(&src, &mut dst, width);
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
fn test_avx_rgb_to_l_various_widths() {
    if !cpu_features::has_avx2() {
        return;
    }

    for &width in &TEST_WIDTHS {
        let src = create_test_row_u8(width, 3);
        let mut dst = vec![0u8; width];

        unsafe {
            avx::convert_rgb_to_l_row_avx2(&src, &mut dst, width);
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

// =============================================================================
// AVX2 L -> RGB/RGBA expansion tests
// =============================================================================

#[test]
fn test_avx_l_to_rgba_various_widths() {
    if !cpu_features::has_avx2() {
        return;
    }

    for &width in &TEST_WIDTHS {
        let src: Vec<u8> = (0..width).map(|i| (i % 256) as u8).collect();
        let mut dst = vec![0u8; width * 4];

        unsafe {
            avx::convert_l_to_rgba_row_avx2(&src, &mut dst, width);
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
fn test_avx_l_to_rgb_various_widths() {
    if !cpu_features::has_avx2() {
        return;
    }

    for &width in &TEST_WIDTHS {
        let src: Vec<u8> = (0..width).map(|i| (i % 256) as u8).collect();
        let mut dst = vec![0u8; width * 3];

        unsafe {
            avx::convert_l_to_rgb_row_avx2(&src, &mut dst, width);
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
// AVX2 LA <-> RGBA tests
// =============================================================================

#[test]
fn test_avx_la_to_rgba_various_widths() {
    if !cpu_features::has_avx2() {
        return;
    }

    for &width in &TEST_WIDTHS {
        let src = create_test_row_u8(width, 2);
        let mut dst = vec![0u8; width * 4];

        unsafe {
            avx::convert_la_to_rgba_row_avx2(&src, &mut dst, width);
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
fn test_avx_rgba_to_la_various_widths() {
    if !cpu_features::has_avx2() {
        return;
    }

    for &width in &TEST_WIDTHS {
        let src = create_test_row_u8(width, 4);
        let mut dst = vec![0u8; width * 2];

        unsafe {
            avx::convert_rgba_to_la_row_avx2(&src, &mut dst, width);
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
// AVX2 F32 -> U8 tests
// =============================================================================

#[test]
fn test_avx_f32_to_u8_various_widths() {
    if !cpu_features::has_avx2() {
        return;
    }

    for &width in &TEST_WIDTHS {
        let src = create_test_row_f32(width, 4);
        let mut dst = vec![0u8; width * 4];

        unsafe {
            avx::convert_f32_to_u8_row_avx2(&src, &mut dst);
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
fn test_avx_f32_to_u8_matches_sse() {
    if !cpu_features::has_avx2() || !cpu_features::get().sse2 {
        return;
    }

    for &width in &TEST_WIDTHS {
        let src = create_test_row_f32(width, 4);
        let mut dst_avx = vec![0u8; width * 4];
        let mut dst_sse = vec![0u8; width * 4];

        unsafe {
            avx::convert_f32_to_u8_row_avx2(&src, &mut dst_avx);
            sse::convert_f32_to_u8_row_sse2(&src, &mut dst_sse);
        }

        for i in 0..dst_avx.len() {
            assert!(
                within_tolerance(dst_avx[i], dst_sse[i], 1),
                "AVX2 and SSE2 results differ at i={} width={}: avx={}, sse={}",
                i,
                width,
                dst_avx[i],
                dst_sse[i]
            );
        }
    }
}

#[test]
fn test_avx_f32_to_u8_clamping() {
    if !cpu_features::has_avx2() {
        return;
    }

    // Test with enough elements for AVX2 SIMD (32)
    let mut src = vec![0.5f32; 32];
    src[0] = -1.0;
    src[1] = -0.5;
    src[2] = 0.0;
    src[3] = 1.0;
    src[4] = 1.5;
    src[5] = 100.0;

    let mut dst = vec![0u8; 32];

    unsafe {
        avx::convert_f32_to_u8_row_avx2(&src, &mut dst);
    }

    assert_eq!(dst[0], 0, "Negative should clamp to 0");
    assert_eq!(dst[1], 0, "Negative should clamp to 0");
    assert_eq!(dst[2], 0, "0.0 should be 0");
    assert_eq!(dst[3], 255, "1.0 should be 255");
    assert_eq!(dst[4], 255, ">1.0 should clamp to 255");
    assert_eq!(dst[5], 255, ">1.0 should clamp to 255");
}

// =============================================================================
// AVX2 U8 <-> U16 tests
// =============================================================================

#[test]
fn test_avx_u8_to_u16_various_widths() {
    if !cpu_features::has_avx2() {
        return;
    }

    for &width in &TEST_WIDTHS {
        let src = create_test_row_u8(width, 4);
        let mut dst = vec![0u16; width * 4];

        unsafe {
            avx::convert_u8_to_u16_row_avx2(&src, &mut dst);
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
fn test_avx_u16_to_u8_various_widths() {
    if !cpu_features::has_avx2() {
        return;
    }

    for &width in &TEST_WIDTHS {
        let src = create_test_row_u16(width, 4);
        let mut dst = vec![0u8; width * 4];

        unsafe {
            avx::convert_u16_to_u8_row_avx2(&src, &mut dst);
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
fn test_avx_u8_u16_round_trip() {
    if !cpu_features::has_avx2() {
        return;
    }

    for &width in &TEST_WIDTHS {
        let src = create_test_row_u8(width, 4);
        let mut u16_buf = vec![0u16; width * 4];
        let mut back = vec![0u8; width * 4];

        unsafe {
            avx::convert_u8_to_u16_row_avx2(&src, &mut u16_buf);
            avx::convert_u16_to_u8_row_avx2(&u16_buf, &mut back);
        }

        assert_eq!(
            src, back,
            "U8->U16->U8 round-trip failed at width={}",
            width
        );
    }
}

#[test]
fn test_avx_u8_u16_matches_sse() {
    if !cpu_features::has_avx2() || !cpu_features::get().sse2 {
        return;
    }

    for &width in &TEST_WIDTHS {
        let src = create_test_row_u8(width, 4);
        let mut dst_avx = vec![0u16; width * 4];
        let mut dst_sse = vec![0u16; width * 4];

        unsafe {
            avx::convert_u8_to_u16_row_avx2(&src, &mut dst_avx);
            sse::convert_u8_to_u16_row_sse2(&src, &mut dst_sse);
        }

        assert_eq!(
            dst_avx, dst_sse,
            "AVX2 and SSE2 U8->U16 results differ at width={}",
            width
        );
    }
}

// =============================================================================
// AVX2 U16 <-> F32 tests
// =============================================================================

#[test]
fn test_avx_u16_to_f32_various_widths() {
    if !cpu_features::has_avx2() {
        return;
    }

    for &width in &TEST_WIDTHS {
        let src = create_test_row_u16(width, 1);
        let mut dst = vec![0.0f32; width];

        unsafe {
            avx::convert_u16_to_f32_row_avx2(&src, &mut dst);
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
fn test_avx_f32_to_u16_various_widths() {
    if !cpu_features::has_avx2() {
        return;
    }

    for &width in &TEST_WIDTHS {
        let src = create_test_row_f32(width, 1);
        let mut dst = vec![0u16; width];

        unsafe {
            avx::convert_f32_to_u16_row_avx2(&src, &mut dst);
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
// AVX2 boundary and edge case tests
// =============================================================================

#[test]
fn test_avx_simd_boundary_widths() {
    if !cpu_features::has_avx2() {
        return;
    }

    // Test exact AVX2 boundaries: 32 pixels for AVX2
    let widths = [31, 32, 33, 63, 64, 65, 95, 96, 97];

    for &width in &widths {
        let src = create_test_row_u8(width, 4);
        let mut dst = vec![0u8; width * 3];

        unsafe {
            avx::convert_rgba_to_rgb_row_avx2(&src, &mut dst, width);
        }

        // Verify first and last pixels
        assert_eq!(dst[0], src[0], "First pixel R at width={}", width);
        assert_eq!(dst[1], src[1], "First pixel G at width={}", width);
        assert_eq!(dst[2], src[2], "First pixel B at width={}", width);

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
fn test_avx_large_image() {
    if !cpu_features::has_avx2() {
        return;
    }

    // Test with a large width to exercise multiple AVX2 iterations
    let width = 1920; // Full HD width
    let src = create_test_row_u8(width, 4);
    let mut dst = vec![0u8; width * 3];

    unsafe {
        avx::convert_rgba_to_rgb_row_avx2(&src, &mut dst, width);
    }

    // Spot check some pixels
    for &x in &[0, 100, 500, 1000, 1500, 1919] {
        assert_eq!(dst[x * 3], src[x * 4], "R at x={}", x);
        assert_eq!(dst[x * 3 + 1], src[x * 4 + 1], "G at x={}", x);
        assert_eq!(dst[x * 3 + 2], src[x * 4 + 2], "B at x={}", x);
    }
}
