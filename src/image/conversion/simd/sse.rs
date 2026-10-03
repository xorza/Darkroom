//! SSE2, SSSE3 and SSE4.1 row conversion kernels for `x86_64`.

use std::arch::x86_64::{
    __m128, __m128d, __m128i, _mm_add_epi16, _mm_add_epi32, _mm_alignr_epi8, _mm_and_si128,
    _mm_cvtepi32_ps, _mm_cvtpd_epi32, _mm_cvtps_pd, _mm_div_ps, _mm_loadu_ps, _mm_loadu_si128,
    _mm_madd_epi16, _mm_max_pd, _mm_min_pd, _mm_movehl_ps, _mm_mul_pd, _mm_mulhi_epu16,
    _mm_or_si128, _mm_packs_epi32, _mm_packus_epi16, _mm_packus_epi32, _mm_set1_epi16,
    _mm_set1_epi32, _mm_set1_pd, _mm_set1_ps, _mm_setr_epi8, _mm_setr_epi16, _mm_setzero_pd,
    _mm_setzero_si128, _mm_shuffle_epi8, _mm_slli_epi16, _mm_slli_si128, _mm_srli_epi16,
    _mm_srli_epi32, _mm_srli_si128, _mm_storeu_ps, _mm_storeu_si128, _mm_unpackhi_epi8,
    _mm_unpackhi_epi16, _mm_unpacklo_epi8, _mm_unpacklo_epi16, _mm_unpacklo_epi64,
};

use crate::common::luma;
use crate::image::conversion::scalar;
use crate::image::conversion::simd::ElementRow;

/// One 16-byte load.
#[inline]
#[target_feature(enable = "sse2")]
fn load(bytes: &[u8; 16]) -> __m128i {
    // SAFETY: the argument is exactly the 16 bytes the load reads.
    unsafe { _mm_loadu_si128(bytes.as_ptr().cast()) }
}

/// One 16-byte store.
#[inline]
#[target_feature(enable = "sse2")]
fn store(bytes: &mut [u8; 16], value: __m128i) {
    // SAFETY: the argument is exactly the 16 bytes the store writes.
    unsafe { _mm_storeu_si128(bytes.as_mut_ptr().cast(), value) }
}

/// Four `f32` loaded.
#[inline]
#[target_feature(enable = "sse2")]
fn load_ps(values: &[f32; 4]) -> __m128 {
    // SAFETY: the argument is exactly the four floats the load reads.
    unsafe { _mm_loadu_ps(values.as_ptr()) }
}

/// Four `f32` stored.
#[inline]
#[target_feature(enable = "sse2")]
fn store_ps(values: &mut [f32; 4], value: __m128) {
    // SAFETY: the argument is exactly the four floats the store writes.
    unsafe { _mm_storeu_ps(values.as_mut_ptr(), value) }
}

/// Sixteen `RGBA_U8` pixels per iteration: four loads, three stores.
#[target_feature(enable = "ssse3")]
pub(super) unsafe fn convert_rgba_to_rgb_row_ssse3(src: &[u8], dst: &mut [u8], width: usize) {
    let shuffle = _mm_setr_epi8(0, 1, 2, 4, 5, 6, 8, 9, 10, 12, 13, 14, -1, -1, -1, -1);
    let pixels = width - width % 16;
    let (groups, _) = src[..pixels * 4].as_chunks::<64>();
    let (out_groups, _) = dst[..pixels * 3].as_chunks_mut::<48>();

    for (group, out) in groups.iter().zip(out_groups) {
        let (parts, _) = group.as_chunks::<16>();
        let rgb = [0, 1, 2, 3].map(|part| _mm_shuffle_epi8(load(&parts[part]), shuffle));
        let (out, _) = out.as_chunks_mut::<16>();
        store(
            &mut out[0],
            _mm_or_si128(rgb[0], _mm_slli_si128::<12>(rgb[1])),
        );
        store(
            &mut out[1],
            _mm_or_si128(_mm_srli_si128::<4>(rgb[1]), _mm_slli_si128::<8>(rgb[2])),
        );
        store(
            &mut out[2],
            _mm_or_si128(_mm_srli_si128::<8>(rgb[2]), _mm_slli_si128::<4>(rgb[3])),
        );
    }

    scalar::convert_row::<u8, u8, 4, 3>(&src[pixels * 4..], &mut dst[pixels * 3..], width - pixels);
}

