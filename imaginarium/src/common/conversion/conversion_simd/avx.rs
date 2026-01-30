// AVX2 implementations for row conversion
//
// This module contains AVX2 SIMD implementations for x86_64.

#![allow(unsafe_op_in_unsafe_fn)]

use super::sse;

// =============================================================================
// AVX2 implementations
// =============================================================================

#[target_feature(enable = "avx2")]
pub(super) unsafe fn convert_rgba_to_rgb_row_avx2(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::x86_64::*;

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();
    let simd_width = width / 32;
    let remainder = width % 32;

    // Process 32 pixels at a time (128 bytes in, 96 bytes out)
    let shuffle = _mm256_setr_epi8(
        0, 1, 2, 4, 5, 6, 8, 9, 10, 12, 13, 14, -1, -1, -1, -1, 0, 1, 2, 4, 5, 6, 8, 9, 10, 12, 13,
        14, -1, -1, -1, -1,
    );

    for i in 0..simd_width {
        let src_offset = i * 128;
        let dst_offset = i * 96;

        // Load 128 bytes (32 RGBA pixels)
        let rgba0 = _mm256_loadu_si256(src_ptr.add(src_offset) as *const __m256i);
        let rgba1 = _mm256_loadu_si256(src_ptr.add(src_offset + 32) as *const __m256i);
        let rgba2 = _mm256_loadu_si256(src_ptr.add(src_offset + 64) as *const __m256i);
        let rgba3 = _mm256_loadu_si256(src_ptr.add(src_offset + 96) as *const __m256i);

        // Shuffle each 256-bit register to get RGB (24 bytes valid per 32-byte register)
        let rgb0 = _mm256_shuffle_epi8(rgba0, shuffle);
        let rgb1 = _mm256_shuffle_epi8(rgba1, shuffle);
        let rgb2 = _mm256_shuffle_epi8(rgba2, shuffle);
        let rgb3 = _mm256_shuffle_epi8(rgba3, shuffle);

        // Extract 128-bit lanes and pack properly
        let rgb0_lo = _mm256_castsi256_si128(rgb0);
        let rgb0_hi = _mm256_extracti128_si256(rgb0, 1);
        let rgb1_lo = _mm256_castsi256_si128(rgb1);
        let rgb1_hi = _mm256_extracti128_si256(rgb1, 1);
        let rgb2_lo = _mm256_castsi256_si128(rgb2);
        let rgb2_hi = _mm256_extracti128_si256(rgb2, 1);
        let rgb3_lo = _mm256_castsi256_si128(rgb3);
        let rgb3_hi = _mm256_extracti128_si256(rgb3, 1);

        // Pack into output (complex interleaving)
        // Each shuffled lane has 12 valid bytes at positions 0-11

        // First output: rgb0_lo[0-11] + rgb0_hi[0-3] -> 16 bytes
        let out0 = _mm_or_si128(rgb0_lo, _mm_slli_si128(rgb0_hi, 12));
        // Second: rgb0_hi[4-11] + rgb1_lo[0-7] -> 16 bytes
        let out1 = _mm_or_si128(_mm_srli_si128(rgb0_hi, 4), _mm_slli_si128(rgb1_lo, 8));
        // Third: rgb1_lo[8-11] + rgb1_hi[0-11] -> 16 bytes
        let out2 = _mm_or_si128(_mm_srli_si128(rgb1_lo, 8), _mm_slli_si128(rgb1_hi, 4));

        // Continue for remaining 48 bytes
        let out3 = _mm_or_si128(rgb2_lo, _mm_slli_si128(rgb2_hi, 12));
        let out4 = _mm_or_si128(_mm_srli_si128(rgb2_hi, 4), _mm_slli_si128(rgb3_lo, 8));
        let out5 = _mm_or_si128(_mm_srli_si128(rgb3_lo, 8), _mm_slli_si128(rgb3_hi, 4));

        _mm_storeu_si128(dst_ptr.add(dst_offset) as *mut __m128i, out0);
        _mm_storeu_si128(dst_ptr.add(dst_offset + 16) as *mut __m128i, out1);
        _mm_storeu_si128(dst_ptr.add(dst_offset + 32) as *mut __m128i, out2);
        _mm_storeu_si128(dst_ptr.add(dst_offset + 48) as *mut __m128i, out3);
        _mm_storeu_si128(dst_ptr.add(dst_offset + 64) as *mut __m128i, out4);
        _mm_storeu_si128(dst_ptr.add(dst_offset + 80) as *mut __m128i, out5);
    }

    // Handle remainder with SSSE3
    if remainder > 0 {
        sse::convert_rgba_to_rgb_row_ssse3(
            &src[simd_width * 128..],
            &mut dst[simd_width * 96..],
            remainder,
        );
    }
}

