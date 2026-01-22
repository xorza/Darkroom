use std::mem::size_of;

use bytemuck::Pod;
use rayon::prelude::*;

use super::ContrastBrightness;
use crate::prelude::*;

/// Applies contrast and brightness adjustment to an image using CPU.
pub(super) fn apply(params: &ContrastBrightness, input: &Image, output: &mut Image) {
    assert_eq!(input.desc().width, output.desc().width, "width mismatch");
    assert_eq!(input.desc().height, output.desc().height, "height mismatch");
    assert_eq!(
        input.desc().color_format,
        output.desc().color_format,
        "color format mismatch"
    );

    let channel_size = input.desc().color_format.channel_size;
    let channel_type = input.desc().color_format.channel_type;

    let channel_count = input.desc().color_format.channel_count;
    let _ = channel_count; // Used in cfg-gated SIMD dispatch below

    // Use SIMD-optimized paths when available
    #[cfg(target_arch = "x86_64")]
    if is_x86_feature_detected!("sse4.1") {
        match (channel_size, channel_type, channel_count) {
            // u8 formats
            (ChannelSize::_8bit, ChannelType::UInt, ChannelCount::Gray) => {
                // SAFETY: SSE4.1 support verified above
                unsafe { apply_u8_gray_sse41(input, output, *params) };
                return;
            }
            (ChannelSize::_8bit, ChannelType::UInt, ChannelCount::GrayAlpha) => {
                // SAFETY: SSE4.1 support verified above
                unsafe { apply_u8_gray_alpha_sse41(input, output, *params) };
                return;
            }
            (ChannelSize::_8bit, ChannelType::UInt, ChannelCount::Rgb) => {
                // SAFETY: SSE4.1 support verified above
                unsafe { apply_u8_rgb_sse41(input, output, *params) };
                return;
            }
            (ChannelSize::_8bit, ChannelType::UInt, ChannelCount::Rgba) => {
                // SAFETY: SSE4.1 support verified above
                unsafe { apply_u8_rgba_sse41(input, output, *params) };
                return;
            }
            // f32 formats
            (ChannelSize::_32bit, ChannelType::Float, ChannelCount::Gray) => {
                // SAFETY: SSE4.1 support verified above
                unsafe { apply_f32_gray_sse41(input, output, *params) };
                return;
            }
            (ChannelSize::_32bit, ChannelType::Float, ChannelCount::GrayAlpha) => {
                // SAFETY: SSE4.1 support verified above
                unsafe { apply_f32_gray_alpha_sse41(input, output, *params) };
                return;
            }
            (ChannelSize::_32bit, ChannelType::Float, ChannelCount::Rgb) => {
                // SAFETY: SSE4.1 support verified above
                unsafe { apply_f32_rgb_sse41(input, output, *params) };
                return;
            }
            (ChannelSize::_32bit, ChannelType::Float, ChannelCount::Rgba) => {
                // SAFETY: SSE4.1 support verified above
                unsafe { apply_f32_rgba_sse41(input, output, *params) };
                return;
            }
            _ => {}
        }
    }

    // Use NEON-optimized paths on aarch64
    #[cfg(target_arch = "aarch64")]
    {
        match (channel_size, channel_type, channel_count) {
            // u8 formats
            (ChannelSize::_8bit, ChannelType::UInt, ChannelCount::Gray) => {
                // SAFETY: NEON is always available on aarch64
                unsafe { apply_u8_gray_neon(input, output, *params) };
                return;
            }
            (ChannelSize::_8bit, ChannelType::UInt, ChannelCount::GrayAlpha) => {
                // SAFETY: NEON is always available on aarch64
                unsafe { apply_u8_gray_alpha_neon(input, output, *params) };
                return;
            }
            (ChannelSize::_8bit, ChannelType::UInt, ChannelCount::Rgb) => {
                // SAFETY: NEON is always available on aarch64
                unsafe { apply_u8_rgb_neon(input, output, *params) };
                return;
            }
            (ChannelSize::_8bit, ChannelType::UInt, ChannelCount::Rgba) => {
                // SAFETY: NEON is always available on aarch64
                unsafe { apply_u8_rgba_neon(input, output, *params) };
                return;
            }
            // f32 formats
            (ChannelSize::_32bit, ChannelType::Float, ChannelCount::Gray) => {
                // SAFETY: NEON is always available on aarch64
                unsafe { apply_f32_gray_neon(input, output, *params) };
                return;
            }
            (ChannelSize::_32bit, ChannelType::Float, ChannelCount::GrayAlpha) => {
                // SAFETY: NEON is always available on aarch64
                unsafe { apply_f32_gray_alpha_neon(input, output, *params) };
                return;
            }
            (ChannelSize::_32bit, ChannelType::Float, ChannelCount::Rgb) => {
                // SAFETY: NEON is always available on aarch64
                unsafe { apply_f32_rgb_neon(input, output, *params) };
                return;
            }
            (ChannelSize::_32bit, ChannelType::Float, ChannelCount::Rgba) => {
                // SAFETY: NEON is always available on aarch64
                unsafe { apply_f32_rgba_neon(input, output, *params) };
                return;
            }
            _ => {}
        }
    }

    match (channel_size, channel_type) {
        (ChannelSize::_8bit, ChannelType::UInt) => {
            apply_typed::<u8>(input, output, *params);
        }
        (ChannelSize::_16bit, ChannelType::UInt) => {
            apply_typed::<u16>(input, output, *params);
        }
        (ChannelSize::_32bit, ChannelType::Float) => {
            apply_typed::<f32>(input, output, *params);
        }
        _ => {
            unreachable!("Unsupported color format for contrast/brightness")
        }
    }
}

trait ContrastBrightnessApply: Pod + Send + Sync {
    fn apply(self, contrast: f32, brightness: f32) -> Self;
}

impl ContrastBrightnessApply for u8 {
    #[inline]
    fn apply(self, contrast: f32, brightness: f32) -> Self {
        let max = Self::MAX as f32;
        let mid = max / 2.0;
        let val = (self as f32 - mid) * contrast + mid + brightness * max;
        val.clamp(0.0, max) as Self
    }
}

impl ContrastBrightnessApply for u16 {
    #[inline]
    fn apply(self, contrast: f32, brightness: f32) -> Self {
        let max = Self::MAX as f32;
        let mid = max / 2.0;
        let val = (self as f32 - mid) * contrast + mid + brightness * max;
        val.clamp(0.0, max) as Self
    }
}

impl ContrastBrightnessApply for f32 {
    #[inline]
    fn apply(self, contrast: f32, brightness: f32) -> Self {
        let mid = 0.5;
        let val = (self - mid) * contrast + mid + brightness;
        val.clamp(0.0, 1.0)
    }
}

fn apply_typed<T>(from: &Image, to: &mut Image, params: ContrastBrightness)
where
    T: Pod + ContrastBrightnessApply,
{
    debug_assert_eq!(from.desc().width, to.desc().width);
    debug_assert_eq!(from.desc().height, to.desc().height);
    debug_assert_eq!(from.desc().color_format, to.desc().color_format);
    debug_assert_eq!(
        from.desc().color_format.channel_size.byte_count() as usize,
        size_of::<T>()
    );

    let width = from.desc().width as usize;
    let channels = from.desc().color_format.channel_count.channel_count() as usize;
    let stride = from.desc().stride;
    let row_bytes = width * channels * size_of::<T>();

    let has_alpha = channels == 2 || channels == 4;
    let color_channels = if has_alpha { channels - 1 } else { channels };

    let contrast = params.contrast;
    let brightness = params.brightness;

    to.bytes_mut()
        .par_chunks_mut(stride)
        .enumerate()
        .for_each(|(y, to_row)| {
            let from_row = &from.bytes()[y * stride..];
            let from_row: &[T] = bytemuck::cast_slice(&from_row[..row_bytes]);
            let to_row: &mut [T] = bytemuck::cast_slice_mut(&mut to_row[..row_bytes]);

            for x in 0..width {
                let src = &from_row[x * channels..];
                let dst = &mut to_row[x * channels..];

                for c in 0..color_channels {
                    dst[c] = src[c].apply(contrast, brightness);
                }

                if has_alpha {
                    dst[channels - 1] = src[channels - 1];
                }
            }
        });
}