/// Sixteen `RGB_U8` pixels per iteration: three loads, four stores.
#[target_feature(enable = "ssse3")]
pub(super) unsafe fn convert_rgb_to_rgba_row_ssse3(src: &[u8], dst: &mut [u8], width: usize) {
    let alpha = _mm_setr_epi8(0, 0, 0, -1, 0, 0, 0, -1, 0, 0, 0, -1, 0, 0, 0, -1);
    let shuffle = _mm_setr_epi8(0, 1, 2, -1, 3, 4, 5, -1, 6, 7, 8, -1, 9, 10, 11, -1);
    let pixels = width - width % 16;
    let (groups, _) = src[..pixels * 3].as_chunks::<48>();
    let (out_groups, _) = dst[..pixels * 4].as_chunks_mut::<64>();

    for (group, out) in groups.iter().zip(out_groups) {
        let (parts, _) = group.as_chunks::<16>();
        let [in0, in1, in2] = [load(&parts[0]), load(&parts[1]), load(&parts[2])];
        let quads = [
            in0,
            _mm_alignr_epi8::<12>(in1, in0),
            _mm_alignr_epi8::<8>(in2, in1),
            _mm_srli_si128::<4>(in2),
        ];
        let (out, _) = out.as_chunks_mut::<16>();
        for (out, quad) in out.iter_mut().zip(quads) {
            store(out, _mm_or_si128(_mm_shuffle_epi8(quad, shuffle), alpha));
        }
    }

    scalar::convert_row::<u8, u8, 3, 4>(&src[pixels * 3..], &mut dst[pixels * 4..], width - pixels);
}

/// The Rec. 709 luma weights arranged for `madd`, together with the shuffle
/// masks that gather one pixel layout's channels into the lanes they expect.
///
/// `madd` multiplies *signed* 16-bit lanes: red and blue ride one lane pair, and
/// green — which overflows `i16` on its own — rides a second pair as two halves
/// that sum back to it. It accumulates into `i32`, so the weighted sum is the
/// scalar reference's exactly, not the 8-bit approximation a 16-bit accumulator
/// would force.
#[derive(Debug, Clone, Copy)]
struct LumaWeights {
    weight_rb: __m128i,
    weight_gg: __m128i,
    gather_rb: __m128i,
    gather_gg: __m128i,
    round: __m128i,
    one: __m128i,
}

const LUMA_R: u32 = luma::WEIGHTS_Q16[0];
const LUMA_G: u32 = luma::WEIGHTS_Q16[1];
const LUMA_B: u32 = luma::WEIGHTS_Q16[2];

// Guards the split above: `madd` would misread a weight that does not fit.
const _: () = assert!(LUMA_R <= i16::MAX as u32);
const _: () = assert!(LUMA_B <= i16::MAX as u32);
const _: () = assert!(LUMA_G - LUMA_G / 2 <= i16::MAX as u32);

impl LumaWeights {
    /// For four `RGBA_U8` pixels held from byte zero.
    #[target_feature(enable = "ssse3")]
    fn rgba() -> Self {
        Self::new(
            _mm_setr_epi8(0, -1, 2, -1, 4, -1, 6, -1, 8, -1, 10, -1, 12, -1, 14, -1),
            _mm_setr_epi8(1, -1, 1, -1, 5, -1, 5, -1, 9, -1, 9, -1, 13, -1, 13, -1),
        )
    }

