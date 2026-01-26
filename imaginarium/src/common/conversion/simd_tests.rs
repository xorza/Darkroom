//! SIMD correctness tests for image conversion
//!
//! These tests verify that SIMD-optimized conversion paths produce correct results
//! across various edge cases including non-aligned widths, small images, and
//! boundary values.

use crate::common::color_format::ColorFormat;
use crate::image::{Image, ImageDesc};

/// Helper to create a test image with specific pixel pattern
fn create_test_image(width: usize, height: usize, format: ColorFormat) -> Image {
    let desc = ImageDesc::new(width, height, format);
    let mut img = Image::new_black(desc).unwrap();
    let bpp = format.byte_count() as usize;
    let stride = img.desc().stride;

    // Fill with a recognizable pattern: pixel index mod 256 for each channel
    let bytes = img.bytes_mut();
    for y in 0..height {
        let row_start = y * stride;
        for x in 0..width {
            let pixel_idx = y * width + x;
            let offset = row_start + x * bpp;
            for c in 0..bpp {
                bytes[offset + c] = ((pixel_idx + c) % 256) as u8;
            }
        }
    }
    img
}

/// Helper to create a test image with f32 values
fn create_test_image_f32(width: usize, height: usize, format: ColorFormat) -> Image {
    let desc = ImageDesc::new(width, height, format);
    let mut img = Image::new_black(desc).unwrap();
    let channels = format.channel_count as usize;
    let float_stride = img.desc().stride / 4;

    // Fill with normalized values
    let bytes = img.bytes_mut();
    let floats: &mut [f32] = bytemuck::cast_slice_mut(bytes);
    for y in 0..height {
        let row_start = y * float_stride;
        for x in 0..width {
            let pixel_idx = y * width + x;
            for c in 0..channels {
                let idx = row_start + x * channels + c;
                // Generate values between 0.0 and 1.0
                floats[idx] = ((pixel_idx + c) % 256) as f32 / 255.0;
            }
        }
    }
    img
}

/// Helper to create a test image with u16 values
fn create_test_image_u16(width: usize, height: usize, format: ColorFormat) -> Image {
    let desc = ImageDesc::new(width, height, format);
    let mut img = Image::new_black(desc).unwrap();
    let channels = format.channel_count as usize;
    let word_stride = img.desc().stride / 2;

    // Fill with pattern
    let bytes = img.bytes_mut();
    let words: &mut [u16] = bytemuck::cast_slice_mut(bytes);
    for y in 0..height {
        let row_start = y * word_stride;
        for x in 0..width {
            let pixel_idx = y * width + x;
            for c in 0..channels {
                let idx = row_start + x * channels + c;
                // Generate values scaled to u16 range
                words[idx] = (((pixel_idx + c) % 256) as u16) * 257; // 0-255 -> 0-65535
            }
        }
    }
    img
}

// =============================================================================
// Round-trip tests for channel conversions
// =============================================================================

/// Test widths that exercise SIMD edge cases:
/// - 1: Single pixel (smaller than any SIMD width)
/// - 15: One less than SSE width (16)
/// - 16: Exact SSE width
/// - 17: One more than SSE width
/// - 31: One less than AVX2 width (32)
/// - 32: Exact AVX2 width
/// - 33: One more than AVX2 width
/// - 100: Arbitrary non-aligned
/// - 256: Multiple of all SIMD widths
const TEST_WIDTHS: [usize; 9] = [1, 15, 16, 17, 31, 32, 33, 100, 256];

#[test]
fn test_rgba_to_rgb_u8_various_widths() {
    for &width in &TEST_WIDTHS {
        let src = create_test_image(width, 2, ColorFormat::RGBA_U8);
        let dst = src.clone().convert(ColorFormat::RGB_U8).unwrap();

        // Verify dimensions
        assert_eq!(dst.desc().width, width);
        assert_eq!(dst.desc().height, 2);

        // Verify pixel values (RGB should match, alpha dropped)
        let src_bytes = src.bytes();
        let dst_bytes = dst.bytes();
        for y in 0..2 {
            let src_row = y * src.desc().stride;
            let dst_row = y * dst.desc().stride;
            for x in 0..width {
                let src_r = src_bytes[src_row + x * 4];
                let src_g = src_bytes[src_row + x * 4 + 1];
                let src_b = src_bytes[src_row + x * 4 + 2];

                let dst_r = dst_bytes[dst_row + x * 3];
                let dst_g = dst_bytes[dst_row + x * 3 + 1];
                let dst_b = dst_bytes[dst_row + x * 3 + 2];

                assert_eq!(src_r, dst_r, "R mismatch at ({}, {}) width={}", x, y, width);
                assert_eq!(src_g, dst_g, "G mismatch at ({}, {}) width={}", x, y, width);
                assert_eq!(src_b, dst_b, "B mismatch at ({}, {}) width={}", x, y, width);
            }
        }
    }
}

