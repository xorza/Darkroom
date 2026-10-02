//! AVX2 row conversion kernels for `x86_64`.

use std::arch::x86_64::{
    __m128, __m128i, __m256, __m256d, __m256i, _mm_or_si128, _mm_packs_epi32, _mm_packus_epi16,
    _mm_packus_epi32, _mm_slli_si128, _mm_srli_si128, _mm_storeu_si128, _mm256_add_epi16,
    _mm256_castps256_ps128, _mm256_castsi256_si128, _mm256_cvtepi32_ps, _mm256_cvtepu8_epi16,
    _mm256_cvtepu8_epi32, _mm256_cvtepu16_epi32, _mm256_cvtpd_epi32, _mm256_cvtps_pd,
    _mm256_div_ps, _mm256_extractf128_ps, _mm256_extracti128_si256, _mm256_loadu_ps,
    _mm256_loadu_si256, _mm256_max_pd, _mm256_min_pd, _mm256_mul_pd, _mm256_mulhi_epu16,
    _mm256_or_si256, _mm256_packus_epi16, _mm256_permute4x64_epi64, _mm256_set1_epi16,
    _mm256_set1_pd, _mm256_set1_ps, _mm256_setr_epi8, _mm256_setzero_pd, _mm256_shuffle_epi8,
    _mm256_slli_epi16, _mm256_srli_epi16, _mm256_storeu_ps, _mm256_storeu_si256,
};
use std::array;

use crate::image::conversion::scalar;
use crate::image::conversion::simd::ElementRow;

/// One 32-byte load.
#[inline]
#[target_feature(enable = "avx2")]
fn load(bytes: &[u8; 32]) -> __m256i {
    // SAFETY: the argument is exactly the 32 bytes the load reads.
    unsafe { _mm256_loadu_si256(bytes.as_ptr().cast()) }
}

/// One 32-byte store.
#[inline]
#[target_feature(enable = "avx2")]
fn store(bytes: &mut [u8; 32], value: __m256i) {
    // SAFETY: the argument is exactly the 32 bytes the store writes.
    unsafe { _mm256_storeu_si256(bytes.as_mut_ptr().cast(), value) }
}

/// One 16-byte store.
#[inline]
#[target_feature(enable = "avx2")]
fn store128(bytes: &mut [u8; 16], value: __m128i) {
    // SAFETY: the argument is exactly the 16 bytes the store writes.
    unsafe { _mm_storeu_si128(bytes.as_mut_ptr().cast(), value) }
}

/// Eight `f32` loaded.
#[inline]
#[target_feature(enable = "avx2")]
fn load_ps(values: &[f32; 8]) -> __m256 {
    // SAFETY: the argument is exactly the eight floats the load reads.
    unsafe { _mm256_loadu_ps(values.as_ptr()) }
}

/// Eight `f32` stored.
#[inline]
#[target_feature(enable = "avx2")]
fn store_ps(values: &mut [f32; 8], value: __m256) {
    // SAFETY: the argument is exactly the eight floats the store writes.
    unsafe { _mm256_storeu_ps(values.as_mut_ptr(), value) }
}

/// Thirty-two `RGBA_U8` pixels per iteration: four loads, six 16-byte stores.
#[target_feature(enable = "avx2")]
pub(super) unsafe fn convert_rgba_to_rgb_row_avx2(src: &[u8], dst: &mut [u8], width: usize) {
    // Within each 128-bit lane: the twelve colour bytes of four pixels, then four zeros.
    let shuffle = _mm256_setr_epi8(
        0, 1, 2, 4, 5, 6, 8, 9, 10, 12, 13, 14, -1, -1, -1, -1, 0, 1, 2, 4, 5, 6, 8, 9, 10, 12, 13,
        14, -1, -1, -1, -1,
    );
    let pixels = width - width % 32;
    let (groups, _) = src[..pixels * 4].as_chunks::<128>();
    let (out_groups, _) = dst[..pixels * 3].as_chunks_mut::<96>();

    for (group, out) in groups.iter().zip(out_groups) {
        let (parts, _) = group.as_chunks::<32>();
        // Eight twelve-byte runs, in pixel order, each at the bottom of its register.
        let runs: [__m128i; 8] = array::from_fn(|run| {
            let rgb = _mm256_shuffle_epi8(load(&parts[run / 2]), shuffle);
            if run % 2 == 0 {
                _mm256_castsi256_si128(rgb)
            } else {
                _mm256_extracti128_si256::<1>(rgb)
            }
        });
        let (out, _) = out.as_chunks_mut::<16>();
        for (half, out) in out.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            let [a, b, c, d] = [
                runs[4 * half],
                runs[4 * half + 1],
                runs[4 * half + 2],
                runs[4 * half + 3],
            ];
            store128(&mut out[0], _mm_or_si128(a, _mm_slli_si128::<12>(b)));
            store128(
                &mut out[1],
                _mm_or_si128(_mm_srli_si128::<4>(b), _mm_slli_si128::<8>(c)),
            );
            store128(
                &mut out[2],
                _mm_or_si128(_mm_srli_si128::<8>(c), _mm_slli_si128::<4>(d)),
            );
        }
    }

    scalar::convert_row::<u8, u8, 4, 3>(&src[pixels * 4..], &mut dst[pixels * 3..], width - pixels);
}