    /// For four `RGB_U8` pixels held from byte zero.
    #[target_feature(enable = "ssse3")]
    fn rgb() -> Self {
        Self::new(
            _mm_setr_epi8(0, -1, 2, -1, 3, -1, 5, -1, 6, -1, 8, -1, 9, -1, 11, -1),
            _mm_setr_epi8(1, -1, 1, -1, 4, -1, 4, -1, 7, -1, 7, -1, 10, -1, 10, -1),
        )
    }

    #[target_feature(enable = "ssse3")]
    fn new(gather_rb: __m128i, gather_gg: __m128i) -> Self {
        let (r, b) = (LUMA_R as i16, LUMA_B as i16);
        let (g_lo, g_hi) = ((LUMA_G / 2) as i16, (LUMA_G - LUMA_G / 2) as i16);
        Self {
            weight_rb: _mm_setr_epi16(r, b, r, b, r, b, r, b),
            weight_gg: _mm_setr_epi16(g_lo, g_hi, g_lo, g_hi, g_lo, g_hi, g_lo, g_hi),
            gather_rb,
            gather_gg,
            round: _mm_set1_epi32(0x7FFF),
            one: _mm_set1_epi32(1),
        }
    }

    /// The luminance of the four pixels `quad` holds from byte zero, one per
    /// `i32` lane, rounded as [`luma::round_q16`] rounds.
    #[inline]
    #[target_feature(enable = "ssse3")]
    fn apply(self, quad: __m128i) -> __m128i {
        let sum = _mm_add_epi32(
            _mm_madd_epi16(_mm_shuffle_epi8(quad, self.gather_rb), self.weight_rb),
            _mm_madd_epi16(_mm_shuffle_epi8(quad, self.gather_gg), self.weight_gg),
        );
        let odd = _mm_and_si128(_mm_srli_epi32::<16>(sum), self.one);
        _mm_srli_epi32::<16>(_mm_add_epi32(_mm_add_epi32(sum, self.round), odd))
    }
}

/// Sixteen luminance values, four `i32` lanes at a time, packed into 16 bytes.
/// Every value is already in `0..=255`, so neither saturating pack can bite.
#[inline]
#[target_feature(enable = "sse2")]
fn pack_quads(lum: [__m128i; 4]) -> __m128i {
    _mm_packus_epi16(
        _mm_packs_epi32(lum[0], lum[1]),
        _mm_packs_epi32(lum[2], lum[3]),
    )
}

/// Sixteen `RGBA_U8` pixels per iteration: four 16-byte loads, each already a
/// four-pixel quad, and one 16-byte store.
#[target_feature(enable = "ssse3")]
pub(super) unsafe fn convert_rgba_to_l_row_ssse3(src: &[u8], dst: &mut [u8], width: usize) {
    let weights = LumaWeights::rgba();
    let pixels = width - width % 16;
    let (groups, _) = src[..pixels * 4].as_chunks::<64>();
    let (out_groups, _) = dst[..pixels].as_chunks_mut::<16>();

    for (group, out) in groups.iter().zip(out_groups) {
        let (parts, _) = group.as_chunks::<16>();
        store(
            out,
            pack_quads([0, 1, 2, 3].map(|part| weights.apply(load(&parts[part])))),
        );
    }

    scalar::convert_row::<u8, u8, 4, 1>(&src[pixels * 4..], &mut dst[pixels..], width - pixels);
}

/// Sixteen `RGB_U8` pixels per iteration: three 16-byte loads spanning the
/// group's 48 bytes exactly, realigned into four-pixel quads, one 16-byte store.
#[target_feature(enable = "ssse3")]
pub(super) unsafe fn convert_rgb_to_l_row_ssse3(src: &[u8], dst: &mut [u8], width: usize) {
    let weights = LumaWeights::rgb();
    let pixels = width - width % 16;
    let (groups, _) = src[..pixels * 3].as_chunks::<48>();
    let (out_groups, _) = dst[..pixels].as_chunks_mut::<16>();

    for (group, out) in groups.iter().zip(out_groups) {
        let (parts, _) = group.as_chunks::<16>();
        let [in0, in1, in2] = [load(&parts[0]), load(&parts[1]), load(&parts[2])];
        // A quad is 12 bytes, so only the first starts on a load boundary;
        // `alignr` slides the next three down to byte zero.
        let lum = [
            weights.apply(in0),
            weights.apply(_mm_alignr_epi8::<12>(in1, in0)),
            weights.apply(_mm_alignr_epi8::<8>(in2, in1)),
            weights.apply(_mm_srli_si128::<4>(in2)),
        ];
        store(out, pack_quads(lum));
    }

    scalar::convert_row::<u8, u8, 3, 1>(&src[pixels * 3..], &mut dst[pixels..], width - pixels);
}

