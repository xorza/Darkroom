use std::mem::size_of;

use bytemuck::Pod;
use rayon::prelude::*;

use super::ContrastBrightness;
use crate::common::color_format::{ChannelCount, ChannelSize, ChannelType, ColorFormat};
#[cfg(target_arch = "x86_64")]
use crate::cpu_features;
use crate::image::Image;

/// A SIMD row kernel: `(in_row, out_row, width, contrast, last)` where `last` is the
/// format family's precomputed final argument — a fused offset for the u8 kernels, raw
/// brightness for the f32 kernels (they fuse the offset internally).
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
type RowKernel = unsafe fn(&[u8], &mut [u8], usize, f32, f32);

/// The SIMD row kernel for `format` on this arch (SSE4.1 / NEON), paired with its
/// `last` argument, or `None` when the format has no SIMD path (u16) or the CPU
/// lacks the feature — callers then fall back to the scalar path.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
fn simd_kernel(format: ColorFormat, params: &ContrastBrightness) -> Option<(RowKernel, f32)> {
    #[cfg(target_arch = "x86_64")]
    if !cpu_features::has_sse4_1() {
        return None;
    }

    let u8_offset = 127.5 * (1.0 - params.contrast) + params.brightness * 255.0;
    let key = (
        format.channel_size,
        format.channel_type,
        format.channel_count,
    );

    #[cfg(target_arch = "x86_64")]
    // SAFETY contract of each kernel: SSE4.1 support verified above.
    let kernel = match key {
        (ChannelSize::_8bit, ChannelType::UInt, ChannelCount::L) => {
            (process_row_u8_gray_sse41 as RowKernel, u8_offset)
        }
        (ChannelSize::_8bit, ChannelType::UInt, ChannelCount::Rgb) => {
            (process_row_u8_rgb_sse41 as RowKernel, u8_offset)
        }
        (ChannelSize::_8bit, ChannelType::UInt, ChannelCount::Rgba) => {
            (process_row_u8_rgba_sse41 as RowKernel, u8_offset)
        }
        (ChannelSize::_32bit, ChannelType::Float, ChannelCount::L) => {
            (process_row_f32_gray_sse41 as RowKernel, params.brightness)
        }
        (ChannelSize::_32bit, ChannelType::Float, ChannelCount::Rgb) => {
            (process_row_f32_rgb_sse41 as RowKernel, params.brightness)
        }
        (ChannelSize::_32bit, ChannelType::Float, ChannelCount::Rgba) => {
            (process_row_f32_rgba_sse41 as RowKernel, params.brightness)
        }
        _ => return None,
    };

    #[cfg(target_arch = "aarch64")]
    // SAFETY contract of each kernel: NEON is always available on aarch64.
    let kernel = match key {
        (ChannelSize::_8bit, ChannelType::UInt, ChannelCount::L) => {
            (process_row_u8_gray_neon as RowKernel, u8_offset)
        }
        (ChannelSize::_8bit, ChannelType::UInt, ChannelCount::Rgb) => {
            (process_row_u8_rgb_neon as RowKernel, u8_offset)
        }
        (ChannelSize::_8bit, ChannelType::UInt, ChannelCount::Rgba) => {
            (process_row_u8_rgba_neon as RowKernel, u8_offset)
        }
        (ChannelSize::_32bit, ChannelType::Float, ChannelCount::L) => {
            (process_row_f32_gray_neon as RowKernel, params.brightness)
        }
        (ChannelSize::_32bit, ChannelType::Float, ChannelCount::Rgb) => {
            (process_row_f32_rgb_neon as RowKernel, params.brightness)
        }
        (ChannelSize::_32bit, ChannelType::Float, ChannelCount::Rgba) => {
            (process_row_f32_rgba_neon as RowKernel, params.brightness)
        }
        _ => return None,
    };

    Some(kernel)
}