#[test]
fn test_rgb_to_rgba_u8_various_widths() {
    for &width in &TEST_WIDTHS {
        let src = create_test_image(width, 2, ColorFormat::RGB_U8);
        let dst = src.clone().convert(ColorFormat::RGBA_U8).unwrap();

        // Verify dimensions
        assert_eq!(dst.desc().width, width);
        assert_eq!(dst.desc().height, 2);

        // Verify pixel values (RGB should match, alpha should be 255)
        let src_bytes = src.bytes();
        let dst_bytes = dst.bytes();
        for y in 0..2 {
            let src_row = y * src.desc().stride;
            let dst_row = y * dst.desc().stride;
            for x in 0..width {
                let src_r = src_bytes[src_row + x * 3];
                let src_g = src_bytes[src_row + x * 3 + 1];
                let src_b = src_bytes[src_row + x * 3 + 2];

                let dst_r = dst_bytes[dst_row + x * 4];
                let dst_g = dst_bytes[dst_row + x * 4 + 1];
                let dst_b = dst_bytes[dst_row + x * 4 + 2];
                let dst_a = dst_bytes[dst_row + x * 4 + 3];

                assert_eq!(src_r, dst_r, "R mismatch at ({}, {}) width={}", x, y, width);
                assert_eq!(src_g, dst_g, "G mismatch at ({}, {}) width={}", x, y, width);
                assert_eq!(src_b, dst_b, "B mismatch at ({}, {}) width={}", x, y, width);
                assert_eq!(dst_a, 255, "Alpha should be 255 at ({}, {})", x, y);
            }
        }
    }
}

#[test]
fn test_rgba_rgb_round_trip() {
    for &width in &TEST_WIDTHS {
        let src = create_test_image(width, 2, ColorFormat::RGBA_U8);

        // Set alpha to 255 so round-trip is lossless
        let mut src_opaque = src.clone();
        let stride = src_opaque.desc().stride;
        let bytes = src_opaque.bytes_mut();
        for y in 0..2 {
            let row = y * stride;
            for x in 0..width {
                bytes[row + x * 4 + 3] = 255;
            }
        }

        let rgb = src_opaque.clone().convert(ColorFormat::RGB_U8).unwrap();
        let back = rgb.convert(ColorFormat::RGBA_U8).unwrap();

        // Compare original with round-trip
        let src_bytes = src_opaque.bytes();
        let back_bytes = back.bytes();
        for y in 0..2 {
            let src_row = y * src_opaque.desc().stride;
            let back_row = y * back.desc().stride;
            for x in 0..width {
                for c in 0..4 {
                    assert_eq!(
                        src_bytes[src_row + x * 4 + c],
                        back_bytes[back_row + x * 4 + c],
                        "Round-trip mismatch at ({}, {})[{}] width={}",
                        x,
                        y,
                        c,
                        width
                    );
                }
            }
        }
    }
}

// =============================================================================
// Luminance conversion tests
// =============================================================================