#[target_feature(enable = "avx2")]
pub(super) unsafe fn convert_rgb_to_rgba_row_avx2(src: &[u8], dst: &mut [u8], width: usize) {
    // For simplicity, fall back to SSSE3 for this operation
    // AVX2 doesn't provide significant benefit for RGB->RGBA due to the complex packing
    sse::convert_rgb_to_rgba_row_ssse3(src, dst, width);
}

#[target_feature(enable = "avx2")]
pub(super) unsafe fn convert_rgba_to_l_row_avx2(src: &[u8], dst: &mut [u8], width: usize) {
    // Fall back to SSSE3 - AVX2 luminance conversion is complex
    sse::convert_rgba_to_l_row_ssse3(src, dst, width);
}

#[target_feature(enable = "avx2")]
pub(super) unsafe fn convert_rgb_to_l_row_avx2(src: &[u8], dst: &mut [u8], width: usize) {
    // Fall back to SSSE3
    sse::convert_rgb_to_l_row_ssse3(src, dst, width);
}

#[target_feature(enable = "avx2")]
pub(super) unsafe fn convert_l_to_rgba_row_avx2(src: &[u8], dst: &mut [u8], width: usize) {
    // Fall back to SSSE3 for simplicity
    sse::convert_l_to_rgba_row_ssse3(src, dst, width);
}

#[target_feature(enable = "avx2")]
pub(super) unsafe fn convert_l_to_rgb_row_avx2(src: &[u8], dst: &mut [u8], width: usize) {
    // Fall back to SSSE3
    sse::convert_l_to_rgb_row_ssse3(src, dst, width);
}

#[target_feature(enable = "avx2")]
pub(super) unsafe fn convert_la_to_rgba_row_avx2(src: &[u8], dst: &mut [u8], width: usize) {
    // Fall back to SSSE3
    sse::convert_la_to_rgba_row_ssse3(src, dst, width);
}

#[target_feature(enable = "avx2")]
pub(super) unsafe fn convert_rgba_to_la_row_avx2(src: &[u8], dst: &mut [u8], width: usize) {
    // Fall back to SSSE3
    sse::convert_rgba_to_la_row_ssse3(src, dst, width);
}

#[target_feature(enable = "avx2")]
pub(super) unsafe fn convert_f32_to_u8_row_avx2(src: &[f32], dst: &mut [u8]) {
    use std::arch::x86_64::*;

    let len = src.len();
    let simd_width = len / 32;
    let remainder = len % 32;

    let scale = _mm256_set1_ps(255.0);
    let zero_f = _mm256_setzero_ps();
    let max_f = _mm256_set1_ps(255.0);

    // Permutation to fix the order after packing:
    // After packs_epi32 and packus_epi16, we have (as 32-bit dwords):
    // [0-3, 8-11, 16-19, 24-27, 4-7, 12-15, 20-23, 28-31]
    // We need: [0-3, 4-7, 8-11, 12-15, 16-19, 20-23, 24-27, 28-31]
    // So permutation is: 0, 4, 1, 5, 2, 6, 3, 7
    let perm = _mm256_setr_epi32(0, 4, 1, 5, 2, 6, 3, 7);

    for i in 0..simd_width {
        let src_offset = i * 32;
        let dst_offset = i * 32;

        let f0 = _mm256_loadu_ps(src.as_ptr().add(src_offset));
        let f1 = _mm256_loadu_ps(src.as_ptr().add(src_offset + 8));
        let f2 = _mm256_loadu_ps(src.as_ptr().add(src_offset + 16));
        let f3 = _mm256_loadu_ps(src.as_ptr().add(src_offset + 24));

        let scaled0 = _mm256_min_ps(_mm256_max_ps(_mm256_mul_ps(f0, scale), zero_f), max_f);
        let scaled1 = _mm256_min_ps(_mm256_max_ps(_mm256_mul_ps(f1, scale), zero_f), max_f);
        let scaled2 = _mm256_min_ps(_mm256_max_ps(_mm256_mul_ps(f2, scale), zero_f), max_f);
        let scaled3 = _mm256_min_ps(_mm256_max_ps(_mm256_mul_ps(f3, scale), zero_f), max_f);

        let i0 = _mm256_cvtps_epi32(scaled0);
        let i1 = _mm256_cvtps_epi32(scaled1);
        let i2 = _mm256_cvtps_epi32(scaled2);
        let i3 = _mm256_cvtps_epi32(scaled3);

        let words_0 = _mm256_packs_epi32(i0, i1);
        let words_1 = _mm256_packs_epi32(i2, i3);
        let bytes = _mm256_packus_epi16(words_0, words_1);

        // Permute 32-bit dwords to get correct sequential order
        let bytes = _mm256_permutevar8x32_epi32(bytes, perm);

        _mm256_storeu_si256(dst.as_mut_ptr().add(dst_offset) as *mut __m256i, bytes);
    }

    // Handle remainder with SSE2
    if remainder > 0 {
        sse::convert_f32_to_u8_row_sse2(&src[simd_width * 32..], &mut dst[simd_width * 32..]);
    }
}

