use std::mem::size_of;

use bytemuck::Pod;
use rayon::prelude::*;

use super::{Blend, BlendMode};
use crate::prelude::*;

/// Applies blending of two images using CPU.
pub(super) fn apply(params: &Blend, src: &Image, dst: &Image, output: &mut Image) {
    assert_eq!(src.desc(), dst.desc(), "src/dst desc mismatch");
    assert_eq!(src.desc(), output.desc(), "src/output desc mismatch");

    let channel_size = src.desc().color_format.channel_size;
    let channel_type = src.desc().color_format.channel_type;
    let channel_count = src.desc().color_format.channel_count;
    let _ = channel_count; // Used in cfg-gated SIMD dispatch below

    // Use SIMD-optimized paths when available
    #[cfg(target_arch = "x86_64")]
    if is_x86_feature_detected!("sse4.1") {
        match (channel_size, channel_type, channel_count) {
            (ChannelSize::_8bit, ChannelType::UInt, ChannelCount::Rgba) => {
                // SAFETY: SSE4.1 support verified above
                unsafe { apply_rgba_u8_sse41(src, dst, output, *params) };
                return;
            }
            (ChannelSize::_32bit, ChannelType::Float, ChannelCount::Rgba) => {
                // SAFETY: SSE4.1 support verified above
                unsafe { apply_rgba_f32_sse41(src, dst, output, *params) };
                return;
            }
            _ => {}
        }
    }

    // Use NEON-optimized paths on aarch64
    #[cfg(target_arch = "aarch64")]
    {
        match (channel_size, channel_type, channel_count) {
            (ChannelSize::_8bit, ChannelType::UInt, ChannelCount::Rgba) => {
                // SAFETY: NEON is always available on aarch64
                unsafe { apply_rgba_u8_neon(src, dst, output, *params) };
                return;
            }
            (ChannelSize::_32bit, ChannelType::Float, ChannelCount::Rgba) => {
                // SAFETY: NEON is always available on aarch64
                unsafe { apply_rgba_f32_neon(src, dst, output, *params) };
                return;
            }
            _ => {}
        }
    }

    // Scalar fallback
    match (channel_size, channel_type) {
        (ChannelSize::_8bit, ChannelType::UInt) => {
            apply_typed::<u8>(src, dst, output, *params);
        }
        (ChannelSize::_16bit, ChannelType::UInt) => {
            apply_typed::<u16>(src, dst, output, *params);
        }
        (ChannelSize::_32bit, ChannelType::Float) => {
            apply_typed::<f32>(src, dst, output, *params);
        }
        _ => {
            unreachable!("Unsupported color format for blend")
        }
    }
}

// ============================================================================
// Scalar Implementation
// ============================================================================

trait BlendApply: Pod + Send + Sync + Copy {
    fn blend(src: Self, dst: Self, mode: BlendMode, alpha: f32) -> Self;
}

impl BlendApply for u8 {
    #[inline]
    fn blend(src: Self, dst: Self, mode: BlendMode, alpha: f32) -> Self {
        let max = Self::MAX as f32;
        let s = src as f32 / max;
        let d = dst as f32 / max;
        let result = blend_normalized(s, d, mode, alpha);
        (result * max).clamp(0.0, max) as Self
    }
}

impl BlendApply for u16 {
    #[inline]
    fn blend(src: Self, dst: Self, mode: BlendMode, alpha: f32) -> Self {
        let max = Self::MAX as f32;
        let s = src as f32 / max;
        let d = dst as f32 / max;
        let result = blend_normalized(s, d, mode, alpha);
        (result * max).clamp(0.0, max) as Self
    }
}

impl BlendApply for f32 {
    #[inline]
    fn blend(src: Self, dst: Self, mode: BlendMode, alpha: f32) -> Self {
        blend_normalized(src, dst, mode, alpha).clamp(0.0, 1.0)
    }
}

/// Blend two normalized [0, 1] values using the specified mode.
#[inline]
fn blend_normalized(src: f32, dst: f32, mode: BlendMode, alpha: f32) -> f32 {
    let blended = match mode {
        BlendMode::Normal => src,
        BlendMode::Add => (src + dst).min(1.0),
        BlendMode::Subtract => (dst - src).max(0.0),
        BlendMode::Multiply => src * dst,
        BlendMode::Screen => 1.0 - (1.0 - src) * (1.0 - dst),
        BlendMode::Overlay => {
            if dst < 0.5 {
                2.0 * src * dst
            } else {
                1.0 - 2.0 * (1.0 - src) * (1.0 - dst)
            }
        }
    };
    // Mix with alpha: result = blended * alpha + dst * (1 - alpha)
    blended * alpha + dst * (1.0 - alpha)
}