#[test]
fn test_luminance_weights_correctness() {
    // Rec. 709 weights: R=0.2126, G=0.7152, B=0.0722
    // For U8: R=54, G=183, B=19 (sum=256, shift by 8)

    // Create single pixel images with primary colors
    let desc = ImageDesc::new(1, 1, ColorFormat::RGBA_U8);

    // Pure white should give L=255
    let mut white = Image::new_black(desc).unwrap();
    white.bytes_mut()[0..4].copy_from_slice(&[255, 255, 255, 255]);
    let white_l = white.convert(ColorFormat::L_U8).unwrap();
    assert_eq!(
        white_l.bytes()[0],
        255,
        "White should convert to L=255, got {}",
        white_l.bytes()[0]
    );

    // Pure black should give L=0
    let mut black = Image::new_black(desc).unwrap();
    black.bytes_mut()[0..4].copy_from_slice(&[0, 0, 0, 255]);
    let black_l = black.convert(ColorFormat::L_U8).unwrap();
    assert_eq!(black_l.bytes()[0], 0, "Black should convert to L=0");

    // Pure red (255, 0, 0) -> L = 255 * 0.2126 ≈ 54
    let mut red = Image::new_black(desc).unwrap();
    red.bytes_mut()[0..4].copy_from_slice(&[255, 0, 0, 255]);
    let red_l = red.convert(ColorFormat::L_U8).unwrap();
    let expected_red = (255.0_f32 * 0.2126).round() as u8;
    assert!(
        (red_l.bytes()[0] as i32 - expected_red as i32).abs() <= 1,
        "Red luminance: expected ~{}, got {}",
        expected_red,
        red_l.bytes()[0]
    );

    // Pure green (0, 255, 0) -> L = 255 * 0.7152 ≈ 182
    let mut green = Image::new_black(desc).unwrap();
    green.bytes_mut()[0..4].copy_from_slice(&[0, 255, 0, 255]);
    let green_l = green.convert(ColorFormat::L_U8).unwrap();
    let expected_green = (255.0_f32 * 0.7152).round() as u8;
    assert!(
        (green_l.bytes()[0] as i32 - expected_green as i32).abs() <= 1,
        "Green luminance: expected ~{}, got {}",
        expected_green,
        green_l.bytes()[0]
    );

    // Pure blue (0, 0, 255) -> L = 255 * 0.0722 ≈ 18
    let mut blue = Image::new_black(desc).unwrap();
    blue.bytes_mut()[0..4].copy_from_slice(&[0, 0, 255, 255]);
    let blue_l = blue.convert(ColorFormat::L_U8).unwrap();
    let expected_blue = (255.0_f32 * 0.0722).round() as u8;
    assert!(
        (blue_l.bytes()[0] as i32 - expected_blue as i32).abs() <= 1,
        "Blue luminance: expected ~{}, got {}",
        expected_blue,
        blue_l.bytes()[0]
    );
}

#[test]
fn test_rgba_to_l_u8_various_widths() {
    for &width in &TEST_WIDTHS {
        let src = create_test_image(width, 2, ColorFormat::RGBA_U8);
        let dst = src.clone().convert(ColorFormat::L_U8).unwrap();

        assert_eq!(dst.desc().width, width);
        assert_eq!(dst.desc().height, 2);

        // Verify luminance calculation for each pixel
        let src_bytes = src.bytes();
        let dst_bytes = dst.bytes();
        for y in 0..2 {
            let src_row = y * src.desc().stride;
            let dst_row = y * dst.desc().stride;
            for x in 0..width {
                let r = src_bytes[src_row + x * 4] as f32;
                let g = src_bytes[src_row + x * 4 + 1] as f32;
                let b = src_bytes[src_row + x * 4 + 2] as f32;

                let expected = (r * 0.2126 + g * 0.7152 + b * 0.0722).round() as u8;
                let actual = dst_bytes[dst_row + x];

                assert!(
                    (expected as i32 - actual as i32).abs() <= 1,
                    "Luminance mismatch at ({}, {}) width={}: expected {}, got {}",
                    x,
                    y,
                    width,
                    expected,
                    actual
                );
            }
        }
    }
}