/// Four `f32` on `[0, 1]` scaled by `full_scale` and rounded as the reference does — the
/// four-lane `f64` form of the SSE2 `narrow4`, whose doc gives the reasoning.
#[inline]
#[target_feature(enable = "avx2")]
fn narrow4(values: __m128, full_scale: __m256d) -> __m128i {
    _mm256_cvtpd_epi32(_mm256_min_pd(
        _mm256_max_pd(
            _mm256_mul_pd(_mm256_cvtps_pd(values), full_scale),
            _mm256_setzero_pd(),
        ),
        full_scale,
    ))
}

/// Eight `f32` as two halves of four.
#[inline]
#[target_feature(enable = "avx2")]
fn halves(values: __m256) -> [__m128; 2] {
    [
        _mm256_castps256_ps128(values),
        _mm256_extractf128_ps::<1>(values),
    ]
}

/// Thirty-two samples per iteration.
#[target_feature(enable = "avx2")]
pub(super) unsafe fn convert_f32_to_u8_row_avx2(src: &[u8], dst: &mut [u8], _width: usize) {
    let ElementRow { src, dst } = ElementRow::<f32, u8>::new(src, dst);
    let full_scale = _mm256_set1_pd(255.0);
    let (groups, src_tail) = src.as_chunks::<32>();
    let (out_groups, dst_tail) = dst.as_chunks_mut::<32>();

    for (group, out) in groups.iter().zip(out_groups.iter_mut()) {
        let (octets, _) = group.as_chunks::<8>();
        let (out, _) = out.as_chunks_mut::<16>();
        for (pair, out) in octets.as_chunks::<2>().0.iter().zip(out) {
            let [a, b] = halves(load_ps(&pair[0]));
            let [c, d] = halves(load_ps(&pair[1]));
            let words = [
                _mm_packs_epi32(narrow4(a, full_scale), narrow4(b, full_scale)),
                _mm_packs_epi32(narrow4(c, full_scale), narrow4(d, full_scale)),
            ];
            store128(out, _mm_packus_epi16(words[0], words[1]));
        }
    }

    scalar::convert_elements(src_tail, dst_tail);
}

/// Thirty-two samples per iteration.
#[target_feature(enable = "avx2")]
pub(super) unsafe fn convert_u8_to_f32_row_avx2(src: &[u8], dst: &mut [u8], _width: usize) {
    let ElementRow { src, dst } = ElementRow::<u8, f32>::new(src, dst);
    // Divide, never a reciprocal multiply — see the module doc on precision.
    let divisor = _mm256_set1_ps(255.0);
    let (groups, src_tail) = src.as_chunks::<32>();
    let (out_groups, dst_tail) = dst.as_chunks_mut::<32>();

    for (group, out) in groups.iter().zip(out_groups.iter_mut()) {
        let bytes = load(group);
        let lo = _mm256_castsi256_si128(bytes);
        let hi = _mm256_extracti128_si256::<1>(bytes);
        let dwords = [
            _mm256_cvtepu8_epi32(lo),
            _mm256_cvtepu8_epi32(_mm_srli_si128::<8>(lo)),
            _mm256_cvtepu8_epi32(hi),
            _mm256_cvtepu8_epi32(_mm_srli_si128::<8>(hi)),
        ];
        let (out, _) = out.as_chunks_mut::<8>();
        for (out, dwords) in out.iter_mut().zip(dwords) {
            store_ps(out, _mm256_div_ps(_mm256_cvtepi32_ps(dwords), divisor));
        }
    }

    scalar::convert_elements(src_tail, dst_tail);
}