/// Applies contrast and brightness adjustment to an image in place using CPU. The
/// SIMD row kernels read and write distinct slices, so each row is bounced through a
/// small per-thread scratch (one row, reused across the thread's rows) — no
/// full-image allocation anywhere.
pub(super) fn apply(params: &ContrastBrightness, image: &mut Image) {
    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    if let Some((kernel, last)) = simd_kernel(image.desc().color_format, params) {
        let width = image.desc().width;
        let stride = image.desc().row_bytes();
        let contrast = params.contrast;
        image.bytes_mut().par_chunks_mut(stride).for_each_init(
            || vec![0u8; stride],
            |scratch, row| {
                scratch.copy_from_slice(row);
                // SAFETY: the kernel's feature requirement was verified by `simd_kernel`.
                unsafe { kernel(scratch, row, width, contrast, last) };
            },
        );
        return;
    }

    match (
        image.desc().color_format.channel_size,
        image.desc().color_format.channel_type,
    ) {
        (ChannelSize::_8bit, ChannelType::UInt) => {
            apply_typed::<u8>(image, *params);
        }
        (ChannelSize::_16bit, ChannelType::UInt) => {
            apply_typed::<u16>(image, *params);
        }
        (ChannelSize::_32bit, ChannelType::Float) => {
            apply_typed::<f32>(image, *params);
        }
        _ => {
            unreachable!("Unsupported color format for contrast/brightness")
        }
    }
}

pub(super) trait ContrastBrightnessApply: Pod + Send + Sync {
    fn apply(self, contrast: f32, brightness: f32) -> Self;
}

impl ContrastBrightnessApply for u8 {
    #[inline]
    fn apply(self, contrast: f32, brightness: f32) -> Self {
        let max = Self::MAX as f32;
        let mid = max / 2.0;
        let val = (self as f32 - mid) * contrast + mid + brightness * max;
        val.round_ties_even().clamp(0.0, max) as Self
    }
}