#[test]
fn test_rgb_to_l_u8_various_widths() {
    for &width in &TEST_WIDTHS {
        let src = create_test_image(width, 2, ColorFormat::RGB_U8);
        let dst = src.clone().convert(ColorFormat::L_U8).unwrap();

        assert_eq!(dst.desc().width, width);
        assert_eq!(dst.desc().height, 2);

        // Verify luminance calculation
        let src_bytes = src.bytes();
        let dst_bytes = dst.bytes();
        for y in 0..2 {
            let src_row = y * src.desc().stride;
            let dst_row = y * dst.desc().stride;
            for x in 0..width {
                let r = src_bytes[src_row + x * 3] as f32;
                let g = src_bytes[src_row + x * 3 + 1] as f32;
                let b = src_bytes[src_row + x * 3 + 2] as f32;

                let expected = (r * 0.2126 + g * 0.7152 + b * 0.0722).round() as u8;
                let actual = dst_bytes[dst_row + x];

                assert!(
                    (expected as i32 - actual as i32).abs() <= 1,
                    "Luminance mismatch at ({}, {}) width={}: expected {}, got {}",
                    x,
                    y,
                    width,
                    expected,
                    actual
                );
            }
        }
    }
}

// =============================================================================
// LA <-> RGBA conversion tests
// =============================================================================

#[test]
fn test_la_to_rgba_u8_various_widths() {
    for &width in &TEST_WIDTHS {
        let desc = ImageDesc::new(width, 2, ColorFormat::LA_U8);
        let mut src = Image::new_black(desc).unwrap();

        // Fill with test pattern: L=pixel_idx, A=(pixel_idx+1)
        let stride = src.desc().stride;
        let bytes = src.bytes_mut();
        for y in 0..2 {
            let row = y * stride;
            for x in 0..width {
                let idx = y * width + x;
                bytes[row + x * 2] = (idx % 256) as u8; // L
                bytes[row + x * 2 + 1] = ((idx + 100) % 256) as u8; // A
            }
        }

        let dst = src.clone().convert(ColorFormat::RGBA_U8).unwrap();

        // Verify: R=G=B=L, A preserved
        let src_bytes = src.bytes();
        let dst_bytes = dst.bytes();
        for y in 0..2 {
            let src_row = y * src.desc().stride;
            let dst_row = y * dst.desc().stride;
            for x in 0..width {
                let l = src_bytes[src_row + x * 2];
                let a = src_bytes[src_row + x * 2 + 1];

                assert_eq!(
                    dst_bytes[dst_row + x * 4],
                    l,
                    "R should equal L at ({}, {}) width={}",
                    x,
                    y,
                    width
                );
                assert_eq!(
                    dst_bytes[dst_row + x * 4 + 1],
                    l,
                    "G should equal L at ({}, {})",
                    x,
                    y
                );
                assert_eq!(
                    dst_bytes[dst_row + x * 4 + 2],
                    l,
                    "B should equal L at ({}, {})",
                    x,
                    y
                );
                assert_eq!(
                    dst_bytes[dst_row + x * 4 + 3],
                    a,
                    "A should be preserved at ({}, {})",
                    x,
                    y
                );
            }
        }
    }
}

#[test]
fn test_rgba_to_la_u8_various_widths() {
    for &width in &TEST_WIDTHS {
        let src = create_test_image(width, 2, ColorFormat::RGBA_U8);
        let dst = src.clone().convert(ColorFormat::LA_U8).unwrap();

        // Verify: L = luminance(R,G,B), A preserved
        let src_bytes = src.bytes();
        let dst_bytes = dst.bytes();
        for y in 0..2 {
            let src_row = y * src.desc().stride;
            let dst_row = y * dst.desc().stride;
            for x in 0..width {
                let r = src_bytes[src_row + x * 4] as f32;
                let g = src_bytes[src_row + x * 4 + 1] as f32;
                let b = src_bytes[src_row + x * 4 + 2] as f32;
                let a = src_bytes[src_row + x * 4 + 3];

                let expected_l = (r * 0.2126 + g * 0.7152 + b * 0.0722).round() as u8;
                let actual_l = dst_bytes[dst_row + x * 2];
                let actual_a = dst_bytes[dst_row + x * 2 + 1];

                assert!(
                    (expected_l as i32 - actual_l as i32).abs() <= 1,
                    "L mismatch at ({}, {}): expected {}, got {}",
                    x,
                    y,
                    expected_l,
                    actual_l
                );
                assert_eq!(a, actual_a, "A should be preserved at ({}, {})", x, y);
            }
        }
    }
}

// =============================================================================
// U8 <-> U16 conversion tests
// =============================================================================