#[target_feature(enable = "avx2")]
pub(super) unsafe fn convert_u8_to_f32_row_avx2(src: &[u8], dst: &mut [f32]) {
    use std::arch::x86_64::*;

    let len = src.len();
    let simd_width = len / 32;
    let remainder = len % 32;

    let scale = _mm256_set1_ps(1.0 / 255.0);

    for i in 0..simd_width {
        let src_offset = i * 32;
        let dst_offset = i * 32;

        let bytes = _mm256_loadu_si256(src.as_ptr().add(src_offset) as *const __m256i);

        // Extract low and high 128-bit lanes
        let bytes_lo = _mm256_castsi256_si128(bytes);
        let bytes_hi = _mm256_extracti128_si256(bytes, 1);

        // Use AVX2 zero-extend instructions for efficient conversion
        let dwords_0 = _mm256_cvtepu8_epi32(bytes_lo);
        let dwords_1 = _mm256_cvtepu8_epi32(_mm_srli_si128(bytes_lo, 8));
        let dwords_2 = _mm256_cvtepu8_epi32(bytes_hi);
        let dwords_3 = _mm256_cvtepu8_epi32(_mm_srli_si128(bytes_hi, 8));

        // Convert to float and scale
        let floats_0 = _mm256_mul_ps(_mm256_cvtepi32_ps(dwords_0), scale);
        let floats_1 = _mm256_mul_ps(_mm256_cvtepi32_ps(dwords_1), scale);
        let floats_2 = _mm256_mul_ps(_mm256_cvtepi32_ps(dwords_2), scale);
        let floats_3 = _mm256_mul_ps(_mm256_cvtepi32_ps(dwords_3), scale);

        _mm256_storeu_ps(dst.as_mut_ptr().add(dst_offset), floats_0);
        _mm256_storeu_ps(dst.as_mut_ptr().add(dst_offset + 8), floats_1);
        _mm256_storeu_ps(dst.as_mut_ptr().add(dst_offset + 16), floats_2);
        _mm256_storeu_ps(dst.as_mut_ptr().add(dst_offset + 24), floats_3);
    }

    // Handle remainder with SSE2
    if remainder > 0 {
        sse::convert_u8_to_f32_row_sse2(&src[simd_width * 32..], &mut dst[simd_width * 32..]);
    }
}

#[target_feature(enable = "avx2")]
pub(super) unsafe fn convert_u8_to_u16_row_avx2(src: &[u8], dst: &mut [u16]) {
    use std::arch::x86_64::*;

    let len = src.len();
    let simd_width = len / 32;
    let remainder = len % 32;

    for i in 0..simd_width {
        let src_offset = i * 32;
        let dst_offset = i * 32;

        let bytes = _mm256_loadu_si256(src.as_ptr().add(src_offset) as *const __m256i);

        let bytes_lo = _mm256_castsi256_si128(bytes);
        let bytes_hi = _mm256_extracti128_si256(bytes, 1);

        let zero = _mm_setzero_si128();
        let words_0 = _mm_unpacklo_epi8(bytes_lo, zero);
        let words_1 = _mm_unpackhi_epi8(bytes_lo, zero);
        let words_2 = _mm_unpacklo_epi8(bytes_hi, zero);
        let words_3 = _mm_unpackhi_epi8(bytes_hi, zero);

        let scaled_0 = _mm_or_si128(words_0, _mm_slli_epi16(words_0, 8));
        let scaled_1 = _mm_or_si128(words_1, _mm_slli_epi16(words_1, 8));
        let scaled_2 = _mm_or_si128(words_2, _mm_slli_epi16(words_2, 8));
        let scaled_3 = _mm_or_si128(words_3, _mm_slli_epi16(words_3, 8));

        _mm_storeu_si128(dst.as_mut_ptr().add(dst_offset) as *mut __m128i, scaled_0);
        _mm_storeu_si128(
            dst.as_mut_ptr().add(dst_offset + 8) as *mut __m128i,
            scaled_1,
        );
        _mm_storeu_si128(
            dst.as_mut_ptr().add(dst_offset + 16) as *mut __m128i,
            scaled_2,
        );
        _mm_storeu_si128(
            dst.as_mut_ptr().add(dst_offset + 24) as *mut __m128i,
            scaled_3,
        );
    }

    if remainder > 0 {
        sse::convert_u8_to_u16_row_sse2(&src[simd_width * 32..], &mut dst[simd_width * 32..]);
    }
}