impl ContrastBrightnessApply for u16 {
    #[inline]
    fn apply(self, contrast: f32, brightness: f32) -> Self {
        let max = Self::MAX as f32;
        let mid = max / 2.0;
        let val = (self as f32 - mid) * contrast + mid + brightness * max;
        val.round_ties_even().clamp(0.0, max) as Self
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

/// The scalar reference: per-element in-place adjustment through
/// [`ContrastBrightnessApply`]. The u16 formats always take this path; the SIMD
/// formats fall back to it when the CPU lacks the feature, and the tests cross-check
/// the SIMD kernels against it.
pub(super) fn apply_typed<T>(image: &mut Image, params: ContrastBrightness)
where
    T: Pod + ContrastBrightnessApply,
{
    debug_assert_eq!(
        image.desc().color_format.channel_size.byte_count() as usize,
        size_of::<T>()
    );

    let width = image.desc().width;
    let channels = image.desc().color_format.channel_count.channel_count() as usize;
    let stride = image.desc().row_bytes();
    let row_bytes = width * channels * size_of::<T>();

    let has_alpha = channels == 2 || channels == 4;
    let color_channels = if has_alpha { channels - 1 } else { channels };

    let contrast = params.contrast;
    let brightness = params.brightness;

    image.bytes_mut().par_chunks_mut(stride).for_each(|row| {
        let row: &mut [T] = bytemuck::cast_slice_mut(&mut row[..row_bytes]);
        for pixel in row.chunks_exact_mut(channels) {
            for value in &mut pixel[..color_channels] {
                *value = value.apply(contrast, brightness);
            }
            // Alpha (if present) is left untouched.
        }
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
            out_row[x] = (in_row[x] as f32 * contrast + offset)
                .round_ties_even()
                .clamp(0.0, 255.0) as u8;
            x += 1;
        }
    }
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
                out_row[x * 3 + c] = (in_row[x * 3 + c] as f32 * contrast + offset)
                    .round_ties_even()
                    .clamp(0.0, 255.0) as u8;
            }
            x += 1;
        }
    }
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
                dst[c] = (src[c] as f32 * contrast + offset)
                    .round_ties_even()
                    .clamp(0.0, 255.0) as u8;
            }
            dst[3] = src[3];
            x += 1;
        }
    }
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
                    vcvtnq_u32_f32(vminq_f32(vmaxq_f32(r, min_val), max_val))
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
            out_row[x] = (in_row[x] as f32 * contrast + offset)
                .round_ties_even()
                .clamp(0.0, 255.0) as u8;
            x += 1;
        }
    }
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

                    let r0 = vcvtnq_u32_f32(vminq_f32(vmaxq_f32(r0, min_val), max_val));
                    let r1 = vcvtnq_u32_f32(vminq_f32(vmaxq_f32(r1, min_val), max_val));

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
                out_row[x * 3 + c] = (in_row[x * 3 + c] as f32 * contrast + offset)
                    .round_ties_even()
                    .clamp(0.0, 255.0) as u8;
            }
            x += 1;
        }
    }
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

                    let r0 = vcvtnq_u32_f32(vminq_f32(vmaxq_f32(r0, min_val), max_val));
                    let r1 = vcvtnq_u32_f32(vminq_f32(vmaxq_f32(r1, min_val), max_val));

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
                dst[c] = (src[c] as f32 * contrast + offset)
                    .round_ties_even()
                    .clamp(0.0, 255.0) as u8;
            }
            dst[3] = src[3];
            x += 1;
        }
    }
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
    use super::{ContrastBrightness, apply_typed};
    use crate::common::color_format::{ALL_FORMATS, ALPHA_FORMATS, ChannelSize, ChannelType};
    use crate::common::image_diff::{max_pixel_diff, pixels_equal};
    use crate::common::internals::{create_test_image, load_lena_rgba_u8_61x38};
    use crate::image::Image;

    fn pixels_changed(img1: &Image, img2: &Image) -> bool {
        !pixels_equal(img1, img2)
    }

    #[test]
    fn test_no_change_all_formats() {
        for format in ALL_FORMATS {
            let input = create_test_image(*format, 17, 5, 0);
            let mut output = input.clone();

            ContrastBrightness::new(1.0, 0.0).apply_cpu(&mut output);

            if format.channel_type == ChannelType::Float {
                // F32 has tiny rounding errors from SIMD floating-point arithmetic
                let diff = max_pixel_diff(&input, &output);
                assert!(
                    diff < 1e-6,
                    "no-change exceeded epsilon for format {format}: diff={diff}"
                );
            } else {
                assert!(
                    pixels_equal(&input, &output),
                    "no-change failed for format {format}"
                );
            }
        }
    }

    #[test]
    fn test_alpha_preserved_all_formats() {
        for format in ALPHA_FORMATS {
            let input = create_test_image(*format, 16, 4, 0);
            let mut output = input.clone();

            ContrastBrightness::new(2.0, 0.3).apply_cpu(&mut output);

            let channels = format.channel_count.channel_count() as usize;
            let channel_size = format.channel_size.byte_count() as usize;
            let alpha_offset = (channels - 1) * channel_size;
            let pixel_size = channels * channel_size;

            // Check alpha bytes for each pixel
            for row in 0..4 {
                let row_start = row * input.desc().row_bytes();
                for x in 0..16 {
                    let pixel_start = row_start + x * pixel_size;
                    let alpha_start = pixel_start + alpha_offset;
                    let in_alpha = &input.bytes()[alpha_start..alpha_start + channel_size];
                    let out_alpha = &output.bytes()[alpha_start..alpha_start + channel_size];
                    assert_eq!(
                        in_alpha, out_alpha,
                        "alpha mismatch for format {format} at pixel ({x}, {row})"
                    );
                }
            }
        }
    }

    #[test]
    fn test_brightness_increase_all_formats() {
        for format in ALL_FORMATS {
            let input = create_test_image(*format, 8, 2, 0);
            let mut output = input.clone();

            ContrastBrightness::new(1.0, 0.2).apply_cpu(&mut output);

            assert!(
                pixels_changed(&input, &output),
                "brightness increase should change output for format {format}"
            );
        }
    }

    #[test]
    fn test_brightness_decrease_all_formats() {
        for format in ALL_FORMATS {
            let input = create_test_image(*format, 8, 2, 0);
            let mut output = input.clone();

            ContrastBrightness::new(1.0, -0.2).apply_cpu(&mut output);

            assert!(
                pixels_changed(&input, &output),
                "brightness decrease should change output for format {format}"
            );
        }
    }

    #[test]
    fn test_contrast_increase_all_formats() {
        for format in ALL_FORMATS {
            let input = create_test_image(*format, 8, 2, 0);
            let mut output = input.clone();

            ContrastBrightness::new(2.0, 0.0).apply_cpu(&mut output);

            assert!(
                pixels_changed(&input, &output),
                "contrast increase should change output for format {format}"
            );
        }
    }

    #[test]
    fn test_contrast_decrease_all_formats() {
        for format in ALL_FORMATS {
            let input = create_test_image(*format, 8, 2, 0);
            let mut output = input.clone();

            ContrastBrightness::new(0.5, 0.0).apply_cpu(&mut output);

            assert!(
                pixels_changed(&input, &output),
                "contrast decrease should change output for format {format}"
            );
        }
    }

    #[test]
    fn test_combined_adjustment_all_formats() {
        for format in ALL_FORMATS {
            let input = create_test_image(*format, 17, 5, 0);
            let mut output = input.clone();

            ContrastBrightness::new(1.5, 0.1).apply_cpu(&mut output);

            assert!(
                pixels_changed(&input, &output),
                "combined adjustment should change output for format {format}"
            );
        }
    }

    #[test]
    fn test_odd_dimensions_all_formats() {
        for format in ALL_FORMATS {
            // Use odd dimensions to trigger scalar fallback
            let input = create_test_image(*format, 17, 7, 0);
            let mut output = input.clone();

            ContrastBrightness::new(1.3, -0.05).apply_cpu(&mut output);

            assert!(
                pixels_changed(&input, &output),
                "odd dimensions test should change output for format {format}"
            );
        }
    }

    #[test]
    fn test_clamp_all_formats() {
        for format in ALL_FORMATS {
            let input = create_test_image(*format, 4, 2, 0);
            let mut output = input.clone();

            // Extreme brightness to trigger clamping
            ContrastBrightness::new(1.0, 1.0).apply_cpu(&mut output);

            // Should not panic and output should be valid
            assert!(
                !output.bytes().is_empty(),
                "clamp overflow test failed for format {format}"
            );

            // Test underflow, again from the pristine input
            let mut output = input.clone();
            ContrastBrightness::new(1.0, -1.0).apply_cpu(&mut output);
            assert!(
                !output.bytes().is_empty(),
                "clamp underflow test failed for format {format}"
            );
        }
    }

    #[test]
    fn test_simd_matches_scalar_reference_all_formats() {
        // Sweep contrast-only, brightness-only, combined, and clamp-heavy params;
        // 17×5 exercises the SIMD tail (width not a multiple of any simd_width).
        for format in ALL_FORMATS {
            for (contrast, brightness) in [(2.0, 0.0), (1.0, 0.2), (1.5, 0.1), (0.5, -0.8)] {
                let input = create_test_image(*format, 17, 5, 0);
                let op = ContrastBrightness::new(contrast, brightness);

                // The public path picks the SIMD kernel when one exists.
                let mut actual = input.clone();
                op.apply_cpu(&mut actual);

                // The scalar reference, forced.
                let mut expected = input.clone();
                match (format.channel_size, format.channel_type) {
                    (ChannelSize::_8bit, ChannelType::UInt) => {
                        apply_typed::<u8>(&mut expected, op);
                    }
                    (ChannelSize::_16bit, ChannelType::UInt) => {
                        apply_typed::<u16>(&mut expected, op);
                    }
                    (ChannelSize::_32bit, ChannelType::Float) => {
                        apply_typed::<f32>(&mut expected, op);
                    }
                    _ => unreachable!("unsupported format in ALL_FORMATS"),
                }

                if format.channel_type == ChannelType::Float {
                    // The SIMD kernels fuse the offset (`v*c + o` vs the reference's
                    // `(v-mid)*c + mid + b`) and NEON's vmlaq is a fused multiply-add,
                    // so float results differ by ulps — same epsilon as the
                    // no-change test.
                    let diff = max_pixel_diff(&expected, &actual);
                    assert!(
                        diff < 1e-6,
                        "SIMD path diverges from the scalar reference for {format} \
                         (contrast={contrast}, brightness={brightness}): diff={diff}"
                    );
                } else {
                    assert!(
                        pixels_equal(&expected, &actual),
                        "SIMD path diverges from the scalar reference for {format} \
                         (contrast={contrast}, brightness={brightness})"
                    );
                }
                // Sanity: the sweep actually transformed the pixels.
                assert!(
                    pixels_changed(&input, &actual),
                    "params ({contrast}, {brightness}) left {format} unchanged"
                );
            }
        }
    }

    #[test]
    fn test_large_image() {
        let input = load_lena_rgba_u8_61x38();
        let mut output = input.clone();

        ContrastBrightness::new(1.2, 0.05).apply_cpu(&mut output);

        assert!(
            pixels_changed(&input, &output),
            "large image test should change output"
        );
    }
}