/// Sixteen `L_U8` pixels per iteration: one load, four stores.
#[target_feature(enable = "ssse3")]
pub(super) unsafe fn convert_l_to_rgba_row_ssse3(src: &[u8], dst: &mut [u8], width: usize) {
    let shuffles = [
        _mm_setr_epi8(0, 0, 0, -1, 1, 1, 1, -1, 2, 2, 2, -1, 3, 3, 3, -1),
        _mm_setr_epi8(4, 4, 4, -1, 5, 5, 5, -1, 6, 6, 6, -1, 7, 7, 7, -1),
        _mm_setr_epi8(8, 8, 8, -1, 9, 9, 9, -1, 10, 10, 10, -1, 11, 11, 11, -1),
        _mm_setr_epi8(
            12, 12, 12, -1, 13, 13, 13, -1, 14, 14, 14, -1, 15, 15, 15, -1,
        ),
    ];
    let alpha = _mm_setr_epi8(0, 0, 0, -1, 0, 0, 0, -1, 0, 0, 0, -1, 0, 0, 0, -1);
    let pixels = width - width % 16;
    let (groups, _) = src[..pixels].as_chunks::<16>();
    let (out_groups, _) = dst[..pixels * 4].as_chunks_mut::<64>();

    for (group, out) in groups.iter().zip(out_groups) {
        let grey = load(group);
        let (out, _) = out.as_chunks_mut::<16>();
        for (out, shuffle) in out.iter_mut().zip(shuffles) {
            store(out, _mm_or_si128(_mm_shuffle_epi8(grey, shuffle), alpha));
        }
    }

    scalar::convert_row::<u8, u8, 1, 4>(&src[pixels..], &mut dst[pixels * 4..], width - pixels);
}

/// Sixteen `L_U8` pixels per iteration: one load, three stores.
#[target_feature(enable = "ssse3")]
pub(super) unsafe fn convert_l_to_rgb_row_ssse3(src: &[u8], dst: &mut [u8], width: usize) {
    let shuffles = [
        _mm_setr_epi8(0, 0, 0, 1, 1, 1, 2, 2, 2, 3, 3, 3, 4, 4, 4, 5),
        _mm_setr_epi8(5, 5, 6, 6, 6, 7, 7, 7, 8, 8, 8, 9, 9, 9, 10, 10),
        _mm_setr_epi8(
            10, 11, 11, 11, 12, 12, 12, 13, 13, 13, 14, 14, 14, 15, 15, 15,
        ),
    ];
    let pixels = width - width % 16;
    let (groups, _) = src[..pixels].as_chunks::<16>();
    let (out_groups, _) = dst[..pixels * 3].as_chunks_mut::<48>();

    for (group, out) in groups.iter().zip(out_groups) {
        let grey = load(group);
        let (out, _) = out.as_chunks_mut::<16>();
        for (out, shuffle) in out.iter_mut().zip(shuffles) {
            store(out, _mm_shuffle_epi8(grey, shuffle));
        }
    }

    scalar::convert_row::<u8, u8, 1, 3>(&src[pixels..], &mut dst[pixels * 3..], width - pixels);
}