#[test]
fn test_u8_to_u16_boundary_values() {
    // Test that 0 -> 0 and 255 -> 65535 (not 65280)
    let desc = ImageDesc::new(3, 1, ColorFormat::RGBA_U8);
    let mut src = Image::new_black(desc).unwrap();

    // Pixel 0: all zeros
    // Pixel 1: all 128s (mid value)
    // Pixel 2: all 255s (max value)
    let bytes = src.bytes_mut();
    bytes[0..4].copy_from_slice(&[0, 0, 0, 0]);
    bytes[4..8].copy_from_slice(&[128, 128, 128, 128]);
    bytes[8..12].copy_from_slice(&[255, 255, 255, 255]);

    let dst = src.convert(ColorFormat::RGBA_U16).unwrap();
    let words: &[u16] = bytemuck::cast_slice(dst.bytes());

    // 0 -> 0
    assert_eq!(words[0], 0, "0 should convert to 0");
    assert_eq!(words[1], 0);
    assert_eq!(words[2], 0);
    assert_eq!(words[3], 0);

    // 128 -> 128 * 257 = 32896
    let expected_mid = 128u16 * 257;
    assert_eq!(
        words[4], expected_mid,
        "128 should convert to {}",
        expected_mid
    );

    // 255 -> 255 * 257 = 65535 (not 65280!)
    assert_eq!(
        words[8], 65535,
        "255 should convert to 65535, got {}",
        words[8]
    );
    assert_eq!(words[9], 65535);
    assert_eq!(words[10], 65535);
    assert_eq!(words[11], 65535);
}

#[test]
fn test_u16_to_u8_boundary_values() {
    let desc = ImageDesc::new(3, 1, ColorFormat::RGBA_U16);
    let mut src = Image::new_black(desc).unwrap();
    let words: &mut [u16] = bytemuck::cast_slice_mut(src.bytes_mut());

    // Pixel 0: all zeros
    words[0..4].copy_from_slice(&[0, 0, 0, 0]);
    // Pixel 1: mid values
    words[4..8].copy_from_slice(&[32768, 32768, 32768, 32768]);
    // Pixel 2: max values
    words[8..12].copy_from_slice(&[65535, 65535, 65535, 65535]);

    let dst = src.convert(ColorFormat::RGBA_U8).unwrap();
    let bytes = dst.bytes();

    // 0 -> 0
    assert_eq!(bytes[0], 0, "0 should convert to 0");

    // 32768 / 257 ≈ 127
    let expected_mid = (32768u32 / 257) as u8;
    assert!(
        (bytes[4] as i32 - expected_mid as i32).abs() <= 1,
        "32768 should convert to ~{}, got {}",
        expected_mid,
        bytes[4]
    );

    // 65535 -> 255
    assert_eq!(
        bytes[8], 255,
        "65535 should convert to 255, got {}",
        bytes[8]
    );
}

#[test]
fn test_u8_u16_round_trip() {
    for &width in &TEST_WIDTHS {
        let src = create_test_image(width, 2, ColorFormat::RGBA_U8);
        let u16_img = src.clone().convert(ColorFormat::RGBA_U16).unwrap();
        let back = u16_img.convert(ColorFormat::RGBA_U8).unwrap();

        // Round-trip should be lossless
        let src_bytes = src.bytes();
        let back_bytes = back.bytes();
        for y in 0..2 {
            let src_row = y * src.desc().stride;
            let back_row = y * back.desc().stride;
            for x in 0..width {
                for c in 0..4 {
                    assert_eq!(
                        src_bytes[src_row + x * 4 + c],
                        back_bytes[back_row + x * 4 + c],
                        "U8->U16->U8 round-trip failed at ({}, {})[{}] width={}",
                        x,
                        y,
                        c,
                        width
                    );
                }
            }
        }
    }
}

// =============================================================================
// F32 <-> U8 conversion tests
// =============================================================================