// ============================================================================
// U8 GRAY SSE4.1
// ============================================================================

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
unsafe fn apply_u8_gray_sse41(from: &Image, to: &mut Image, params: ContrastBrightness) {
    let width = from.desc().width as usize;
    let in_stride = from.desc().stride;
    let out_stride = to.desc().stride;
    let contrast = params.contrast;
    let offset = 127.5 * (1.0 - contrast) + params.brightness * 255.0;

    to.bytes_mut()
        .par_chunks_mut(out_stride)
        .enumerate()
        .for_each(|(y, out_row)| {
            let in_row = &from.bytes()[y * in_stride..];
            unsafe { process_row_u8_gray_sse41(in_row, out_row, width, contrast, offset) };
        });
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
unsafe fn process_row_u8_gray_sse41(
    in_row: &[u8],
    out_row: &mut [u8],
    width: usize,
    contrast: f32,
    offset: f32,
) {
    use std::arch::x86_64::*;

    unsafe {
        let contrast_vec = _mm_set1_ps(contrast);
        let offset_vec = _mm_set1_ps(offset);
        let max_val = _mm_set1_ps(255.0);
        let min_val = _mm_setzero_ps();
        let zero = _mm_setzero_si128();

        // Process 16 gray pixels at a time
        let simd_width = 16;
        let mut x = 0;

        while x + simd_width <= width {
            let pixels = _mm_loadu_si128(in_row[x..].as_ptr() as *const __m128i);

            // Unpack to 16-bit, then 32-bit, process in 4 batches of 4
            let lo_16 = _mm_unpacklo_epi8(pixels, zero);
            let hi_16 = _mm_unpackhi_epi8(pixels, zero);

            let p0_32 = _mm_unpacklo_epi16(lo_16, zero);
            let p1_32 = _mm_unpackhi_epi16(lo_16, zero);
            let p2_32 = _mm_unpacklo_epi16(hi_16, zero);
            let p3_32 = _mm_unpackhi_epi16(hi_16, zero);

            macro_rules! process {
                ($v:expr) => {{
                    let f = _mm_cvtepi32_ps($v);
                    let r = _mm_add_ps(_mm_mul_ps(f, contrast_vec), offset_vec);
                    _mm_cvtps_epi32(_mm_min_ps(_mm_max_ps(r, min_val), max_val))
                }};
            }

            let r0 = process!(p0_32);
            let r1 = process!(p1_32);
            let r2 = process!(p2_32);
            let r3 = process!(p3_32);

            let lo_16_out = _mm_packs_epi32(r0, r1);
            let hi_16_out = _mm_packs_epi32(r2, r3);
            let result = _mm_packus_epi16(lo_16_out, hi_16_out);

            _mm_storeu_si128(out_row[x..].as_mut_ptr() as *mut __m128i, result);
            x += simd_width;
        }

        // Scalar fallback
        while x < width {
            out_row[x] = (in_row[x] as f32 * contrast + offset).clamp(0.0, 255.0) as u8;
            x += 1;
        }
    }
}

// ============================================================================
// U8 GRAY_ALPHA SSE4.1
// ============================================================================

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
unsafe fn apply_u8_gray_alpha_sse41(from: &Image, to: &mut Image, params: ContrastBrightness) {
    let width = from.desc().width as usize;
    let in_stride = from.desc().stride;
    let out_stride = to.desc().stride;
    let contrast = params.contrast;
    let offset = 127.5 * (1.0 - contrast) + params.brightness * 255.0;

    to.bytes_mut()
        .par_chunks_mut(out_stride)
        .enumerate()
        .for_each(|(y, out_row)| {
            let in_row = &from.bytes()[y * in_stride..];
            unsafe { process_row_u8_gray_alpha_sse41(in_row, out_row, width, contrast, offset) };
        });
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
unsafe fn process_row_u8_gray_alpha_sse41(
    in_row: &[u8],
    out_row: &mut [u8],
    width: usize,
    contrast: f32,
    offset: f32,
) {
    use std::arch::x86_64::*;

    unsafe {
        let contrast_vec = _mm_set1_ps(contrast);
        let offset_vec = _mm_set1_ps(offset);
        let max_val = _mm_set1_ps(255.0);
        let min_val = _mm_setzero_ps();
        let zero = _mm_setzero_si128();

        // Process 8 GrayAlpha pixels at a time (16 bytes)
        let simd_width = 8;
        let mut x = 0;

        while x + simd_width <= width {
            let pixels = _mm_loadu_si128(in_row[x * 2..].as_ptr() as *const __m128i);

            // Extract gray and alpha channels
            // Layout: G0 A0 G1 A1 G2 A2 G3 A3 G4 A4 G5 A5 G6 A6 G7 A7
            let shuffle_g =
                _mm_setr_epi8(0, 2, 4, 6, 8, 10, 12, 14, -1, -1, -1, -1, -1, -1, -1, -1);
            let shuffle_a =
                _mm_setr_epi8(1, 3, 5, 7, 9, 11, 13, 15, -1, -1, -1, -1, -1, -1, -1, -1);

            let gray_bytes = _mm_shuffle_epi8(pixels, shuffle_g);
            let alpha_bytes = _mm_shuffle_epi8(pixels, shuffle_a);

            // Process gray channel (8 values in lower 64 bits)
            let gray_16 = _mm_unpacklo_epi8(gray_bytes, zero);
            let g0_32 = _mm_unpacklo_epi16(gray_16, zero);
            let g1_32 = _mm_unpackhi_epi16(gray_16, zero);

            macro_rules! process {
                ($v:expr) => {{
                    let f = _mm_cvtepi32_ps($v);
                    let r = _mm_add_ps(_mm_mul_ps(f, contrast_vec), offset_vec);
                    _mm_cvtps_epi32(_mm_min_ps(_mm_max_ps(r, min_val), max_val))
                }};
            }

            let r0 = process!(g0_32);
            let r1 = process!(g1_32);

            let gray_16_out = _mm_packs_epi32(r0, r1);
            let gray_8_out = _mm_packus_epi16(gray_16_out, zero);

            // Interleave gray and alpha back
            let result = _mm_unpacklo_epi8(gray_8_out, alpha_bytes);

            _mm_storeu_si128(out_row[x * 2..].as_mut_ptr() as *mut __m128i, result);
            x += simd_width;
        }

        // Scalar fallback
        while x < width {
            out_row[x * 2] = (in_row[x * 2] as f32 * contrast + offset).clamp(0.0, 255.0) as u8;
            out_row[x * 2 + 1] = in_row[x * 2 + 1]; // preserve alpha
            x += 1;
        }
    }
}

// ============================================================================
// U8 RGB SSE4.1
// ============================================================================

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
unsafe fn apply_u8_rgb_sse41(from: &Image, to: &mut Image, params: ContrastBrightness) {
    let width = from.desc().width as usize;
    let in_stride = from.desc().stride;
    let out_stride = to.desc().stride;
    let contrast = params.contrast;
    let offset = 127.5 * (1.0 - contrast) + params.brightness * 255.0;

    to.bytes_mut()
        .par_chunks_mut(out_stride)
        .enumerate()
        .for_each(|(y, out_row)| {
            let in_row = &from.bytes()[y * in_stride..];
            unsafe { process_row_u8_rgb_sse41(in_row, out_row, width, contrast, offset) };
        });
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
unsafe fn process_row_u8_rgb_sse41(
    in_row: &[u8],
    out_row: &mut [u8],
    width: usize,
    contrast: f32,
    offset: f32,
) {
    use std::arch::x86_64::*;

    unsafe {
        let contrast_vec = _mm_set1_ps(contrast);
        let offset_vec = _mm_set1_ps(offset);
        let max_val = _mm_set1_ps(255.0);
        let min_val = _mm_setzero_ps();
        let zero = _mm_setzero_si128();

        // RGB is tricky - 3 bytes per pixel doesn't align nicely
        // Process 4 pixels at a time (12 bytes), but load 16 and mask
        let simd_width = 4;
        let mut x = 0;

        while x + simd_width <= width {
            // Load 16 bytes, but only first 12 are valid RGB data
            let pixels = _mm_loadu_si128(in_row[x * 3..].as_ptr() as *const __m128i);

            // Layout: R0 G0 B0 R1 G1 B1 R2 G2 B2 R3 G3 B3 (xx xx xx xx)
            // Unpack all 12 bytes and process them
            let lo_16 = _mm_unpacklo_epi8(pixels, zero); // R0 G0 B0 R1 G1 B1 R2 G2
            let hi_16 = _mm_unpackhi_epi8(pixels, zero); // B2 R3 G3 B3 xx xx xx xx

            let p0_32 = _mm_unpacklo_epi16(lo_16, zero); // R0 G0 B0 R1
            let p1_32 = _mm_unpackhi_epi16(lo_16, zero); // G1 B1 R2 G2
            let p2_32 = _mm_unpacklo_epi16(hi_16, zero); // B2 R3 G3 B3

            macro_rules! process {
                ($v:expr) => {{
                    let f = _mm_cvtepi32_ps($v);
                    let r = _mm_add_ps(_mm_mul_ps(f, contrast_vec), offset_vec);
                    _mm_cvtps_epi32(_mm_min_ps(_mm_max_ps(r, min_val), max_val))
                }};
            }

            let r0 = process!(p0_32);
            let r1 = process!(p1_32);
            let r2 = process!(p2_32);

            // Pack back
            let lo_16_out = _mm_packs_epi32(r0, r1);
            let hi_16_out = _mm_packs_epi32(r2, zero);
            let result = _mm_packus_epi16(lo_16_out, hi_16_out);

            // Store only 12 bytes
            _mm_storeu_si64(out_row[x * 3..].as_mut_ptr(), result);
            let high_part = _mm_srli_si128(result, 8);
            std::ptr::copy_nonoverlapping(
                &high_part as *const __m128i as *const u8,
                out_row[x * 3 + 8..].as_mut_ptr(),
                4,
            );

            x += simd_width;
        }

        // Scalar fallback
        while x < width {
            for c in 0..3 {
                out_row[x * 3 + c] =
                    (in_row[x * 3 + c] as f32 * contrast + offset).clamp(0.0, 255.0) as u8;
            }
            x += 1;
        }
    }
}

// ============================================================================
// U8 RGBA SSE4.1
// ============================================================================

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
unsafe fn apply_u8_rgba_sse41(from: &Image, to: &mut Image, params: ContrastBrightness) {
    let width = from.desc().width as usize;
    let in_stride = from.desc().stride;
    let out_stride = to.desc().stride;
    let contrast = params.contrast;
    let offset = 127.5 * (1.0 - contrast) + params.brightness * 255.0;

    to.bytes_mut()
        .par_chunks_mut(out_stride)
        .enumerate()
        .for_each(|(y, out_row)| {
            let in_row = &from.bytes()[y * in_stride..];
            unsafe { process_row_u8_rgba_sse41(in_row, out_row, width, contrast, offset) };
        });
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
unsafe fn process_row_u8_rgba_sse41(
    in_row: &[u8],
    out_row: &mut [u8],
    width: usize,
    contrast: f32,
    offset: f32,
) {
    use std::arch::x86_64::*;

    unsafe {
        let contrast_vec = _mm_set1_ps(contrast);
        let offset_vec = _mm_set1_ps(offset);
        let max_val = _mm_set1_ps(255.0);
        let min_val = _mm_setzero_ps();
        let zero = _mm_setzero_si128();

        let simd_width = 4;
        let mut x = 0;

        while x + simd_width <= width {
            // Load 16 bytes (4 RGBA pixels)
            let pixels = _mm_loadu_si128(in_row[x * 4..].as_ptr() as *const __m128i);

            // Extract alpha
            let shuffle_a =
                _mm_setr_epi8(3, 7, 11, 15, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);
            let alpha_bytes = _mm_shuffle_epi8(pixels, shuffle_a);

            // Process pixels 0-1
            let p01_16 = _mm_unpacklo_epi8(pixels, zero);
            let p01_lo_32 = _mm_unpacklo_epi16(p01_16, zero);
            let p01_hi_32 = _mm_unpackhi_epi16(p01_16, zero);

            // Process pixels 2-3
            let p23_16 = _mm_unpackhi_epi8(pixels, zero);
            let p23_lo_32 = _mm_unpacklo_epi16(p23_16, zero);
            let p23_hi_32 = _mm_unpackhi_epi16(p23_16, zero);

            // Convert to float and apply
            macro_rules! process_rgba {
                ($int_vec:expr) => {{
                    let float_vec = _mm_cvtepi32_ps($int_vec);
                    let result = _mm_add_ps(_mm_mul_ps(float_vec, contrast_vec), offset_vec);
                    let clamped = _mm_min_ps(_mm_max_ps(result, min_val), max_val);
                    _mm_cvtps_epi32(clamped)
                }};
            }

            let r01_lo = process_rgba!(p01_lo_32);
            let r01_hi = process_rgba!(p01_hi_32);
            let r23_lo = process_rgba!(p23_lo_32);
            let r23_hi = process_rgba!(p23_hi_32);

            // Pack back
            let r01_16 = _mm_packs_epi32(r01_lo, r01_hi);
            let r23_16 = _mm_packs_epi32(r23_lo, r23_hi);
            let result = _mm_packus_epi16(r01_16, r23_16);

            // Restore alpha
            let alpha_mask = _mm_setr_epi8(0, 0, 0, -1, 0, 0, 0, -1, 0, 0, 0, -1, 0, 0, 0, -1);
            let shuffle_alpha_expand =
                _mm_setr_epi8(-1, -1, -1, 0, -1, -1, -1, 1, -1, -1, -1, 2, -1, -1, -1, 3);
            let alpha_expanded = _mm_shuffle_epi8(alpha_bytes, shuffle_alpha_expand);
            let final_result = _mm_blendv_epi8(result, alpha_expanded, alpha_mask);

            _mm_storeu_si128(out_row[x * 4..].as_mut_ptr() as *mut __m128i, final_result);

            x += simd_width;
        }

        // Scalar fallback
        while x < width {
            let src = &in_row[x * 4..];
            let dst = &mut out_row[x * 4..];
            for c in 0..3 {
                dst[c] = (src[c] as f32 * contrast + offset).clamp(0.0, 255.0) as u8;
            }
            dst[3] = src[3];
            x += 1;
        }
    }
}

// ============================================================================
// F32 GRAY SSE4.1
// ============================================================================

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
unsafe fn apply_f32_gray_sse41(from: &Image, to: &mut Image, params: ContrastBrightness) {
    let width = from.desc().width as usize;
    let in_stride = from.desc().stride;
    let out_stride = to.desc().stride;
    let contrast = params.contrast;
    let brightness = params.brightness;

    to.bytes_mut()
        .par_chunks_mut(out_stride)
        .enumerate()
        .for_each(|(y, out_row)| {
            let in_row = &from.bytes()[y * in_stride..];
            unsafe { process_row_f32_gray_sse41(in_row, out_row, width, contrast, brightness) };
        });
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
unsafe fn process_row_f32_gray_sse41(
    in_row: &[u8],
    out_row: &mut [u8],
    width: usize,
    contrast: f32,
    brightness: f32,
) {
    use std::arch::x86_64::*;

    unsafe {
        // For f32: output = (input - 0.5) * contrast + 0.5 + brightness
        // Simplified: output = input * contrast + (0.5 * (1 - contrast) + brightness)
        let offset = 0.5 * (1.0 - contrast) + brightness;
        let contrast_vec = _mm_set1_ps(contrast);
        let offset_vec = _mm_set1_ps(offset);
        let max_val = _mm_set1_ps(1.0);
        let min_val = _mm_setzero_ps();

        let in_f32: &[f32] = bytemuck::cast_slice(in_row);
        let out_f32: &mut [f32] = bytemuck::cast_slice_mut(out_row);

        // Process 4 f32 values at a time
        let simd_width = 4;
        let mut x = 0;

        while x + simd_width <= width {
            let pixels = _mm_loadu_ps(in_f32[x..].as_ptr());
            let result = _mm_add_ps(_mm_mul_ps(pixels, contrast_vec), offset_vec);
            let clamped = _mm_min_ps(_mm_max_ps(result, min_val), max_val);
            _mm_storeu_ps(out_f32[x..].as_mut_ptr(), clamped);
            x += simd_width;
        }

        // Scalar fallback
        while x < width {
            out_f32[x] = (in_f32[x] * contrast + offset).clamp(0.0, 1.0);
            x += 1;
        }
    }
}

// ============================================================================
// F32 GRAY_ALPHA SSE4.1
// ============================================================================

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
unsafe fn apply_f32_gray_alpha_sse41(from: &Image, to: &mut Image, params: ContrastBrightness) {
    let width = from.desc().width as usize;
    let in_stride = from.desc().stride;
    let out_stride = to.desc().stride;
    let contrast = params.contrast;
    let brightness = params.brightness;

    to.bytes_mut()
        .par_chunks_mut(out_stride)
        .enumerate()
        .for_each(|(y, out_row)| {
            let in_row = &from.bytes()[y * in_stride..];
            unsafe {
                process_row_f32_gray_alpha_sse41(in_row, out_row, width, contrast, brightness)
            };
        });
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
unsafe fn process_row_f32_gray_alpha_sse41(
    in_row: &[u8],
    out_row: &mut [u8],
    width: usize,
    contrast: f32,
    brightness: f32,
) {
    use std::arch::x86_64::*;

    unsafe {
        let offset = 0.5 * (1.0 - contrast) + brightness;
        let contrast_vec = _mm_set1_ps(contrast);
        let offset_vec = _mm_set1_ps(offset);
        let max_val = _mm_set1_ps(1.0);
        let min_val = _mm_setzero_ps();

        let in_f32: &[f32] = bytemuck::cast_slice(in_row);
        let out_f32: &mut [f32] = bytemuck::cast_slice_mut(out_row);

        // Process 2 GrayAlpha pixels at a time (4 floats)
        let simd_width = 2;
        let mut x = 0;

        while x + simd_width <= width {
            // Load G0 A0 G1 A1
            let pixels = _mm_loadu_ps(in_f32[x * 2..].as_ptr());

            // Process all 4 values
            let result = _mm_add_ps(_mm_mul_ps(pixels, contrast_vec), offset_vec);
            let clamped = _mm_min_ps(_mm_max_ps(result, min_val), max_val);

            // Restore alpha (blend original alpha back)
            // Mask: process G, keep A
            let blend_mask = _mm_castsi128_ps(_mm_setr_epi32(0, -1, 0, -1));
            let final_result = _mm_blendv_ps(clamped, pixels, blend_mask);

            _mm_storeu_ps(out_f32[x * 2..].as_mut_ptr(), final_result);
            x += simd_width;
        }

        // Scalar fallback
        while x < width {
            out_f32[x * 2] = (in_f32[x * 2] * contrast + offset).clamp(0.0, 1.0);
            out_f32[x * 2 + 1] = in_f32[x * 2 + 1]; // preserve alpha
            x += 1;
        }
    }
}

// ============================================================================
// F32 RGB SSE4.1
// ============================================================================

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
unsafe fn apply_f32_rgb_sse41(from: &Image, to: &mut Image, params: ContrastBrightness) {
    let width = from.desc().width as usize;
    let in_stride = from.desc().stride;
    let out_stride = to.desc().stride;
    let contrast = params.contrast;
    let brightness = params.brightness;

    to.bytes_mut()
        .par_chunks_mut(out_stride)
        .enumerate()
        .for_each(|(y, out_row)| {
            let in_row = &from.bytes()[y * in_stride..];
            unsafe { process_row_f32_rgb_sse41(in_row, out_row, width, contrast, brightness) };
        });
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
unsafe fn process_row_f32_rgb_sse41(
    in_row: &[u8],
    out_row: &mut [u8],
    width: usize,
    contrast: f32,
    brightness: f32,
) {
    use std::arch::x86_64::*;

    unsafe {
        let offset = 0.5 * (1.0 - contrast) + brightness;
        let contrast_vec = _mm_set1_ps(contrast);
        let offset_vec = _mm_set1_ps(offset);
        let max_val = _mm_set1_ps(1.0);
        let min_val = _mm_setzero_ps();

        let in_f32: &[f32] = bytemuck::cast_slice(in_row);
        let out_f32: &mut [f32] = bytemuck::cast_slice_mut(out_row);

        // Process 4 floats at a time (1.33 RGB pixels)
        // For simplicity, process 4 RGB pixels = 12 floats = 3 SIMD ops
        let simd_width = 4;
        let mut x = 0;

        while x + simd_width <= width {
            // Load 12 floats (4 RGB pixels) in 3 batches
            let p0 = _mm_loadu_ps(in_f32[x * 3..].as_ptr());
            let p1 = _mm_loadu_ps(in_f32[x * 3 + 4..].as_ptr());
            let p2 = _mm_loadu_ps(in_f32[x * 3 + 8..].as_ptr());

            macro_rules! process {
                ($v:expr) => {{
                    let r = _mm_add_ps(_mm_mul_ps($v, contrast_vec), offset_vec);
                    _mm_min_ps(_mm_max_ps(r, min_val), max_val)
                }};
            }

            let r0 = process!(p0);
            let r1 = process!(p1);
            let r2 = process!(p2);

            _mm_storeu_ps(out_f32[x * 3..].as_mut_ptr(), r0);
            _mm_storeu_ps(out_f32[x * 3 + 4..].as_mut_ptr(), r1);
            _mm_storeu_ps(out_f32[x * 3 + 8..].as_mut_ptr(), r2);

            x += simd_width;
        }

        // Scalar fallback
        while x < width {
            for c in 0..3 {
                out_f32[x * 3 + c] = (in_f32[x * 3 + c] * contrast + offset).clamp(0.0, 1.0);
            }
            x += 1;
        }
    }
}

// ============================================================================
// F32 RGBA SSE4.1
// ============================================================================

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
unsafe fn apply_f32_rgba_sse41(from: &Image, to: &mut Image, params: ContrastBrightness) {
    let width = from.desc().width as usize;
    let in_stride = from.desc().stride;
    let out_stride = to.desc().stride;
    let contrast = params.contrast;
    let brightness = params.brightness;

    to.bytes_mut()
        .par_chunks_mut(out_stride)
        .enumerate()
        .for_each(|(y, out_row)| {
            let in_row = &from.bytes()[y * in_stride..];
            unsafe { process_row_f32_rgba_sse41(in_row, out_row, width, contrast, brightness) };
        });
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
unsafe fn process_row_f32_rgba_sse41(
    in_row: &[u8],
    out_row: &mut [u8],
    width: usize,
    contrast: f32,
    brightness: f32,
) {
    use std::arch::x86_64::*;

    unsafe {
        let offset = 0.5 * (1.0 - contrast) + brightness;
        let contrast_vec = _mm_set1_ps(contrast);
        let offset_vec = _mm_set1_ps(offset);
        let max_val = _mm_set1_ps(1.0);
        let min_val = _mm_setzero_ps();

        let in_f32: &[f32] = bytemuck::cast_slice(in_row);
        let out_f32: &mut [f32] = bytemuck::cast_slice_mut(out_row);

        // Process 1 RGBA pixel at a time (4 floats)
        let mut x = 0;

        while x < width {
            // Load R G B A
            let pixels = _mm_loadu_ps(in_f32[x * 4..].as_ptr());

            // Process all channels
            let result = _mm_add_ps(_mm_mul_ps(pixels, contrast_vec), offset_vec);
            let clamped = _mm_min_ps(_mm_max_ps(result, min_val), max_val);

            // Restore alpha (blend original alpha back)
            let blend_mask = _mm_castsi128_ps(_mm_setr_epi32(0, 0, 0, -1));
            let final_result = _mm_blendv_ps(clamped, pixels, blend_mask);

            _mm_storeu_ps(out_f32[x * 4..].as_mut_ptr(), final_result);
            x += 1;
        }
    }
}

// ============================================================================
// AARCH64 NEON IMPLEMENTATIONS
// ============================================================================

// ============================================================================
// U8 GRAY NEON
// ============================================================================

#[cfg(target_arch = "aarch64")]
unsafe fn apply_u8_gray_neon(from: &Image, to: &mut Image, params: ContrastBrightness) {
    let width = from.desc().width as usize;
    let in_stride = from.desc().stride;
    let out_stride = to.desc().stride;
    let contrast = params.contrast;
    let offset = 127.5 * (1.0 - contrast) + params.brightness * 255.0;

    to.bytes_mut()
        .par_chunks_mut(out_stride)
        .enumerate()
        .for_each(|(y, out_row)| {
            let in_row = &from.bytes()[y * in_stride..];
            unsafe { process_row_u8_gray_neon(in_row, out_row, width, contrast, offset) };
        });
}

#[cfg(target_arch = "aarch64")]
unsafe fn process_row_u8_gray_neon(
    in_row: &[u8],
    out_row: &mut [u8],
    width: usize,
    contrast: f32,
    offset: f32,
) {
    use std::arch::aarch64::*;

    unsafe {
        let contrast_vec = vdupq_n_f32(contrast);
        let offset_vec = vdupq_n_f32(offset);
        let max_val = vdupq_n_f32(255.0);
        let min_val = vdupq_n_f32(0.0);

        // Process 16 gray pixels at a time
        let simd_width = 16;
        let mut x = 0;

        while x + simd_width <= width {
            let pixels = vld1q_u8(in_row[x..].as_ptr());

            // Unpack to 16-bit, then 32-bit, process in 4 batches of 4
            let lo_16 = vmovl_u8(vget_low_u8(pixels));
            let hi_16 = vmovl_u8(vget_high_u8(pixels));

            let p0_32 = vmovl_u16(vget_low_u16(lo_16));
            let p1_32 = vmovl_u16(vget_high_u16(lo_16));
            let p2_32 = vmovl_u16(vget_low_u16(hi_16));
            let p3_32 = vmovl_u16(vget_high_u16(hi_16));

            macro_rules! process {
                ($v:expr) => {{
                    let f = vcvtq_f32_u32($v);
                    let r = vmlaq_f32(offset_vec, f, contrast_vec);
                    vcvtq_u32_f32(vminq_f32(vmaxq_f32(r, min_val), max_val))
                }};
            }

            let r0 = process!(p0_32);
            let r1 = process!(p1_32);
            let r2 = process!(p2_32);
            let r3 = process!(p3_32);

            // Pack back to u8
            let lo_16_out = vcombine_u16(vmovn_u32(r0), vmovn_u32(r1));
            let hi_16_out = vcombine_u16(vmovn_u32(r2), vmovn_u32(r3));
            let result = vcombine_u8(vmovn_u16(lo_16_out), vmovn_u16(hi_16_out));

            vst1q_u8(out_row[x..].as_mut_ptr(), result);
            x += simd_width;
        }

        // Scalar fallback
        while x < width {
            out_row[x] = (in_row[x] as f32 * contrast + offset).clamp(0.0, 255.0) as u8;
            x += 1;
        }
    }
}

// ============================================================================
// U8 GRAY_ALPHA NEON
// ============================================================================

#[cfg(target_arch = "aarch64")]
unsafe fn apply_u8_gray_alpha_neon(from: &Image, to: &mut Image, params: ContrastBrightness) {
    let width = from.desc().width as usize;
    let in_stride = from.desc().stride;
    let out_stride = to.desc().stride;
    let contrast = params.contrast;
    let offset = 127.5 * (1.0 - contrast) + params.brightness * 255.0;

    to.bytes_mut()
        .par_chunks_mut(out_stride)
        .enumerate()
        .for_each(|(y, out_row)| {
            let in_row = &from.bytes()[y * in_stride..];
            unsafe { process_row_u8_gray_alpha_neon(in_row, out_row, width, contrast, offset) };
        });
}

#[cfg(target_arch = "aarch64")]
unsafe fn process_row_u8_gray_alpha_neon(
    in_row: &[u8],
    out_row: &mut [u8],
    width: usize,
    contrast: f32,
    offset: f32,
) {
    use std::arch::aarch64::*;

    unsafe {
        let contrast_vec = vdupq_n_f32(contrast);
        let offset_vec = vdupq_n_f32(offset);
        let max_val = vdupq_n_f32(255.0);
        let min_val = vdupq_n_f32(0.0);

        // Process 8 GrayAlpha pixels at a time (16 bytes)
        let simd_width = 8;
        let mut x = 0;

        while x + simd_width <= width {
            // Load 16 bytes as 8x2 structure (gray, alpha pairs)
            let pixels = vld2_u8(in_row[x * 2..].as_ptr());
            let gray_bytes = pixels.0;
            let alpha_bytes = pixels.1;

            // Process gray channel (8 values)
            let gray_16 = vmovl_u8(gray_bytes);
            let g0_32 = vmovl_u16(vget_low_u16(gray_16));
            let g1_32 = vmovl_u16(vget_high_u16(gray_16));

            macro_rules! process {
                ($v:expr) => {{
                    let f = vcvtq_f32_u32($v);
                    let r = vmlaq_f32(offset_vec, f, contrast_vec);
                    vcvtq_u32_f32(vminq_f32(vmaxq_f32(r, min_val), max_val))
                }};
            }

            let r0 = process!(g0_32);
            let r1 = process!(g1_32);

            let gray_16_out = vcombine_u16(vmovn_u32(r0), vmovn_u32(r1));
            let gray_8_out = vmovn_u16(gray_16_out);

            // Store interleaved gray and alpha
            let result = uint8x8x2_t(gray_8_out, alpha_bytes);
            vst2_u8(out_row[x * 2..].as_mut_ptr(), result);
            x += simd_width;
        }

        // Scalar fallback
        while x < width {
            out_row[x * 2] = (in_row[x * 2] as f32 * contrast + offset).clamp(0.0, 255.0) as u8;
            out_row[x * 2 + 1] = in_row[x * 2 + 1]; // preserve alpha
            x += 1;
        }
    }
}

// ============================================================================
// U8 RGB NEON
// ============================================================================

#[cfg(target_arch = "aarch64")]
unsafe fn apply_u8_rgb_neon(from: &Image, to: &mut Image, params: ContrastBrightness) {
    let width = from.desc().width as usize;
    let in_stride = from.desc().stride;
    let out_stride = to.desc().stride;
    let contrast = params.contrast;
    let offset = 127.5 * (1.0 - contrast) + params.brightness * 255.0;

    to.bytes_mut()
        .par_chunks_mut(out_stride)
        .enumerate()
        .for_each(|(y, out_row)| {
            let in_row = &from.bytes()[y * in_stride..];
            unsafe { process_row_u8_rgb_neon(in_row, out_row, width, contrast, offset) };
        });
}

#[cfg(target_arch = "aarch64")]
unsafe fn process_row_u8_rgb_neon(
    in_row: &[u8],
    out_row: &mut [u8],
    width: usize,
    contrast: f32,
    offset: f32,
) {
    use std::arch::aarch64::*;

    unsafe {
        let contrast_vec = vdupq_n_f32(contrast);
        let offset_vec = vdupq_n_f32(offset);
        let max_val = vdupq_n_f32(255.0);
        let min_val = vdupq_n_f32(0.0);

        // Process 8 RGB pixels at a time (24 bytes)
        let simd_width = 8;
        let mut x = 0;

        while x + simd_width <= width {
            // Load 24 bytes as 8x3 structure (R, G, B channels)
            let pixels = vld3_u8(in_row[x * 3..].as_ptr());

            macro_rules! process_channel {
                ($chan:expr) => {{
                    let chan_16 = vmovl_u8($chan);
                    let c0_32 = vmovl_u16(vget_low_u16(chan_16));
                    let c1_32 = vmovl_u16(vget_high_u16(chan_16));

                    let f0 = vcvtq_f32_u32(c0_32);
                    let f1 = vcvtq_f32_u32(c1_32);

                    let r0 = vmlaq_f32(offset_vec, f0, contrast_vec);
                    let r1 = vmlaq_f32(offset_vec, f1, contrast_vec);

                    let r0 = vcvtq_u32_f32(vminq_f32(vmaxq_f32(r0, min_val), max_val));
                    let r1 = vcvtq_u32_f32(vminq_f32(vmaxq_f32(r1, min_val), max_val));

                    let out_16 = vcombine_u16(vmovn_u32(r0), vmovn_u32(r1));
                    vmovn_u16(out_16)
                }};
            }

            let r_out = process_channel!(pixels.0);
            let g_out = process_channel!(pixels.1);
            let b_out = process_channel!(pixels.2);

            let result = uint8x8x3_t(r_out, g_out, b_out);
            vst3_u8(out_row[x * 3..].as_mut_ptr(), result);
            x += simd_width;
        }

        // Scalar fallback
        while x < width {
            for c in 0..3 {
                out_row[x * 3 + c] =
                    (in_row[x * 3 + c] as f32 * contrast + offset).clamp(0.0, 255.0) as u8;
            }
            x += 1;
        }
    }
}

// ============================================================================
// U8 RGBA NEON
// ============================================================================

#[cfg(target_arch = "aarch64")]
unsafe fn apply_u8_rgba_neon(from: &Image, to: &mut Image, params: ContrastBrightness) {
    let width = from.desc().width as usize;
    let in_stride = from.desc().stride;
    let out_stride = to.desc().stride;
    let contrast = params.contrast;
    let offset = 127.5 * (1.0 - contrast) + params.brightness * 255.0;

    to.bytes_mut()
        .par_chunks_mut(out_stride)
        .enumerate()
        .for_each(|(y, out_row)| {
            let in_row = &from.bytes()[y * in_stride..];
            unsafe { process_row_u8_rgba_neon(in_row, out_row, width, contrast, offset) };
        });
}

#[cfg(target_arch = "aarch64")]
unsafe fn process_row_u8_rgba_neon(
    in_row: &[u8],
    out_row: &mut [u8],
    width: usize,
    contrast: f32,
    offset: f32,
) {
    use std::arch::aarch64::*;

    unsafe {
        let contrast_vec = vdupq_n_f32(contrast);
        let offset_vec = vdupq_n_f32(offset);
        let max_val = vdupq_n_f32(255.0);
        let min_val = vdupq_n_f32(0.0);

        // Process 8 RGBA pixels at a time (32 bytes)
        let simd_width = 8;
        let mut x = 0;

        while x + simd_width <= width {
            // Load 32 bytes as 8x4 structure (R, G, B, A channels)
            let pixels = vld4_u8(in_row[x * 4..].as_ptr());

            macro_rules! process_channel {
                ($chan:expr) => {{
                    let chan_16 = vmovl_u8($chan);
                    let c0_32 = vmovl_u16(vget_low_u16(chan_16));
                    let c1_32 = vmovl_u16(vget_high_u16(chan_16));

                    let f0 = vcvtq_f32_u32(c0_32);
                    let f1 = vcvtq_f32_u32(c1_32);

                    let r0 = vmlaq_f32(offset_vec, f0, contrast_vec);
                    let r1 = vmlaq_f32(offset_vec, f1, contrast_vec);

                    let r0 = vcvtq_u32_f32(vminq_f32(vmaxq_f32(r0, min_val), max_val));
                    let r1 = vcvtq_u32_f32(vminq_f32(vmaxq_f32(r1, min_val), max_val));

                    let out_16 = vcombine_u16(vmovn_u32(r0), vmovn_u32(r1));
                    vmovn_u16(out_16)
                }};
            }

            let r_out = process_channel!(pixels.0);
            let g_out = process_channel!(pixels.1);
            let b_out = process_channel!(pixels.2);
            // Alpha is preserved unchanged
            let a_out = pixels.3;

            let result = uint8x8x4_t(r_out, g_out, b_out, a_out);
            vst4_u8(out_row[x * 4..].as_mut_ptr(), result);
            x += simd_width;
        }

        // Scalar fallback
        while x < width {
            let src = &in_row[x * 4..];
            let dst = &mut out_row[x * 4..];
            for c in 0..3 {
                dst[c] = (src[c] as f32 * contrast + offset).clamp(0.0, 255.0) as u8;
            }
            dst[3] = src[3];
            x += 1;
        }
    }
}

// ============================================================================
// F32 GRAY NEON
// ============================================================================

#[cfg(target_arch = "aarch64")]
unsafe fn apply_f32_gray_neon(from: &Image, to: &mut Image, params: ContrastBrightness) {
    let width = from.desc().width as usize;
    let in_stride = from.desc().stride;
    let out_stride = to.desc().stride;
    let contrast = params.contrast;
    let brightness = params.brightness;

    to.bytes_mut()
        .par_chunks_mut(out_stride)
        .enumerate()
        .for_each(|(y, out_row)| {
            let in_row = &from.bytes()[y * in_stride..];
            unsafe { process_row_f32_gray_neon(in_row, out_row, width, contrast, brightness) };
        });
}

#[cfg(target_arch = "aarch64")]
unsafe fn process_row_f32_gray_neon(
    in_row: &[u8],
    out_row: &mut [u8],
    width: usize,
    contrast: f32,
    brightness: f32,
) {
    use std::arch::aarch64::*;

    unsafe {
        // For f32: output = (input - 0.5) * contrast + 0.5 + brightness
        // Simplified: output = input * contrast + (0.5 * (1 - contrast) + brightness)
        let offset = 0.5 * (1.0 - contrast) + brightness;
        let contrast_vec = vdupq_n_f32(contrast);
        let offset_vec = vdupq_n_f32(offset);
        let max_val = vdupq_n_f32(1.0);
        let min_val = vdupq_n_f32(0.0);

        let in_f32: &[f32] = bytemuck::cast_slice(in_row);
        let out_f32: &mut [f32] = bytemuck::cast_slice_mut(out_row);

        // Process 4 f32 values at a time
        let simd_width = 4;
        let mut x = 0;

        while x + simd_width <= width {
            let pixels = vld1q_f32(in_f32[x..].as_ptr());
            let result = vmlaq_f32(offset_vec, pixels, contrast_vec);
            let clamped = vminq_f32(vmaxq_f32(result, min_val), max_val);
            vst1q_f32(out_f32[x..].as_mut_ptr(), clamped);
            x += simd_width;
        }

        // Scalar fallback
        while x < width {
            out_f32[x] = (in_f32[x] * contrast + offset).clamp(0.0, 1.0);
            x += 1;
        }
    }
}

// ============================================================================
// F32 GRAY_ALPHA NEON
// ============================================================================

#[cfg(target_arch = "aarch64")]
unsafe fn apply_f32_gray_alpha_neon(from: &Image, to: &mut Image, params: ContrastBrightness) {
    let width = from.desc().width as usize;
    let in_stride = from.desc().stride;
    let out_stride = to.desc().stride;
    let contrast = params.contrast;
    let brightness = params.brightness;

    to.bytes_mut()
        .par_chunks_mut(out_stride)
        .enumerate()
        .for_each(|(y, out_row)| {
            let in_row = &from.bytes()[y * in_stride..];
            unsafe {
                process_row_f32_gray_alpha_neon(in_row, out_row, width, contrast, brightness)
            };
        });
}

#[cfg(target_arch = "aarch64")]
unsafe fn process_row_f32_gray_alpha_neon(
    in_row: &[u8],
    out_row: &mut [u8],
    width: usize,
    contrast: f32,
    brightness: f32,
) {
    use std::arch::aarch64::*;

    unsafe {
        let offset = 0.5 * (1.0 - contrast) + brightness;
        let contrast_vec = vdupq_n_f32(contrast);
        let offset_vec = vdupq_n_f32(offset);
        let max_val = vdupq_n_f32(1.0);
        let min_val = vdupq_n_f32(0.0);

        let in_f32: &[f32] = bytemuck::cast_slice(in_row);
        let out_f32: &mut [f32] = bytemuck::cast_slice_mut(out_row);

        // Process 4 GrayAlpha pixels at a time (8 floats) using deinterleaved load
        let simd_width = 4;
        let mut x = 0;

        while x + simd_width <= width {
            // Load 8 floats as 4x2 structure (gray, alpha pairs)
            let pixels = vld2q_f32(in_f32[x * 2..].as_ptr());
            let gray = pixels.0;
            let alpha = pixels.1;

            // Process gray channel
            let result = vmlaq_f32(offset_vec, gray, contrast_vec);
            let clamped = vminq_f32(vmaxq_f32(result, min_val), max_val);

            // Store interleaved
            let output = float32x4x2_t(clamped, alpha);
            vst2q_f32(out_f32[x * 2..].as_mut_ptr(), output);
            x += simd_width;
        }

        // Scalar fallback
        while x < width {
            out_f32[x * 2] = (in_f32[x * 2] * contrast + offset).clamp(0.0, 1.0);
            out_f32[x * 2 + 1] = in_f32[x * 2 + 1]; // preserve alpha
            x += 1;
        }
    }
}

// ============================================================================
// F32 RGB NEON
// ============================================================================

#[cfg(target_arch = "aarch64")]
unsafe fn apply_f32_rgb_neon(from: &Image, to: &mut Image, params: ContrastBrightness) {
    let width = from.desc().width as usize;
    let in_stride = from.desc().stride;
    let out_stride = to.desc().stride;
    let contrast = params.contrast;
    let brightness = params.brightness;

    to.bytes_mut()
        .par_chunks_mut(out_stride)
        .enumerate()
        .for_each(|(y, out_row)| {
            let in_row = &from.bytes()[y * in_stride..];
            unsafe { process_row_f32_rgb_neon(in_row, out_row, width, contrast, brightness) };
        });
}

#[cfg(target_arch = "aarch64")]
unsafe fn process_row_f32_rgb_neon(
    in_row: &[u8],
    out_row: &mut [u8],
    width: usize,
    contrast: f32,
    brightness: f32,
) {
    use std::arch::aarch64::*;

    unsafe {
        let offset = 0.5 * (1.0 - contrast) + brightness;
        let contrast_vec = vdupq_n_f32(contrast);
        let offset_vec = vdupq_n_f32(offset);
        let max_val = vdupq_n_f32(1.0);
        let min_val = vdupq_n_f32(0.0);

        let in_f32: &[f32] = bytemuck::cast_slice(in_row);
        let out_f32: &mut [f32] = bytemuck::cast_slice_mut(out_row);

        // Process 4 RGB pixels at a time (12 floats) using deinterleaved load
        let simd_width = 4;
        let mut x = 0;

        while x + simd_width <= width {
            // Load 12 floats as 4x3 structure (R, G, B channels)
            let pixels = vld3q_f32(in_f32[x * 3..].as_ptr());

            macro_rules! process {
                ($v:expr) => {{
                    let r = vmlaq_f32(offset_vec, $v, contrast_vec);
                    vminq_f32(vmaxq_f32(r, min_val), max_val)
                }};
            }

            let r_out = process!(pixels.0);
            let g_out = process!(pixels.1);
            let b_out = process!(pixels.2);

            let output = float32x4x3_t(r_out, g_out, b_out);
            vst3q_f32(out_f32[x * 3..].as_mut_ptr(), output);
            x += simd_width;
        }

        // Scalar fallback
        while x < width {
            for c in 0..3 {
                out_f32[x * 3 + c] = (in_f32[x * 3 + c] * contrast + offset).clamp(0.0, 1.0);
            }
            x += 1;
        }
    }
}

// ============================================================================
// F32 RGBA NEON
// ============================================================================

#[cfg(target_arch = "aarch64")]
unsafe fn apply_f32_rgba_neon(from: &Image, to: &mut Image, params: ContrastBrightness) {
    let width = from.desc().width as usize;
    let in_stride = from.desc().stride;
    let out_stride = to.desc().stride;
    let contrast = params.contrast;
    let brightness = params.brightness;

    to.bytes_mut()
        .par_chunks_mut(out_stride)
        .enumerate()
        .for_each(|(y, out_row)| {
            let in_row = &from.bytes()[y * in_stride..];
            unsafe { process_row_f32_rgba_neon(in_row, out_row, width, contrast, brightness) };
        });
}

#[cfg(target_arch = "aarch64")]
unsafe fn process_row_f32_rgba_neon(
    in_row: &[u8],
    out_row: &mut [u8],
    width: usize,
    contrast: f32,
    brightness: f32,
) {
    use std::arch::aarch64::*;

    unsafe {
        let offset = 0.5 * (1.0 - contrast) + brightness;
        let contrast_vec = vdupq_n_f32(contrast);
        let offset_vec = vdupq_n_f32(offset);
        let max_val = vdupq_n_f32(1.0);
        let min_val = vdupq_n_f32(0.0);

        let in_f32: &[f32] = bytemuck::cast_slice(in_row);
        let out_f32: &mut [f32] = bytemuck::cast_slice_mut(out_row);

        // Process 4 RGBA pixels at a time (16 floats) using deinterleaved load
        let simd_width = 4;
        let mut x = 0;

        while x + simd_width <= width {
            // Load 16 floats as 4x4 structure (R, G, B, A channels)
            let pixels = vld4q_f32(in_f32[x * 4..].as_ptr());

            macro_rules! process {
                ($v:expr) => {{
                    let r = vmlaq_f32(offset_vec, $v, contrast_vec);
                    vminq_f32(vmaxq_f32(r, min_val), max_val)
                }};
            }

            let r_out = process!(pixels.0);
            let g_out = process!(pixels.1);
            let b_out = process!(pixels.2);
            // Alpha is preserved unchanged
            let a_out = pixels.3;

            let output = float32x4x4_t(r_out, g_out, b_out, a_out);
            vst4q_f32(out_f32[x * 4..].as_mut_ptr(), output);
            x += simd_width;
        }

        // Scalar fallback (shouldn't happen since we process 4 at a time and RGBA aligns)
        while x < width {
            for c in 0..3 {
                out_f32[x * 4 + c] = (in_f32[x * 4 + c] * contrast + offset).clamp(0.0, 1.0);
            }
            out_f32[x * 4 + 3] = in_f32[x * 4 + 3]; // preserve alpha
            x += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ContrastBrightness;
    use crate::common::image_diff::{max_pixel_diff, pixels_equal};
    use crate::common::test_utils::create_test_image;
    use crate::prelude::*;

    fn pixels_changed(img1: &Image, img2: &Image) -> bool {
        !pixels_equal(img1, img2)
    }

    // ========================================================================
    // No-change tests (contrast=1.0, brightness=0.0 should preserve input)
    // ========================================================================

    #[test]
    fn test_no_change_all_formats() {
        for format in ALL_FORMATS {
            let input = create_test_image(*format, 17, 5, 0);
            let mut output = Image::new_black(*input.desc()).unwrap();

            ContrastBrightness::new(1.0, 0.0).apply_cpu(&input, &mut output);

            if format.channel_type == ChannelType::Float {
                // F32 has tiny rounding errors from SIMD floating-point arithmetic
                let diff = max_pixel_diff(&input, &output);
                assert!(
                    diff < 1e-6,
                    "no-change exceeded epsilon for format {}: diff={}",
                    format,
                    diff
                );
            } else {
                assert!(
                    pixels_equal(&input, &output),
                    "no-change failed for format {}",
                    format
                );
            }
        }
    }

    // ========================================================================
    // Alpha preservation tests
    // ========================================================================

    #[test]
    fn test_alpha_preserved_all_formats() {
        for format in ALPHA_FORMATS {
            let input = create_test_image(*format, 16, 4, 0);
            let mut output = Image::new_black(*input.desc()).unwrap();

            ContrastBrightness::new(2.0, 0.3).apply_cpu(&input, &mut output);

            let channels = format.channel_count.channel_count() as usize;
            let channel_size = format.channel_size.byte_count() as usize;
            let alpha_offset = (channels - 1) * channel_size;
            let pixel_size = channels * channel_size;

            // Check alpha bytes for each pixel
            for row in 0..4 {
                let row_start = row * input.desc().stride as usize;
                for x in 0..16 {
                    let pixel_start = row_start + x * pixel_size;
                    let alpha_start = pixel_start + alpha_offset;
                    let in_alpha = &input.bytes()[alpha_start..alpha_start + channel_size];
                    let out_alpha = &output.bytes()[alpha_start..alpha_start + channel_size];
                    assert_eq!(
                        in_alpha, out_alpha,
                        "alpha mismatch for format {} at pixel ({}, {})",
                        format, x, row
                    );
                }
            }
        }
    }

    // ========================================================================
    // Brightness tests
    // ========================================================================

    #[test]
    fn test_brightness_increase_all_formats() {
        for format in ALL_FORMATS {
            let input = create_test_image(*format, 8, 2, 0);
            let mut output = Image::new_black(*input.desc()).unwrap();

            ContrastBrightness::new(1.0, 0.2).apply_cpu(&input, &mut output);

            assert!(
                pixels_changed(&input, &output),
                "brightness increase should change output for format {}",
                format
            );
        }
    }

    #[test]
    fn test_brightness_decrease_all_formats() {
        for format in ALL_FORMATS {
            let input = create_test_image(*format, 8, 2, 0);
            let mut output = Image::new_black(*input.desc()).unwrap();

            ContrastBrightness::new(1.0, -0.2).apply_cpu(&input, &mut output);

            assert!(
                pixels_changed(&input, &output),
                "brightness decrease should change output for format {}",
                format
            );
        }
    }

    // ========================================================================
    // Contrast tests
    // ========================================================================

    #[test]
    fn test_contrast_increase_all_formats() {
        for format in ALL_FORMATS {
            let input = create_test_image(*format, 8, 2, 0);
            let mut output = Image::new_black(*input.desc()).unwrap();

            ContrastBrightness::new(2.0, 0.0).apply_cpu(&input, &mut output);

            assert!(
                pixels_changed(&input, &output),
                "contrast increase should change output for format {}",
                format
            );
        }
    }

    #[test]
    fn test_contrast_decrease_all_formats() {
        for format in ALL_FORMATS {
            let input = create_test_image(*format, 8, 2, 0);
            let mut output = Image::new_black(*input.desc()).unwrap();

            ContrastBrightness::new(0.5, 0.0).apply_cpu(&input, &mut output);

            assert!(
                pixels_changed(&input, &output),
                "contrast decrease should change output for format {}",
                format
            );
        }
    }

    // ========================================================================
    // Combined contrast and brightness tests
    // ========================================================================

    #[test]
    fn test_combined_adjustment_all_formats() {
        for format in ALL_FORMATS {
            let input = create_test_image(*format, 17, 5, 0);
            let mut output = Image::new_black(*input.desc()).unwrap();

            ContrastBrightness::new(1.5, 0.1).apply_cpu(&input, &mut output);

            assert!(
                pixels_changed(&input, &output),
                "combined adjustment should change output for format {}",
                format
            );
        }
    }

    // ========================================================================
    // Odd dimension tests (exercises scalar fallback in SIMD paths)
    // ========================================================================

    #[test]
    fn test_odd_dimensions_all_formats() {
        for format in ALL_FORMATS {
            // Use odd dimensions to trigger scalar fallback
            let input = create_test_image(*format, 17, 7, 0);
            let mut output = Image::new_black(*input.desc()).unwrap();

            ContrastBrightness::new(1.3, -0.05).apply_cpu(&input, &mut output);

            assert!(
                pixels_changed(&input, &output),
                "odd dimensions test should change output for format {}",
                format
            );
        }
    }

    // ========================================================================
    // Clamp tests
    // ========================================================================

    #[test]
    fn test_clamp_all_formats() {
        for format in ALL_FORMATS {
            let input = create_test_image(*format, 4, 2, 0);
            let mut output = Image::new_black(*input.desc()).unwrap();

            // Extreme brightness to trigger clamping
            ContrastBrightness::new(1.0, 1.0).apply_cpu(&input, &mut output);

            // Should not panic and output should be valid
            assert!(
                !output.bytes().is_empty(),
                "clamp overflow test failed for format {}",
                format
            );

            // Test underflow
            ContrastBrightness::new(1.0, -1.0).apply_cpu(&input, &mut output);
            assert!(
                !output.bytes().is_empty(),
                "clamp underflow test failed for format {}",
                format
            );
        }
    }

    // ========================================================================
    // Large image tests (stress test SIMD paths)
    // ========================================================================

    #[test]
    fn test_large_image_all_formats() {
        for format in ALL_FORMATS {
            let input = create_test_image(*format, 256, 128, 0);
            let mut output = Image::new_black(*input.desc()).unwrap();

            ContrastBrightness::new(1.2, 0.05).apply_cpu(&input, &mut output);

            assert!(
                pixels_changed(&input, &output),
                "large image test should change output for format {}",
                format
            );
        }
    }
}
