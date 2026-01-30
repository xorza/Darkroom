//! SIMD correctness tests for image conversion
//!
//! These tests verify that SIMD-optimized conversion paths produce correct results
//! across various edge cases including non-aligned widths, small images, and
//! boundary values.
//!
//! Test modules:
//! - `common_tests`: Cross-platform tests using high-level Image API
//! - `sse_tests`: x86_64 SSE2/SSSE3 specific tests
//! - `avx_tests`: x86_64 AVX2 specific tests
//! - `neon_tests`: aarch64 NEON specific tests

mod common_tests;

#[cfg(target_arch = "x86_64")]
mod sse_tests;

#[cfg(target_arch = "x86_64")]
mod avx_tests;

#[cfg(target_arch = "aarch64")]
mod neon_tests;

use crate::common::color_format::ColorFormat;
use crate::image::{Image, ImageDesc};

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
pub const TEST_WIDTHS: [usize; 9] = [1, 15, 16, 17, 31, 32, 33, 100, 256];

/// Helper to create a test image with specific pixel pattern
pub fn create_test_image(width: usize, height: usize, format: ColorFormat) -> Image {
    let desc = ImageDesc::new_with_stride(width, height, format);
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
pub fn create_test_image_f32(width: usize, height: usize, format: ColorFormat) -> Image {
    let desc = ImageDesc::new_with_stride(width, height, format);
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
pub fn create_test_image_u16(width: usize, height: usize, format: ColorFormat) -> Image {
    let desc = ImageDesc::new_with_stride(width, height, format);
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

/// Create a row buffer for testing direct row conversion functions
pub fn create_test_row_u8(width: usize, channels: usize) -> Vec<u8> {
    (0..width * channels).map(|i| (i % 256) as u8).collect()
}

/// Create a row buffer with f32 values for testing
pub fn create_test_row_f32(width: usize, channels: usize) -> Vec<f32> {
    (0..width * channels)
        .map(|i| (i % 256) as f32 / 255.0)
        .collect()
}

/// Create a row buffer with u16 values for testing
pub fn create_test_row_u16(width: usize, channels: usize) -> Vec<u16> {
    (0..width * channels)
        .map(|i| ((i % 256) as u16) * 257)
        .collect()
}

/// Compute expected luminance from RGB values using Rec. 709 weights
pub fn expected_luminance(r: u8, g: u8, b: u8) -> u8 {
    (r as f32 * 0.2126 + g as f32 * 0.7152 + b as f32 * 0.0722).round() as u8
}

/// Check if two u8 values are within tolerance (for luminance calculations)
pub fn within_tolerance(a: u8, b: u8, tolerance: i32) -> bool {
    (a as i32 - b as i32).abs() <= tolerance
}