/// Four `f32` on `[0, 1]` scaled by `full_scale` and rounded as [`Sample::from_unit`] does:
/// the product in `f64`, clamped to `[0, full_scale]` — `maxpd` returns its second operand for
/// a NaN, so NaN goes to zero — and converted under the default round-to-nearest-even mode.
///
/// [`Sample::from_unit`]: crate::common::sample::Sample::from_unit
#[inline]
#[target_feature(enable = "sse2")]
fn narrow4(values: __m128, full_scale: __m128d) -> __m128i {
    let scale = |half: __m128d| {
        _mm_cvtpd_epi32(_mm_min_pd(
            _mm_max_pd(_mm_mul_pd(half, full_scale), _mm_setzero_pd()),
            full_scale,
        ))
    };
    _mm_unpacklo_epi64(
        scale(_mm_cvtps_pd(values)),
        scale(_mm_cvtps_pd(_mm_movehl_ps(values, values))),
    )
}

/// Sixteen samples per iteration.
#[target_feature(enable = "sse2")]
pub(super) unsafe fn convert_f32_to_u8_row_sse2(src: &[u8], dst: &mut [u8], _width: usize) {
    let ElementRow { src, dst } = ElementRow::<f32, u8>::new(src, dst);
    let full_scale = _mm_set1_pd(255.0);
    let (groups, src_tail) = src.as_chunks::<16>();
    let (out_groups, dst_tail) = dst.as_chunks_mut::<16>();

    for (group, out) in groups.iter().zip(out_groups.iter_mut()) {
        let (quads, _) = group.as_chunks::<4>();
        let ints = [0, 1, 2, 3].map(|quad| narrow4(load_ps(&quads[quad]), full_scale));
        store(out, pack_quads(ints));
    }

    scalar::convert_elements(src_tail, dst_tail);
}

/// Sixteen samples per iteration.
#[target_feature(enable = "sse2")]
pub(super) unsafe fn convert_u8_to_f32_row_sse2(src: &[u8], dst: &mut [u8], _width: usize) {
    let ElementRow { src, dst } = ElementRow::<u8, f32>::new(src, dst);
    // Divide, never a reciprocal multiply — see the module doc on precision.
    let divisor = _mm_set1_ps(255.0);
    let zero = _mm_setzero_si128();
    let (groups, src_tail) = src.as_chunks::<16>();
    let (out_groups, dst_tail) = dst.as_chunks_mut::<16>();

    for (group, out) in groups.iter().zip(out_groups.iter_mut()) {
        let bytes = load(group);
        let words = [
            _mm_unpacklo_epi8(bytes, zero),
            _mm_unpackhi_epi8(bytes, zero),
        ];
        let dwords = [
            _mm_unpacklo_epi16(words[0], zero),
            _mm_unpackhi_epi16(words[0], zero),
            _mm_unpacklo_epi16(words[1], zero),
            _mm_unpackhi_epi16(words[1], zero),
        ];
        let (out, _) = out.as_chunks_mut::<4>();
        for (out, dwords) in out.iter_mut().zip(dwords) {
            store_ps(out, _mm_div_ps(_mm_cvtepi32_ps(dwords), divisor));
        }
    }

    scalar::convert_elements(src_tail, dst_tail);
}

/// Sixteen samples per iteration: `v · 257` is `v` in both bytes.
#[target_feature(enable = "sse2")]
pub(super) unsafe fn convert_u8_to_u16_row_sse2(src: &[u8], dst: &mut [u8], _width: usize) {
    let ElementRow { src, dst } = ElementRow::<u8, u16>::new(src, dst);
    let zero = _mm_setzero_si128();
    let (groups, src_tail) = src.as_chunks::<16>();
    let (out_groups, dst_tail) = dst.as_chunks_mut::<16>();

    for (group, out) in groups.iter().zip(out_groups.iter_mut()) {
        let bytes = load(group);
        let words = [
            _mm_unpacklo_epi8(bytes, zero),
            _mm_unpackhi_epi8(bytes, zero),
        ];
        let out: &mut [u8; 32] = bytemuck::cast_mut(out);
        let (out, _) = out.as_chunks_mut::<16>();
        for (out, words) in out.iter_mut().zip(words) {
            store(out, _mm_or_si128(words, _mm_slli_epi16::<8>(words)));
        }
    }

    scalar::convert_elements(src_tail, dst_tail);
}