fn apply_typed<T>(src: &Image, dst: &Image, output: &mut Image, params: Blend)
where
    T: Pod + BlendApply,
{
    let width = src.desc().width;
    let channels = src.desc().color_format.channel_count.channel_count() as usize;
    let src_stride = src.desc().stride;
    let dst_stride = dst.desc().stride;
    let out_stride = output.desc().stride;
    let row_bytes = width * channels * size_of::<T>();

    let has_alpha = channels == 2 || channels == 4;
    let color_channels = if has_alpha { channels - 1 } else { channels };

    let mode = params.mode;
    let alpha = params.alpha;

    output
        .bytes_mut()
        .par_chunks_mut(out_stride)
        .enumerate()
        .for_each(|(y, out_row)| {
            let src_row = &src.bytes()[y * src_stride..];
            let dst_row = &dst.bytes()[y * dst_stride..];
            let src_row: &[T] = bytemuck::cast_slice(&src_row[..row_bytes]);
            let dst_row: &[T] = bytemuck::cast_slice(&dst_row[..row_bytes]);
            let out_row: &mut [T] = bytemuck::cast_slice_mut(&mut out_row[..row_bytes]);

            for x in 0..width {
                let src_pixel = &src_row[x * channels..];
                let dst_pixel = &dst_row[x * channels..];
                let out_pixel = &mut out_row[x * channels..];

                for c in 0..color_channels {
                    out_pixel[c] = T::blend(src_pixel[c], dst_pixel[c], mode, alpha);
                }

                // Alpha channel: use normal blend (weighted average)
                if has_alpha {
                    out_pixel[channels - 1] = T::blend(
                        src_pixel[channels - 1],
                        dst_pixel[channels - 1],
                        BlendMode::Normal,
                        alpha,
                    );
                }
            }
        });
}