#[test]
fn test_f32_to_u8_clamping() {
    // Test that values outside 0.0-1.0 are clamped
    let desc = ImageDesc::new(4, 1, ColorFormat::RGBA_F32);
    let mut src = Image::new_black(desc).unwrap();
    let floats: &mut [f32] = bytemuck::cast_slice_mut(src.bytes_mut());

    // Pixel 0: negative values (should clamp to 0)
    floats[0..4].copy_from_slice(&[-1.0, -0.5, -100.0, -0.1]);
    // Pixel 1: values > 1.0 (should clamp to 255)
    floats[4..8].copy_from_slice(&[1.5, 2.0, 100.0, 1.1]);
    // Pixel 2: exact boundaries
    floats[8..12].copy_from_slice(&[0.0, 1.0, 0.5, 0.0]);
    // Pixel 3: normal values
    floats[12..16].copy_from_slice(&[0.25, 0.5, 0.75, 1.0]);

    let dst = src.convert(ColorFormat::RGBA_U8).unwrap();
    let bytes = dst.bytes();

    // Negative -> 0
    assert_eq!(bytes[0], 0, "Negative should clamp to 0");
    assert_eq!(bytes[1], 0);
    assert_eq!(bytes[2], 0);
    assert_eq!(bytes[3], 0);

    // > 1.0 -> 255
    assert_eq!(bytes[4], 255, "> 1.0 should clamp to 255");
    assert_eq!(bytes[5], 255);
    assert_eq!(bytes[6], 255);
    assert_eq!(bytes[7], 255);

    // Exact boundaries
    assert_eq!(bytes[8], 0, "0.0 should convert to 0");
    assert_eq!(bytes[9], 255, "1.0 should convert to 255");
    assert!(
        (bytes[10] as i32 - 128).abs() <= 1,
        "0.5 should convert to ~128"
    );

    // Normal values
    assert!(
        (bytes[12] as i32 - 64).abs() <= 1,
        "0.25 should convert to ~64"
    );
    assert!(
        (bytes[13] as i32 - 128).abs() <= 1,
        "0.5 should convert to ~128"
    );
    assert!(
        (bytes[14] as i32 - 191).abs() <= 1,
        "0.75 should convert to ~191"
    );
    assert_eq!(bytes[15], 255, "1.0 should convert to 255");
}

#[test]
fn test_f32_to_u8_various_widths() {
    for &width in &TEST_WIDTHS {
        let src = create_test_image_f32(width, 2, ColorFormat::RGBA_F32);
        let dst = src.clone().convert(ColorFormat::RGBA_U8).unwrap();

        assert_eq!(dst.desc().width, width);
        assert_eq!(dst.desc().height, 2);

        // Verify conversion
        let floats: &[f32] = bytemuck::cast_slice(src.bytes());
        let bytes = dst.bytes();
        let float_stride = src.desc().stride / 4;

        for y in 0..2 {
            let src_row = y * float_stride;
            let dst_row = y * dst.desc().stride;
            for x in 0..width {
                for c in 0..4 {
                    let f = floats[src_row + x * 4 + c];
                    let expected = (f * 255.0).round().clamp(0.0, 255.0) as u8;
                    let actual = bytes[dst_row + x * 4 + c];

                    assert!(
                        (expected as i32 - actual as i32).abs() <= 1,
                        "F32->U8 mismatch at ({}, {})[{}] width={}: expected {}, got {}",
                        x,
                        y,
                        c,
                        width,
                        expected,
                        actual
                    );
                }
            }
        }
    }
}

// =============================================================================
// L_U16 <-> F32 conversion tests
// =============================================================================

#[test]
fn test_l_u16_to_f32_various_widths() {
    for &width in &TEST_WIDTHS {
        let src = create_test_image_u16(width, 2, ColorFormat::L_U16);
        let dst = src.clone().convert(ColorFormat::L_F32).unwrap();

        assert_eq!(dst.desc().width, width);

        // Verify conversion
        let words: &[u16] = bytemuck::cast_slice(src.bytes());
        let floats: &[f32] = bytemuck::cast_slice(dst.bytes());
        let word_stride = src.desc().stride / 2;
        let float_stride = dst.desc().stride / 4;

        for y in 0..2 {
            for x in 0..width {
                let u16_val = words[y * word_stride + x];
                let f32_val = floats[y * float_stride + x];
                let expected = u16_val as f32 / 65535.0;

                assert!(
                    (expected - f32_val).abs() < 0.0001,
                    "L_U16->F32 mismatch at ({}, {}) width={}: expected {}, got {}",
                    x,
                    y,
                    width,
                    expected,
                    f32_val
                );
            }
        }
    }
}