/// Thirty-two samples per iteration: `v · 257` is `v` in both bytes.
#[target_feature(enable = "avx2")]
pub(super) unsafe fn convert_u8_to_u16_row_avx2(src: &[u8], dst: &mut [u8], _width: usize) {
    let ElementRow { src, dst } = ElementRow::<u8, u16>::new(src, dst);
    let (groups, src_tail) = src.as_chunks::<32>();
    let (out_groups, dst_tail) = dst.as_chunks_mut::<32>();

    for (group, out) in groups.iter().zip(out_groups.iter_mut()) {
        let bytes = load(group);
        let words = [
            _mm256_cvtepu8_epi16(_mm256_castsi256_si128(bytes)),
            _mm256_cvtepu8_epi16(_mm256_extracti128_si256::<1>(bytes)),
        ];
        let out: &mut [u8; 64] = bytemuck::cast_mut(out);
        let (out, _) = out.as_chunks_mut::<32>();
        for (out, words) in out.iter_mut().zip(words) {
            store(out, _mm256_or_si256(words, _mm256_slli_epi16::<8>(words)));
        }
    }

    scalar::convert_elements(src_tail, dst_tail);
}

/// `round(v / 257)` for sixteen `u16` lanes — the SSE2 `divide_by_257`, whose doc gives the
/// derivation.
#[inline]
#[target_feature(enable = "avx2")]
fn divide_by_257(words: __m256i) -> __m256i {
    #[expect(
        clippy::cast_possible_wrap,
        reason = "the bit pattern of 65281 as a u16 lane"
    )]
    let multiplier = _mm256_set1_epi16(65281u16 as i16);
    let quotient = _mm256_mulhi_epu16(words, multiplier);
    _mm256_srli_epi16::<8>(_mm256_add_epi16(quotient, _mm256_set1_epi16(128)))
}

/// Thirty-two samples per iteration.
#[target_feature(enable = "avx2")]
pub(super) unsafe fn convert_u16_to_u8_row_avx2(src: &[u8], dst: &mut [u8], _width: usize) {
    let ElementRow { src, dst } = ElementRow::<u16, u8>::new(src, dst);
    let (groups, src_tail) = src.as_chunks::<32>();
    let (out_groups, dst_tail) = dst.as_chunks_mut::<32>();

    for (group, out) in groups.iter().zip(out_groups.iter_mut()) {
        let group: &[u8; 64] = bytemuck::cast_ref(group);
        let (halves, _) = group.as_chunks::<32>();
        let lo = divide_by_257(load(&halves[0]));
        let hi = divide_by_257(load(&halves[1]));
        // `packus` interleaves the two inputs per 128-bit lane; the permute restores order.
        let bytes = _mm256_permute4x64_epi64::<0b11_01_10_00>(_mm256_packus_epi16(lo, hi));
        store(out, bytes);
    }

    scalar::convert_elements(src_tail, dst_tail);
}

/// Sixteen samples per iteration.
#[target_feature(enable = "avx2")]
pub(super) unsafe fn convert_u16_to_f32_row_avx2(src: &[u8], dst: &mut [u8], _width: usize) {
    let ElementRow { src, dst } = ElementRow::<u16, f32>::new(src, dst);
    // Divide, never a reciprocal multiply — see the module doc on precision.
    let divisor = _mm256_set1_ps(65535.0);
    let (groups, src_tail) = src.as_chunks::<16>();
    let (out_groups, dst_tail) = dst.as_chunks_mut::<16>();

    for (group, out) in groups.iter().zip(out_groups.iter_mut()) {
        let words = load(bytemuck::cast_ref(group));
        let dwords = [
            _mm256_cvtepu16_epi32(_mm256_castsi256_si128(words)),
            _mm256_cvtepu16_epi32(_mm256_extracti128_si256::<1>(words)),
        ];
        let (out, _) = out.as_chunks_mut::<8>();
        for (out, dwords) in out.iter_mut().zip(dwords) {
            store_ps(out, _mm256_div_ps(_mm256_cvtepi32_ps(dwords), divisor));
        }
    }

    scalar::convert_elements(src_tail, dst_tail);
}

/// Sixteen samples per iteration.
#[target_feature(enable = "avx2")]
pub(super) unsafe fn convert_f32_to_u16_row_avx2(src: &[u8], dst: &mut [u8], _width: usize) {
    let ElementRow { src, dst } = ElementRow::<f32, u16>::new(src, dst);
    let full_scale = _mm256_set1_pd(65535.0);
    let (groups, src_tail) = src.as_chunks::<16>();
    let (out_groups, dst_tail) = dst.as_chunks_mut::<16>();

    for (group, out) in groups.iter().zip(out_groups.iter_mut()) {
        let (octets, _) = group.as_chunks::<8>();
        let out: &mut [u8; 32] = bytemuck::cast_mut(out);
        let (out, _) = out.as_chunks_mut::<16>();
        for (octet, out) in octets.iter().zip(out) {
            let [a, b] = halves(load_ps(octet));
            store128(
                out,
                _mm_packus_epi32(narrow4(a, full_scale), narrow4(b, full_scale)),
            );
        }
    }

    scalar::convert_elements(src_tail, dst_tail);
}