/// `round(v / 257)` for eight `u16` lanes, exactly: `⌊v · 65281 / 2¹⁶⌋` stays within 65280, so
/// adding 128 cannot overflow, and the shift by 8 then gives the nearest quotient for every `v`
/// — `v / 257` is never a tie, 257 being odd. The sample tests check all 65 536 inputs.
#[inline]
#[target_feature(enable = "sse2")]
fn divide_by_257(words: __m128i) -> __m128i {
    #[expect(
        clippy::cast_possible_wrap,
        reason = "the bit pattern of 65281 as a u16 lane"
    )]
    let multiplier = _mm_set1_epi16(65281u16 as i16);
    let quotient = _mm_mulhi_epu16(words, multiplier);
    _mm_srli_epi16::<8>(_mm_add_epi16(quotient, _mm_set1_epi16(128)))
}

/// Sixteen samples per iteration.
#[target_feature(enable = "sse2")]
pub(super) unsafe fn convert_u16_to_u8_row_sse2(src: &[u8], dst: &mut [u8], _width: usize) {
    let ElementRow { src, dst } = ElementRow::<u16, u8>::new(src, dst);
    let (groups, src_tail) = src.as_chunks::<16>();
    let (out_groups, dst_tail) = dst.as_chunks_mut::<16>();

    for (group, out) in groups.iter().zip(out_groups.iter_mut()) {
        let group: &[u8; 32] = bytemuck::cast_ref(group);
        let (halves, _) = group.as_chunks::<16>();
        let [lo, hi] = [
            divide_by_257(load(&halves[0])),
            divide_by_257(load(&halves[1])),
        ];
        store(out, _mm_packus_epi16(lo, hi));
    }

    scalar::convert_elements(src_tail, dst_tail);
}

/// Eight samples per iteration.
#[target_feature(enable = "sse2")]
pub(super) unsafe fn convert_u16_to_f32_row_sse2(src: &[u8], dst: &mut [u8], _width: usize) {
    let ElementRow { src, dst } = ElementRow::<u16, f32>::new(src, dst);
    // Divide, never a reciprocal multiply — see the module doc on precision.
    let divisor = _mm_set1_ps(65535.0);
    let zero = _mm_setzero_si128();
    let (groups, src_tail) = src.as_chunks::<8>();
    let (out_groups, dst_tail) = dst.as_chunks_mut::<8>();

    for (group, out) in groups.iter().zip(out_groups.iter_mut()) {
        let words = load(bytemuck::cast_ref(group));
        let dwords = [
            _mm_unpacklo_epi16(words, zero),
            _mm_unpackhi_epi16(words, zero),
        ];
        let (out, _) = out.as_chunks_mut::<4>();
        for (out, dwords) in out.iter_mut().zip(dwords) {
            store_ps(out, _mm_div_ps(_mm_cvtepi32_ps(dwords), divisor));
        }
    }

    scalar::convert_elements(src_tail, dst_tail);
}

/// Eight samples per iteration. SSE4.1 for `packus_epi32`, the unsigned pack into `u16`.
#[target_feature(enable = "sse4.1")]
pub(super) unsafe fn convert_f32_to_u16_row_sse41(src: &[u8], dst: &mut [u8], _width: usize) {
    let ElementRow { src, dst } = ElementRow::<f32, u16>::new(src, dst);
    let full_scale = _mm_set1_pd(65535.0);
    let (groups, src_tail) = src.as_chunks::<8>();
    let (out_groups, dst_tail) = dst.as_chunks_mut::<8>();

    for (group, out) in groups.iter().zip(out_groups.iter_mut()) {
        let (quads, _) = group.as_chunks::<4>();
        let words = _mm_packus_epi32(
            narrow4(load_ps(&quads[0]), full_scale),
            narrow4(load_ps(&quads[1]), full_scale),
        );
        store(bytemuck::cast_mut(out), words);
    }

    scalar::convert_elements(src_tail, dst_tail);
}