#[test]
fn test_l_f32_to_u16_various_widths() {
    for &width in &TEST_WIDTHS {
        let src = create_test_image_f32(width, 2, ColorFormat::L_F32);
        let dst = src.clone().convert(ColorFormat::L_U16).unwrap();

        assert_eq!(dst.desc().width, width);

        // Verify conversion
        let floats: &[f32] = bytemuck::cast_slice(src.bytes());
        let words: &[u16] = bytemuck::cast_slice(dst.bytes());
        let float_stride = src.desc().stride / 4;
        let word_stride = dst.desc().stride / 2;

        for y in 0..2 {
            for x in 0..width {
                let f32_val = floats[y * float_stride + x];
                let u16_val = words[y * word_stride + x];
                let expected = (f32_val * 65535.0).round() as u16;

                assert!(
                    (expected as i32 - u16_val as i32).abs() <= 1,
                    "L_F32->U16 mismatch at ({}, {}) width={}: expected {}, got {}",
                    x,
                    y,
                    width,
                    expected,
                    u16_val
                );
            }
        }
    }
}

// =============================================================================
// Single-row and very small image tests
// =============================================================================

#[test]
fn test_single_row_conversions() {
    // Single row with various widths
    for &width in &TEST_WIDTHS {
        let src = create_test_image(width, 1, ColorFormat::RGBA_U8);

        // Test multiple conversion paths
        let _ = src.clone().convert(ColorFormat::RGB_U8).unwrap();
        let _ = src.clone().convert(ColorFormat::L_U8).unwrap();
        let _ = src.clone().convert(ColorFormat::RGBA_U16).unwrap();
    }
}

#[test]
fn test_single_pixel_conversions() {
    let desc = ImageDesc::new(1, 1, ColorFormat::RGBA_U8);
    let mut src = Image::new_black(desc).unwrap();
    src.bytes_mut()[0..4].copy_from_slice(&[100, 150, 200, 255]);

    // Test all SIMD paths with single pixel
    let rgb = src.clone().convert(ColorFormat::RGB_U8).unwrap();
    assert_eq!(&rgb.bytes()[0..3], &[100, 150, 200]);

    let gray = src.clone().convert(ColorFormat::L_U8).unwrap();
    let expected_l = (100.0_f32 * 0.2126 + 150.0 * 0.7152 + 200.0 * 0.0722).round() as u8;
    assert!(
        (gray.bytes()[0] as i32 - expected_l as i32).abs() <= 1,
        "Single pixel luminance: expected ~{}, got {}",
        expected_l,
        gray.bytes()[0]
    );

    let u16_img = src.clone().convert(ColorFormat::RGBA_U16).unwrap();
    let words: &[u16] = bytemuck::cast_slice(u16_img.bytes());
    assert_eq!(words[0], 100 * 257);
    assert_eq!(words[1], 150 * 257);
    assert_eq!(words[2], 200 * 257);
    assert_eq!(words[3], 255 * 257);
}

// =============================================================================
// All formats - bit depth conversion tests
// =============================================================================

#[test]
fn test_all_u8_u16_format_pairs() {
    let formats_u8 = [
        ColorFormat::L_U8,
        ColorFormat::LA_U8,
        ColorFormat::RGB_U8,
        ColorFormat::RGBA_U8,
    ];
    let formats_u16 = [
        ColorFormat::L_U16,
        ColorFormat::LA_U16,
        ColorFormat::RGB_U16,
        ColorFormat::RGBA_U16,
    ];

    for (&fmt_u8, &fmt_u16) in formats_u8.iter().zip(formats_u16.iter()) {
        // U8 -> U16 -> U8 round-trip
        let src = create_test_image(33, 2, fmt_u8);
        let u16_img = src.clone().convert(fmt_u16).unwrap();
        let back = u16_img.convert(fmt_u8).unwrap();

        let src_bytes = src.bytes();
        let back_bytes = back.bytes();

        for i in 0..src_bytes.len().min(back_bytes.len()) {
            assert_eq!(
                src_bytes[i], back_bytes[i],
                "Round-trip failed for {:?} <-> {:?} at byte {}",
                fmt_u8, fmt_u16, i
            );
        }
    }
}