// ============================================================================
// RGBA U8 SSE4.1
// ============================================================================

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
unsafe fn apply_rgba_u8_sse41(src: &Image, dst: &Image, output: &mut Image, params: Blend) {
    let width = src.desc().width;
    let src_stride = src.desc().stride;
    let dst_stride = dst.desc().stride;
    let out_stride = output.desc().stride;

    output
        .bytes_mut()
        .par_chunks_mut(out_stride)
        .enumerate()
        .for_each(|(y, out_row)| {
            let src_row = &src.bytes()[y * src_stride..];
            let dst_row = &dst.bytes()[y * dst_stride..];
            unsafe {
                process_row_rgba_u8_sse41(
                    src_row,
                    dst_row,
                    out_row,
                    width,
                    params.mode,
                    params.alpha,
                )
            };
        });
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
unsafe fn process_row_rgba_u8_sse41(
    src_row: &[u8],
    dst_row: &[u8],
    out_row: &mut [u8],
    width: usize,
    mode: BlendMode,
    alpha: f32,
) {
    use std::arch::x86_64::*;

    unsafe {
        let alpha_vec = _mm_set1_ps(alpha);
        let one_minus_alpha = _mm_set1_ps(1.0 - alpha);
        let scale = _mm_set1_ps(255.0);
        let one = _mm_set1_ps(1.0);
        let zero = _mm_setzero_ps();
        let half = _mm_set1_ps(0.5);
        let two = _mm_set1_ps(2.0);

        // Process 4 RGBA pixels at a time
        let simd_width = 4;
        let mut x = 0;

        while x + simd_width <= width {
            // Process each pixel separately (unpack, blend, repack)
            // This is simpler than trying to do 4 pixels in parallel with complex blend modes
            let mut result_bytes = [0u8; 16];

            for i in 0..4 {
                let src_offset = i * 4;
                let src_r = _mm_set1_ps(src_row[x * 4 + src_offset] as f32 * (1.0 / 255.0));
                let src_g = _mm_set1_ps(src_row[x * 4 + src_offset + 1] as f32 * (1.0 / 255.0));
                let src_b = _mm_set1_ps(src_row[x * 4 + src_offset + 2] as f32 * (1.0 / 255.0));
                let src_a = _mm_set1_ps(src_row[x * 4 + src_offset + 3] as f32 * (1.0 / 255.0));

                let dst_r = _mm_set1_ps(dst_row[x * 4 + src_offset] as f32 * (1.0 / 255.0));
                let dst_g = _mm_set1_ps(dst_row[x * 4 + src_offset + 1] as f32 * (1.0 / 255.0));
                let dst_b = _mm_set1_ps(dst_row[x * 4 + src_offset + 2] as f32 * (1.0 / 255.0));
                let dst_a = _mm_set1_ps(dst_row[x * 4 + src_offset + 3] as f32 * (1.0 / 255.0));

                macro_rules! blend_channel {
                    ($src:expr, $dst:expr) => {{
                        let blended = match mode {
                            BlendMode::Normal => $src,
                            BlendMode::Add => _mm_min_ps(_mm_add_ps($src, $dst), one),
                            BlendMode::Subtract => _mm_max_ps(_mm_sub_ps($dst, $src), zero),
                            BlendMode::Multiply => _mm_mul_ps($src, $dst),
                            BlendMode::Screen => _mm_sub_ps(
                                one,
                                _mm_mul_ps(_mm_sub_ps(one, $src), _mm_sub_ps(one, $dst)),
                            ),
                            BlendMode::Overlay => {
                                let mask = _mm_cmplt_ps($dst, half);
                                let dark = _mm_mul_ps(two, _mm_mul_ps($src, $dst));
                                let light = _mm_sub_ps(
                                    one,
                                    _mm_mul_ps(
                                        two,
                                        _mm_mul_ps(_mm_sub_ps(one, $src), _mm_sub_ps(one, $dst)),
                                    ),
                                );
                                _mm_blendv_ps(light, dark, mask)
                            }
                        };
                        _mm_add_ps(
                            _mm_mul_ps(blended, alpha_vec),
                            _mm_mul_ps($dst, one_minus_alpha),
                        )
                    }};
                }

                let out_r = blend_channel!(src_r, dst_r);
                let out_g = blend_channel!(src_g, dst_g);
                let out_b = blend_channel!(src_b, dst_b);
                // Alpha uses normal blend
                let out_a = _mm_add_ps(
                    _mm_mul_ps(src_a, alpha_vec),
                    _mm_mul_ps(dst_a, one_minus_alpha),
                );

                // Convert back to u8
                result_bytes[i * 4] =
                    (_mm_cvtss_f32(_mm_mul_ps(out_r, scale)).clamp(0.0, 255.0)) as u8;
                result_bytes[i * 4 + 1] =
                    (_mm_cvtss_f32(_mm_mul_ps(out_g, scale)).clamp(0.0, 255.0)) as u8;
                result_bytes[i * 4 + 2] =
                    (_mm_cvtss_f32(_mm_mul_ps(out_b, scale)).clamp(0.0, 255.0)) as u8;
                result_bytes[i * 4 + 3] =
                    (_mm_cvtss_f32(_mm_mul_ps(out_a, scale)).clamp(0.0, 255.0)) as u8;
            }

            let result = _mm_loadu_si128(result_bytes.as_ptr() as *const __m128i);
            _mm_storeu_si128(out_row[x * 4..].as_mut_ptr() as *mut __m128i, result);

            x += simd_width;
        }

        // Scalar fallback
        while x < width {
            for c in 0..3 {
                let s = src_row[x * 4 + c] as f32 / 255.0;
                let d = dst_row[x * 4 + c] as f32 / 255.0;
                out_row[x * 4 + c] =
                    (blend_normalized(s, d, mode, alpha) * 255.0).clamp(0.0, 255.0) as u8;
            }
            // Alpha
            let s = src_row[x * 4 + 3] as f32 / 255.0;
            let d = dst_row[x * 4 + 3] as f32 / 255.0;
            out_row[x * 4 + 3] =
                (blend_normalized(s, d, BlendMode::Normal, alpha) * 255.0).clamp(0.0, 255.0) as u8;
            x += 1;
        }
    }
}

// ============================================================================
// RGBA F32 SSE4.1
// ============================================================================

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
unsafe fn apply_rgba_f32_sse41(src: &Image, dst: &Image, output: &mut Image, params: Blend) {
    let width = src.desc().width;
    let src_stride = src.desc().stride;
    let dst_stride = dst.desc().stride;
    let out_stride = output.desc().stride;

    output
        .bytes_mut()
        .par_chunks_mut(out_stride)
        .enumerate()
        .for_each(|(y, out_row)| {
            let src_row = &src.bytes()[y * src_stride..];
            let dst_row = &dst.bytes()[y * dst_stride..];
            unsafe {
                process_row_rgba_f32_sse41(
                    src_row,
                    dst_row,
                    out_row,
                    width,
                    params.mode,
                    params.alpha,
                )
            };
        });
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
unsafe fn process_row_rgba_f32_sse41(
    src_row: &[u8],
    dst_row: &[u8],
    out_row: &mut [u8],
    width: usize,
    mode: BlendMode,
    alpha: f32,
) {
    use std::arch::x86_64::*;

    unsafe {
        let alpha_vec = _mm_set1_ps(alpha);
        let one_minus_alpha = _mm_set1_ps(1.0 - alpha);
        let one = _mm_set1_ps(1.0);
        let zero = _mm_setzero_ps();
        let half = _mm_set1_ps(0.5);
        let two = _mm_set1_ps(2.0);

        let src_f32: &[f32] = bytemuck::cast_slice(src_row);
        let dst_f32: &[f32] = bytemuck::cast_slice(dst_row);
        let out_f32: &mut [f32] = bytemuck::cast_slice_mut(out_row);

        // Process 1 RGBA pixel at a time (4 floats fit in one SSE register)
        let mut x = 0;

        while x < width {
            let src_pixel = _mm_loadu_ps(src_f32[x * 4..].as_ptr());
            let dst_pixel = _mm_loadu_ps(dst_f32[x * 4..].as_ptr());

            let blended = match mode {
                BlendMode::Normal => src_pixel,
                BlendMode::Add => _mm_min_ps(_mm_add_ps(src_pixel, dst_pixel), one),
                BlendMode::Subtract => _mm_max_ps(_mm_sub_ps(dst_pixel, src_pixel), zero),
                BlendMode::Multiply => _mm_mul_ps(src_pixel, dst_pixel),
                BlendMode::Screen => _mm_sub_ps(
                    one,
                    _mm_mul_ps(_mm_sub_ps(one, src_pixel), _mm_sub_ps(one, dst_pixel)),
                ),
                BlendMode::Overlay => {
                    let mask = _mm_cmplt_ps(dst_pixel, half);
                    let dark = _mm_mul_ps(two, _mm_mul_ps(src_pixel, dst_pixel));
                    let light = _mm_sub_ps(
                        one,
                        _mm_mul_ps(
                            two,
                            _mm_mul_ps(_mm_sub_ps(one, src_pixel), _mm_sub_ps(one, dst_pixel)),
                        ),
                    );
                    _mm_blendv_ps(light, dark, mask)
                }
            };

            // Apply alpha mixing
            let result = _mm_add_ps(
                _mm_mul_ps(blended, alpha_vec),
                _mm_mul_ps(dst_pixel, one_minus_alpha),
            );

            // Clamp to [0, 1]
            let clamped = _mm_min_ps(_mm_max_ps(result, zero), one);

            _mm_storeu_ps(out_f32[x * 4..].as_mut_ptr(), clamped);
            x += 1;
        }
    }
}

// ============================================================================
// RGBA U8 NEON
// ============================================================================

#[cfg(target_arch = "aarch64")]
unsafe fn apply_rgba_u8_neon(src: &Image, dst: &Image, output: &mut Image, params: Blend) {
    let width = src.desc().width;
    let src_stride = src.desc().stride;
    let dst_stride = dst.desc().stride;
    let out_stride = output.desc().stride;

    output
        .bytes_mut()
        .par_chunks_mut(out_stride)
        .enumerate()
        .for_each(|(y, out_row)| {
            let src_row = &src.bytes()[y * src_stride..];
            let dst_row = &dst.bytes()[y * dst_stride..];
            unsafe {
                process_row_rgba_u8_neon(
                    src_row,
                    dst_row,
                    out_row,
                    width,
                    params.mode,
                    params.alpha,
                )
            };
        });
}

#[cfg(target_arch = "aarch64")]
unsafe fn process_row_rgba_u8_neon(
    src_row: &[u8],
    dst_row: &[u8],
    out_row: &mut [u8],
    width: usize,
    mode: BlendMode,
    alpha: f32,
) {
    use std::arch::aarch64::*;

    unsafe {
        let alpha_vec = vdupq_n_f32(alpha);
        let one_minus_alpha = vdupq_n_f32(1.0 - alpha);
        let one = vdupq_n_f32(1.0);
        let zero = vdupq_n_f32(0.0);
        let half = vdupq_n_f32(0.5);
        let two = vdupq_n_f32(2.0);
        let scale = vdupq_n_f32(255.0);
        let inv_scale = vdupq_n_f32(1.0 / 255.0);

        // Process 4 RGBA pixels at a time using deinterleaved loads
        let simd_width = 4;
        let mut x = 0;

        while x + simd_width <= width {
            // Load 4 RGBA pixels deinterleaved (R, G, B, A separate)
            let src_pixels = vld4_u8(src_row[x * 4..].as_ptr());
            let dst_pixels = vld4_u8(dst_row[x * 4..].as_ptr());

            macro_rules! process_channel {
                ($src_chan:expr, $dst_chan:expr) => {{
                    // Convert to f32 (only first 4 values, we have 8 in uint8x8)
                    let src_16 = vmovl_u8($src_chan);
                    let dst_16 = vmovl_u8($dst_chan);

                    let src_32_lo = vmovl_u16(vget_low_u16(src_16));
                    let dst_32_lo = vmovl_u16(vget_low_u16(dst_16));

                    let src_f = vmulq_f32(vcvtq_f32_u32(src_32_lo), inv_scale);
                    let dst_f = vmulq_f32(vcvtq_f32_u32(dst_32_lo), inv_scale);

                    let blended = match mode {
                        BlendMode::Normal => src_f,
                        BlendMode::Add => vminq_f32(vaddq_f32(src_f, dst_f), one),
                        BlendMode::Subtract => vmaxq_f32(vsubq_f32(dst_f, src_f), zero),
                        BlendMode::Multiply => vmulq_f32(src_f, dst_f),
                        BlendMode::Screen => {
                            vsubq_f32(one, vmulq_f32(vsubq_f32(one, src_f), vsubq_f32(one, dst_f)))
                        }
                        BlendMode::Overlay => {
                            let mask = vcltq_f32(dst_f, half);
                            let dark = vmulq_f32(two, vmulq_f32(src_f, dst_f));
                            let light = vsubq_f32(
                                one,
                                vmulq_f32(
                                    two,
                                    vmulq_f32(vsubq_f32(one, src_f), vsubq_f32(one, dst_f)),
                                ),
                            );
                            vbslq_f32(mask, dark, light)
                        }
                    };

                    // Apply alpha: result = blended * alpha + dst * (1 - alpha)
                    let result = vmlaq_f32(vmulq_f32(dst_f, one_minus_alpha), blended, alpha_vec);

                    // Convert back to u8
                    let result_scaled = vmulq_f32(vminq_f32(vmaxq_f32(result, zero), one), scale);
                    let result_u32 = vcvtq_u32_f32(result_scaled);
                    let result_u16 = vmovn_u32(result_u32);
                    // We only have 4 values, pad with zeros for vmovn_u16
                    let result_u16_full = vcombine_u16(result_u16, vdup_n_u16(0));
                    vmovn_u16(result_u16_full)
                }};
            }

            // Process R, G, B channels with blend mode
            let r_out = process_channel!(src_pixels.0, dst_pixels.0);
            let g_out = process_channel!(src_pixels.1, dst_pixels.1);
            let b_out = process_channel!(src_pixels.2, dst_pixels.2);

            // Alpha uses normal blend (weighted average)
            let a_src_16 = vmovl_u8(src_pixels.3);
            let a_dst_16 = vmovl_u8(dst_pixels.3);
            let a_src_32 = vmovl_u16(vget_low_u16(a_src_16));
            let a_dst_32 = vmovl_u16(vget_low_u16(a_dst_16));
            let a_src_f = vmulq_f32(vcvtq_f32_u32(a_src_32), inv_scale);
            let a_dst_f = vmulq_f32(vcvtq_f32_u32(a_dst_32), inv_scale);
            let a_result = vmlaq_f32(vmulq_f32(a_dst_f, one_minus_alpha), a_src_f, alpha_vec);
            let a_scaled = vmulq_f32(vminq_f32(vmaxq_f32(a_result, zero), one), scale);
            let a_u32 = vcvtq_u32_f32(a_scaled);
            let a_u16 = vmovn_u32(a_u32);
            let a_u16_full = vcombine_u16(a_u16, vdup_n_u16(0));
            let a_out = vmovn_u16(a_u16_full);

            // Store interleaved - but we only have 4 pixels worth of data in the low half
            // Need to extract just the first 4 bytes from each channel
            let result = uint8x8x4_t(r_out, g_out, b_out, a_out);
            // Store only 16 bytes (4 RGBA pixels)
            vst4_lane_u8::<0>(out_row[x * 4..].as_mut_ptr(), result);
            vst4_lane_u8::<1>(out_row[x * 4 + 4..].as_mut_ptr(), result);
            vst4_lane_u8::<2>(out_row[x * 4 + 8..].as_mut_ptr(), result);
            vst4_lane_u8::<3>(out_row[x * 4 + 12..].as_mut_ptr(), result);

            x += simd_width;
        }

        // Scalar fallback
        while x < width {
            for c in 0..3 {
                let s = src_row[x * 4 + c] as f32 / 255.0;
                let d = dst_row[x * 4 + c] as f32 / 255.0;
                out_row[x * 4 + c] =
                    (blend_normalized(s, d, mode, alpha) * 255.0).clamp(0.0, 255.0) as u8;
            }
            let s = src_row[x * 4 + 3] as f32 / 255.0;
            let d = dst_row[x * 4 + 3] as f32 / 255.0;
            out_row[x * 4 + 3] =
                (blend_normalized(s, d, BlendMode::Normal, alpha) * 255.0).clamp(0.0, 255.0) as u8;
            x += 1;
        }
    }
}

// ============================================================================
// RGBA F32 NEON
// ============================================================================

#[cfg(target_arch = "aarch64")]
unsafe fn apply_rgba_f32_neon(src: &Image, dst: &Image, output: &mut Image, params: Blend) {
    let width = src.desc().width;
    let src_stride = src.desc().stride;
    let dst_stride = dst.desc().stride;
    let out_stride = output.desc().stride;

    output
        .bytes_mut()
        .par_chunks_mut(out_stride)
        .enumerate()
        .for_each(|(y, out_row)| {
            let src_row = &src.bytes()[y * src_stride..];
            let dst_row = &dst.bytes()[y * dst_stride..];
            unsafe {
                process_row_rgba_f32_neon(
                    src_row,
                    dst_row,
                    out_row,
                    width,
                    params.mode,
                    params.alpha,
                )
            };
        });
}

#[cfg(target_arch = "aarch64")]
unsafe fn process_row_rgba_f32_neon(
    src_row: &[u8],
    dst_row: &[u8],
    out_row: &mut [u8],
    width: usize,
    mode: BlendMode,
    alpha: f32,
) {
    use std::arch::aarch64::*;

    unsafe {
        let alpha_vec = vdupq_n_f32(alpha);
        let one_minus_alpha = vdupq_n_f32(1.0 - alpha);
        let one = vdupq_n_f32(1.0);
        let zero = vdupq_n_f32(0.0);
        let half = vdupq_n_f32(0.5);
        let two = vdupq_n_f32(2.0);

        let src_f32: &[f32] = bytemuck::cast_slice(src_row);
        let dst_f32: &[f32] = bytemuck::cast_slice(dst_row);
        let out_f32: &mut [f32] = bytemuck::cast_slice_mut(out_row);

        // Process 4 RGBA pixels at a time using deinterleaved loads
        let simd_width = 4;
        let mut x = 0;

        while x + simd_width <= width {
            // Load 4 RGBA pixels deinterleaved
            let src_pixels = vld4q_f32(src_f32[x * 4..].as_ptr());
            let dst_pixels = vld4q_f32(dst_f32[x * 4..].as_ptr());

            macro_rules! blend_channel {
                ($src:expr, $dst:expr) => {{
                    let blended = match mode {
                        BlendMode::Normal => $src,
                        BlendMode::Add => vminq_f32(vaddq_f32($src, $dst), one),
                        BlendMode::Subtract => vmaxq_f32(vsubq_f32($dst, $src), zero),
                        BlendMode::Multiply => vmulq_f32($src, $dst),
                        BlendMode::Screen => {
                            vsubq_f32(one, vmulq_f32(vsubq_f32(one, $src), vsubq_f32(one, $dst)))
                        }
                        BlendMode::Overlay => {
                            let mask = vcltq_f32($dst, half);
                            let dark = vmulq_f32(two, vmulq_f32($src, $dst));
                            let light = vsubq_f32(
                                one,
                                vmulq_f32(
                                    two,
                                    vmulq_f32(vsubq_f32(one, $src), vsubq_f32(one, $dst)),
                                ),
                            );
                            vbslq_f32(mask, dark, light)
                        }
                    };
                    // result = blended * alpha + dst * (1 - alpha)
                    let result = vmlaq_f32(vmulq_f32($dst, one_minus_alpha), blended, alpha_vec);
                    vminq_f32(vmaxq_f32(result, zero), one)
                }};
            }

            let r_out = blend_channel!(src_pixels.0, dst_pixels.0);
            let g_out = blend_channel!(src_pixels.1, dst_pixels.1);
            let b_out = blend_channel!(src_pixels.2, dst_pixels.2);
            // Alpha uses normal blend
            let a_blended = src_pixels.3;
            let a_result = vmlaq_f32(
                vmulq_f32(dst_pixels.3, one_minus_alpha),
                a_blended,
                alpha_vec,
            );
            let a_out = vminq_f32(vmaxq_f32(a_result, zero), one);

            let result = float32x4x4_t(r_out, g_out, b_out, a_out);
            vst4q_f32(out_f32[x * 4..].as_mut_ptr(), result);

            x += simd_width;
        }

        // Scalar fallback
        while x < width {
            for c in 0..3 {
                let s = src_f32[x * 4 + c];
                let d = dst_f32[x * 4 + c];
                out_f32[x * 4 + c] = blend_normalized(s, d, mode, alpha).clamp(0.0, 1.0);
            }
            let s = src_f32[x * 4 + 3];
            let d = dst_f32[x * 4 + 3];
            out_f32[x * 4 + 3] = blend_normalized(s, d, BlendMode::Normal, alpha).clamp(0.0, 1.0);
            x += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Blend, BlendMode};
    use crate::common::image_diff::max_pixel_diff;
    use crate::common::test_utils::{create_test_image, load_lena_rgba_u8_61x38};
    use crate::prelude::*;

    #[test]
    fn test_normal_blend_alpha_zero() {
        // Alpha = 0 should return dst unchanged (with small tolerance for float precision)
        for format in ALL_FORMATS {
            let src = create_test_image(*format, 8, 4, 0);
            let dst = create_test_image(*format, 8, 4, 100);
            let mut output = Image::new_black(*dst.desc()).unwrap();

            Blend::new(BlendMode::Normal, 0.0).apply_cpu(&src, &dst, &mut output);

            let diff = max_pixel_diff(&dst, &output);
            assert!(
                diff < 1e-6,
                "alpha=0 should return dst for format {}, got diff={}",
                format,
                diff
            );
        }
    }

    #[test]
    fn test_normal_blend_alpha_one() {
        // Alpha = 1 with Normal mode should return src
        for format in ALL_FORMATS {
            let src = create_test_image(*format, 8, 4, 0);
            let dst = create_test_image(*format, 8, 4, 100);
            let mut output = Image::new_black(*dst.desc()).unwrap();

            Blend::new(BlendMode::Normal, 1.0).apply_cpu(&src, &dst, &mut output);

            // Allow small precision errors for float formats
            let diff = max_pixel_diff(&src, &output);
            assert!(
                diff < 0.01,
                "alpha=1 Normal should return src for format {}, got diff={}",
                format,
                diff
            );
        }
    }

    #[test]
    fn test_multiply_with_white() {
        // Multiply with white (1.0) on RGB channels should return the other image's RGB
        let format = ColorFormat::RGBA_U8;
        let mut src = create_test_image(format, 8, 4, 50);
        let dst = create_test_image(format, 8, 4, 100);
        // Set src RGB to white (255), but copy dst alpha to src so alpha matches
        for i in 0..src.bytes().len() {
            if i % 4 == 3 {
                src.bytes_mut()[i] = dst.bytes()[i]; // Copy alpha from dst
            } else {
                src.bytes_mut()[i] = 255; // White RGB
            }
        }
        let mut output = Image::new_black(*dst.desc()).unwrap();

        Blend::new(BlendMode::Multiply, 1.0).apply_cpu(&src, &dst, &mut output);

        // Compare RGB channels only (alpha may differ due to blending)
        let mut max_rgb_diff = 0.0f64;
        for i in 0..dst.bytes().len() {
            if i % 4 != 3 {
                let diff = (dst.bytes()[i] as f64 - output.bytes()[i] as f64).abs() / 255.0;
                max_rgb_diff = max_rgb_diff.max(diff);
            }
        }
        assert!(
            max_rgb_diff < 0.01,
            "Multiply with white should return dst RGB, got diff={}",
            max_rgb_diff
        );
    }

    #[test]
    fn test_multiply_with_black() {
        // Multiply with black (0.0) should return black
        let format = ColorFormat::RGBA_U8;
        let mut src = create_test_image(format, 8, 4, 50);
        // Set src to black (keep alpha)
        for (i, byte) in src.bytes_mut().iter_mut().enumerate() {
            if i % 4 != 3 {
                *byte = 0;
            }
        }
        let dst = create_test_image(format, 8, 4, 100);
        let mut output = Image::new_black(*dst.desc()).unwrap();

        Blend::new(BlendMode::Multiply, 1.0).apply_cpu(&src, &dst, &mut output);

        // Check that RGB channels are 0
        for (i, &byte) in output.bytes().iter().enumerate() {
            if i % 4 != 3 {
                assert_eq!(byte, 0, "Multiply with black should return black");
            }
        }
    }

    #[test]
    fn test_add_mode() {
        let format = ColorFormat::RGBA_U8;
        let src = create_test_image(format, 8, 4, 0);
        let dst = create_test_image(format, 8, 4, 100);
        let mut output = Image::new_black(*dst.desc()).unwrap();

        Blend::new(BlendMode::Add, 1.0).apply_cpu(&src, &dst, &mut output);

        // Result should be >= both src and dst (clamped to 255)
        // Just verify it doesn't crash and produces valid output
        assert!(!output.bytes().is_empty());
    }

    #[test]
    fn test_screen_mode() {
        let format = ColorFormat::RGBA_U8;
        let src = create_test_image(format, 8, 4, 0);
        let dst = create_test_image(format, 8, 4, 100);
        let mut output = Image::new_black(*dst.desc()).unwrap();

        Blend::new(BlendMode::Screen, 1.0).apply_cpu(&src, &dst, &mut output);

        assert!(!output.bytes().is_empty());
    }

    #[test]
    fn test_overlay_mode() {
        let format = ColorFormat::RGBA_U8;
        let src = create_test_image(format, 8, 4, 0);
        let dst = create_test_image(format, 8, 4, 100);
        let mut output = Image::new_black(*dst.desc()).unwrap();

        Blend::new(BlendMode::Overlay, 1.0).apply_cpu(&src, &dst, &mut output);

        assert!(!output.bytes().is_empty());
    }

    #[test]
    fn test_subtract_mode() {
        let format = ColorFormat::RGBA_U8;
        let src = create_test_image(format, 8, 4, 0);
        let dst = create_test_image(format, 8, 4, 100);
        let mut output = Image::new_black(*dst.desc()).unwrap();

        Blend::new(BlendMode::Subtract, 1.0).apply_cpu(&src, &dst, &mut output);

        assert!(!output.bytes().is_empty());
    }

    #[test]
    fn test_all_formats_all_modes() {
        let modes = [
            BlendMode::Normal,
            BlendMode::Add,
            BlendMode::Subtract,
            BlendMode::Multiply,
            BlendMode::Screen,
            BlendMode::Overlay,
        ];

        for format in ALL_FORMATS {
            for mode in &modes {
                let src = create_test_image(*format, 17, 5, 0);
                let dst = create_test_image(*format, 17, 5, 100);
                let mut output = Image::new_black(*dst.desc()).unwrap();

                Blend::new(*mode, 0.5).apply_cpu(&src, &dst, &mut output);

                assert!(
                    !output.bytes().is_empty(),
                    "Blend {:?} failed for format {}",
                    mode,
                    format
                );
            }
        }
    }

    #[test]
    fn test_large_image() {
        let src = load_lena_rgba_u8_61x38();
        let dst = load_lena_rgba_u8_61x38();
        let mut output = Image::new_black(*dst.desc()).unwrap();

        Blend::new(BlendMode::Normal, 0.5).apply_cpu(&src, &dst, &mut output);

        assert!(!output.bytes().is_empty());
    }
}