#[target_feature(enable = "avx2")]
pub(super) unsafe fn convert_u16_to_u8_row_avx2(src: &[u16], dst: &mut [u8]) {
    use std::arch::x86_64::*;

    let len = src.len();
    let simd_width = len / 32;
    let remainder = len % 32;

    for i in 0..simd_width {
        let src_offset = i * 32;
        let dst_offset = i * 32;

        let words_0 = _mm256_loadu_si256(src.as_ptr().add(src_offset) as *const __m256i);
        let words_1 = _mm256_loadu_si256(src.as_ptr().add(src_offset + 16) as *const __m256i);

        let shifted_0 = _mm256_srli_epi16(words_0, 8);
        let shifted_1 = _mm256_srli_epi16(words_1, 8);

        let bytes = _mm256_packus_epi16(shifted_0, shifted_1);
        let bytes = _mm256_permute4x64_epi64(bytes, 0b11011000);

        _mm256_storeu_si256(dst.as_mut_ptr().add(dst_offset) as *mut __m256i, bytes);
    }

    if remainder > 0 {
        sse::convert_u16_to_u8_row_sse2(&src[simd_width * 32..], &mut dst[simd_width * 32..]);
    }
}

#[target_feature(enable = "avx2")]
pub(super) unsafe fn convert_u16_to_f32_row_avx2(src: &[u16], dst: &mut [f32]) {
    use std::arch::x86_64::*;

    let len = src.len();
    let simd_width = len / 16;
    let remainder = len % 16;

    let scale = _mm256_set1_ps(1.0 / 65535.0);

    for i in 0..simd_width {
        let src_offset = i * 16;
        let dst_offset = i * 16;

        let words = _mm256_loadu_si256(src.as_ptr().add(src_offset) as *const __m256i);

        let words_lo = _mm256_castsi256_si128(words);
        let words_hi = _mm256_extracti128_si256(words, 1);

        let dwords_0 = _mm256_cvtepu16_epi32(words_lo);
        let dwords_1 = _mm256_cvtepu16_epi32(words_hi);

        let floats_0 = _mm256_mul_ps(_mm256_cvtepi32_ps(dwords_0), scale);
        let floats_1 = _mm256_mul_ps(_mm256_cvtepi32_ps(dwords_1), scale);

        _mm256_storeu_ps(dst.as_mut_ptr().add(dst_offset), floats_0);
        _mm256_storeu_ps(dst.as_mut_ptr().add(dst_offset + 8), floats_1);
    }

    if remainder > 0 {
        sse::convert_u16_to_f32_row_sse2(&src[simd_width * 16..], &mut dst[simd_width * 16..]);
    }
}

#[target_feature(enable = "avx2")]
pub(super) unsafe fn convert_f32_to_u16_row_avx2(src: &[f32], dst: &mut [u16]) {
    use std::arch::x86_64::*;

    let len = src.len();
    let simd_width = len / 16;
    let remainder = len % 16;

    let scale = _mm256_set1_ps(65535.0);
    let zero_f = _mm256_setzero_ps();
    let max_f = _mm256_set1_ps(65535.0);

    for i in 0..simd_width {
        let src_offset = i * 16;
        let dst_offset = i * 16;

        let f0 = _mm256_loadu_ps(src.as_ptr().add(src_offset));
        let f1 = _mm256_loadu_ps(src.as_ptr().add(src_offset + 8));

        let scaled0 = _mm256_min_ps(_mm256_max_ps(_mm256_mul_ps(f0, scale), zero_f), max_f);
        let scaled1 = _mm256_min_ps(_mm256_max_ps(_mm256_mul_ps(f1, scale), zero_f), max_f);

        let i0 = _mm256_cvtps_epi32(scaled0);
        let i1 = _mm256_cvtps_epi32(scaled1);

        // Pack to 16-bit
        let words = _mm256_packus_epi32(i0, i1);
        let words = _mm256_permute4x64_epi64(words, 0b11011000);

        _mm256_storeu_si256(dst.as_mut_ptr().add(dst_offset) as *mut __m256i, words);
    }

    if remainder > 0 {
        sse::convert_f32_to_u16_row_sse2(&src[simd_width * 16..], &mut dst[simd_width * 16..]);
    }
}
