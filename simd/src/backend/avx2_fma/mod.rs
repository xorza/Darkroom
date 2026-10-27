//! [`Avx2Fma`]: the `x86_64` [`Isa`], one 256-bit register per vector.
//!
//! AVX2 and FMA are the x86 baseline, x86-64-v3: the crate does not build for x86 without them
//! (see `backend`), so a token needs no detection and [`IsaTier::run`](crate::IsaTier::run) no
//! `#[target_feature]` entry,
//! and every intrinsic here is sound to call on any operands. Loads and stores take arrays of
//! exactly one vector's width.

pub(crate) mod f32x8;
pub(crate) mod f64x4;
pub(crate) mod i32x8;
pub(crate) mod mask8;
pub(crate) mod u16x16;
pub(crate) mod u8x32;

use std::arch::x86_64::*;

use crate::backend::avx2_fma::f32x8::Avx2F32;
use crate::backend::avx2_fma::f64x4::Avx2F64;
use crate::backend::avx2_fma::i32x8::Avx2I32;
use crate::backend::avx2_fma::mask8::Avx2Mask;
use crate::backend::avx2_fma::u8x32::Avx2U8;
use crate::backend::avx2_fma::u16x16::Avx2U16;
use crate::f32x8::{F32_LANES, F32x8};
use crate::f64x4::F64_LANES;
use crate::full_scale::FullScale;
use crate::halves::Halves;
use crate::i32x8::{I32_LANES, I32x8};
use crate::isa::{Isa, interleave_lanes, lookup_last, pair_lanes, u32_le_lanes};
use crate::mask8::Mask8;
use crate::u8x32::U8_LANES;
use crate::u16x16::U16_LANES;

/// AVX2 with FMA: every `x86_64` CPU since Haswell (2013) and Excavator (2015), and the
/// workspace's x86 baseline.
#[derive(Debug, Default, Clone, Copy)]
pub struct Avx2Fma {
    _private: (),
}

impl Avx2Fma {
    pub const fn new() -> Self {
        Self { _private: () }
    }
}

impl Isa for Avx2Fma {
    type F32 = Avx2F32;
    type F64 = Avx2F64;
    type U8 = Avx2U8;
    type U16 = Avx2U16;
    type I32 = Avx2I32;

    /// Measured 0.64–0.70 of `vdivps` on the conversions to `f32` on Zen 3, where the divide is
    /// the bottleneck; on NEON the divide is the faster.
    #[inline(always)]
    fn div_full_scale(self, values: Avx2F32, scale: FullScale) -> Avx2F32 {
        unsafe {
            let divisor = _mm256_set1_ps(scale.divisor());
            let reciprocal = _mm256_set1_ps(scale.reciprocal());
            let quotient = _mm256_mul_ps(values.0, reciprocal);
            let residual = _mm256_fnmadd_ps(quotient, divisor, values.0);
            Avx2F32(_mm256_fmadd_ps(residual, reciprocal, quotient))
        }
    }

    #[inline(always)]
    fn splat_f32(self, value: f32) -> Avx2F32 {
        Avx2F32(unsafe { _mm256_set1_ps(value) })
    }

    #[inline(always)]
    fn splat_f64(self, value: f64) -> Avx2F64 {
        Avx2F64(unsafe { _mm256_set1_pd(value) })
    }

    #[inline(always)]
    fn load_f32(self, lanes: &[f32; F32_LANES]) -> Avx2F32 {
        Avx2F32(unsafe { _mm256_loadu_ps(lanes.as_ptr()) })
    }

    #[inline(always)]
    fn load_f64(self, lanes: &[f64; F64_LANES]) -> Avx2F64 {
        Avx2F64(unsafe { _mm256_loadu_pd(lanes.as_ptr()) })
    }

    #[inline(always)]
    fn load_u8(self, lanes: &[u8; U8_LANES]) -> Avx2U8 {
        Avx2U8(unsafe { _mm256_loadu_si256(lanes.as_ptr().cast()) })
    }

    #[inline(always)]
    fn load_u16(self, lanes: &[u16; U16_LANES]) -> Avx2U16 {
        Avx2U16(unsafe { _mm256_loadu_si256(lanes.as_ptr().cast()) })
    }

    #[inline(always)]
    fn splat_u8(self, value: u8) -> Avx2U8 {
        Avx2U8(unsafe { _mm256_set1_epi8(value.cast_signed()) })
    }

    /// One shuffle a register drops each pixel's fourth byte and packs the twelve left at its
    /// bottom; byte shifts then join the twelve-byte runs into whole 16-byte stores.
    #[inline(always)]
    fn drop_fourth_u8(self, pixels: &[u8; 4 * U8_LANES], out: &mut [u8; 3 * U8_LANES]) {
        let (loads, []) = pixels.as_chunks::<32>() else {
            unreachable!("a hundred and twenty-eight bytes are four loads of thirty-two")
        };
        let (stores, []) = out.as_chunks_mut::<16>() else {
            unreachable!("ninety-six bytes are six stores of sixteen")
        };
        unsafe {
            let shuffle = mask(&DROP_FOURTH);
            let r0 = _mm256_shuffle_epi8(_mm256_loadu_si256(loads[0].as_ptr().cast()), shuffle);
            let r1 = _mm256_shuffle_epi8(_mm256_loadu_si256(loads[1].as_ptr().cast()), shuffle);
            let r2 = _mm256_shuffle_epi8(_mm256_loadu_si256(loads[2].as_ptr().cast()), shuffle);
            let r3 = _mm256_shuffle_epi8(_mm256_loadu_si256(loads[3].as_ptr().cast()), shuffle);
            // Eight twelve-byte runs, in pixel order, each at the bottom of its register.
            let runs = [
                _mm256_castsi256_si128(r0),
                _mm256_extracti128_si256::<1>(r0),
                _mm256_castsi256_si128(r1),
                _mm256_extracti128_si256::<1>(r1),
                _mm256_castsi256_si128(r2),
                _mm256_extracti128_si256::<1>(r2),
                _mm256_castsi256_si128(r3),
                _mm256_extracti128_si256::<1>(r3),
            ];
            for half in 0..2 {
                let [a, b, c, d] = [
                    runs[4 * half],
                    runs[4 * half + 1],
                    runs[4 * half + 2],
                    runs[4 * half + 3],
                ];
                let base = 3 * half;
                _mm_storeu_si128(
                    stores[base].as_mut_ptr().cast(),
                    _mm_or_si128(a, _mm_slli_si128::<12>(b)),
                );
                _mm_storeu_si128(
                    stores[base + 1].as_mut_ptr().cast(),
                    _mm_or_si128(_mm_srli_si128::<4>(b), _mm_slli_si128::<8>(c)),
                );
                _mm_storeu_si128(
                    stores[base + 2].as_mut_ptr().cast(),
                    _mm_or_si128(_mm_srli_si128::<8>(c), _mm_slli_si128::<4>(d)),
                );
            }
        }
    }

    /// Byte alignments bring each four-pixel quad to the bottom of a register, and one shuffle
    /// spreads it to four bytes a pixel, the fourth or-ed in.
    #[inline(always)]
    fn append_fourth_u8(
        self,
        pixels: &[u8; 3 * U8_LANES],
        fourth: u8,
        out: &mut [u8; 4 * U8_LANES],
    ) {
        let (loads, []) = pixels.as_chunks::<16>() else {
            unreachable!("ninety-six bytes are six loads of sixteen")
        };
        let (stores, []) = out.as_chunks_mut::<16>() else {
            unreachable!("a hundred and twenty-eight bytes are eight stores of sixteen")
        };
        unsafe {
            let spread = _mm_loadu_si128(SPREAD_THREE.as_ptr().cast());
            let fill = _mm_and_si128(
                _mm_set1_epi8(fourth.cast_signed()),
                _mm_loadu_si128(FOURTH_BYTES.as_ptr().cast()),
            );
            for half in 0..2 {
                let in0 = _mm_loadu_si128(loads[3 * half].as_ptr().cast());
                let in1 = _mm_loadu_si128(loads[3 * half + 1].as_ptr().cast());
                let in2 = _mm_loadu_si128(loads[3 * half + 2].as_ptr().cast());
                let quads = [
                    in0,
                    _mm_alignr_epi8::<12>(in1, in0),
                    _mm_alignr_epi8::<8>(in2, in1),
                    _mm_srli_si128::<4>(in2),
                ];
                for (quad, store) in quads.into_iter().zip(&mut stores[4 * half..4 * half + 4]) {
                    _mm_storeu_si128(
                        store.as_mut_ptr().cast(),
                        _mm_or_si128(_mm_shuffle_epi8(quad, spread), fill),
                    );
                }
            }
        }
    }

    /// Byte alignments bring each four-pixel quad to the bottom of a register; see
    /// `weighted_quad`.
    #[inline(always)]
    fn weighted_sum3_u8(self, pixels: &[u8; 3 * U8_LANES], weights: [u16; 3]) -> Avx2U8 {
        let (loads, []) = pixels.as_chunks::<16>() else {
            unreachable!("ninety-six bytes are six loads of sixteen")
        };
        unsafe {
            let weights = PairWeights::new(weights, &GATHER_PAIRS3);
            let mut halves = [_mm_setzero_si128(); 2];
            for (half, out) in halves.iter_mut().enumerate() {
                let in0 = _mm_loadu_si128(loads[3 * half].as_ptr().cast());
                let in1 = _mm_loadu_si128(loads[3 * half + 1].as_ptr().cast());
                let in2 = _mm_loadu_si128(loads[3 * half + 2].as_ptr().cast());
                *out = pack_quads([
                    weighted_quad(in0, weights),
                    weighted_quad(_mm_alignr_epi8::<12>(in1, in0), weights),
                    weighted_quad(_mm_alignr_epi8::<8>(in2, in1), weights),
                    weighted_quad(_mm_srli_si128::<4>(in2), weights),
                ]);
            }
            Avx2U8(_mm256_set_m128i(halves[1], halves[0]))
        }
    }

    /// Each sixteen-byte load is a four-pixel quad already; see `weighted_quad`.
    #[inline(always)]
    fn weighted_sum3_of4_u8(self, pixels: &[u8; 4 * U8_LANES], weights: [u16; 3]) -> Avx2U8 {
        let (loads, []) = pixels.as_chunks::<16>() else {
            unreachable!("a hundred and twenty-eight bytes are eight loads of sixteen")
        };
        unsafe {
            let weights = PairWeights::new(weights, &GATHER_PAIRS4);
            let mut halves = [_mm_setzero_si128(); 2];
            for (half, out) in halves.iter_mut().enumerate() {
                let quads = &loads[4 * half..4 * half + 4];
                *out = pack_quads([
                    weighted_quad(_mm_loadu_si128(quads[0].as_ptr().cast()), weights),
                    weighted_quad(_mm_loadu_si128(quads[1].as_ptr().cast()), weights),
                    weighted_quad(_mm_loadu_si128(quads[2].as_ptr().cast()), weights),
                    weighted_quad(_mm_loadu_si128(quads[3].as_ptr().cast()), weights),
                ]);
            }
            Avx2U8(_mm256_set_m128i(halves[1], halves[0]))
        }
    }

    /// Three channels through `store_rgb_u8`, four through `store_rgba_u8`.
    #[inline(always)]
    fn store_interleaved_u8<const C: usize, const G: usize>(
        self,
        channels: [Avx2U8; C],
        bytes: &mut [u8; G],
    ) {
        const { assert!((C == 3 || C == 4) && G == C * U8_LANES) };
        let c = channels.as_slice();
        let bytes = bytes.as_mut_slice();
        if C == 3 {
            store_rgb_u8(
                [c[0].0, c[1].0, c[2].0],
                bytes
                    .try_into()
                    .expect("three channels fill ninety-six bytes"),
            );
        } else {
            store_rgba_u8(
                [c[0].0, c[1].0, c[2].0, c[3].0],
                bytes
                    .try_into()
                    .expect("four channels fill a hundred and twenty-eight bytes"),
            );
        }
    }

    /// `maxps` returns its second operand where either is NaN, so a NaN goes to 0; `cvtps2dq`
    /// rounds under the default round-to-nearest-even mode.
    #[inline(always)]
    fn round_clamped_f32_to_i32(self, values: Avx2F32, high: Avx2F32) -> Avx2I32 {
        unsafe {
            let clamped = _mm256_min_ps(_mm256_max_ps(values.0, _mm256_setzero_ps()), high.0);
            Avx2I32(_mm256_cvtps_epi32(clamped))
        }
    }

    #[inline(always)]
    fn load_i32(self, lanes: &[i32; I32_LANES]) -> Avx2I32 {
        Avx2I32(unsafe { _mm256_loadu_si256(lanes.as_ptr().cast()) })
    }

    #[inline(always)]
    fn splat_u16(self, value: u16) -> Avx2U16 {
        Avx2U16(unsafe { _mm256_set1_epi16(value.cast_signed()) })
    }

    #[inline(always)]
    fn splat_i32(self, value: i32) -> Avx2I32 {
        Avx2I32(unsafe { _mm256_set1_epi32(value) })
    }

    /// A masked gather reads nothing in a lane its mask clears, so it reads the lanes whose four
    /// bytes lie in `bytes`. A lane whose four bytes run past the end, rare, sends the vector lane
    /// by lane.
    #[inline(always)]
    fn gather_u32_le(self, bytes: &[u8], offset: Avx2I32, valid: Avx2Mask) -> Avx2I32 {
        let reads = Reads::new(bytes.len(), 4, offset, valid);
        if reads.partial {
            return self.load_i32(
                &u32_le_lanes(bytes, offset.to_array(), valid.to_bitmask()).map(u32::cast_signed),
            );
        }
        Avx2I32(unsafe {
            _mm256_mask_i32gather_epi32::<1>(
                _mm256_setzero_si256(),
                bytes.as_ptr().cast(),
                offset.0,
                reads.whole,
            )
        })
    }

    /// Eight 256-bit loads, a lane's eight samples each from its start or, in a lane not read,
    /// from the table's first, then two in-lane 4 × 4 transposes of their halves.
    #[inline(always)]
    fn gather_pair_f32<const SPLIT: usize>(
        self,
        table: &[f32],
        start: Avx2I32,
        first: Avx2Mask,
        second: Avx2Mask,
    ) -> [Avx2F32; 8] {
        let reads = Reads::new(table.len(), 8, start, first.or(second));
        if reads.partial {
            let bits = [first.to_bitmask(), second.to_bitmask()];
            return pair_lanes::<f32, SPLIT>(table, start.to_array(), bits)
                .map(|lanes| self.load_f32(&lanes));
        }
        let starts = reads.starts(start);
        let at = table.as_ptr();
        let lanes = unsafe {
            [
                _mm256_loadu_ps(at.add(starts[0])),
                _mm256_loadu_ps(at.add(starts[1])),
                _mm256_loadu_ps(at.add(starts[2])),
                _mm256_loadu_ps(at.add(starts[3])),
                _mm256_loadu_ps(at.add(starts[4])),
                _mm256_loadu_ps(at.add(starts[5])),
                _mm256_loadu_ps(at.add(starts[6])),
                _mm256_loadu_ps(at.add(starts[7])),
            ]
        };
        pair_transposed::<SPLIT>(lanes, reads.whole, first, second).map(Avx2F32)
    }

    /// [`Self::gather_pair_f32`]'s loads and transposes, each lane's eight words widened first.
    #[inline(always)]
    fn gather_pair_u16<const SPLIT: usize>(
        self,
        table: &[u16],
        start: Avx2I32,
        first: Avx2Mask,
        second: Avx2Mask,
    ) -> [Avx2I32; 8] {
        let reads = Reads::new(table.len(), 8, start, first.or(second));
        if reads.partial {
            let bits = [first.to_bitmask(), second.to_bitmask()];
            return pair_lanes::<u16, SPLIT>(table, start.to_array(), bits)
                .map(|lanes| self.load_i32(&lanes.map(i32::from)));
        }
        let starts = reads.starts(start);
        let at = table.as_ptr();
        let lanes = [
            widened_at(at, starts[0]),
            widened_at(at, starts[1]),
            widened_at(at, starts[2]),
            widened_at(at, starts[3]),
            widened_at(at, starts[4]),
            widened_at(at, starts[5]),
            widened_at(at, starts[6]),
            widened_at(at, starts[7]),
        ];
        pair_transposed::<SPLIT>(lanes, reads.whole, first, second)
            .map(|samples| Avx2I32(unsafe { _mm256_castps_si256(samples) }))
    }

    /// Four channels transpose back into pixels by `transpose4` and leave as four 256-bit
    /// stores; three take the lane loop.
    #[inline(always)]
    fn store_interleaved_f32<const C: usize, const G: usize>(
        self,
        channels: [Avx2F32; C],
        samples: &mut [f32; G],
    ) {
        const { assert!((C == 3 || C == 4) && G == C * F32_LANES) };
        if C == 3 {
            interleave_lanes(&channels.map(F32x8::to_array), samples);
            return;
        }
        let c = channels.as_slice();
        let rows = transpose4([c[0].0, c[1].0, c[2].0, c[3].0]);
        let at = samples.as_mut_ptr();
        unsafe {
            _mm256_storeu_ps(at, _mm256_permute2f128_ps::<0x20>(rows[0], rows[1]));
            _mm256_storeu_ps(at.add(8), _mm256_permute2f128_ps::<0x20>(rows[2], rows[3]));
            _mm256_storeu_ps(at.add(16), _mm256_permute2f128_ps::<0x31>(rows[0], rows[1]));
            _mm256_storeu_ps(at.add(24), _mm256_permute2f128_ps::<0x31>(rows[2], rows[3]));
        }
    }

    #[inline(always)]
    fn load_u8_as_i32(self, bytes: &[u8; I32_LANES]) -> Avx2I32 {
        Avx2I32(unsafe { _mm256_cvtepu8_epi32(_mm_loadl_epi64(bytes.as_ptr().cast())) })
    }

    /// `maxpd` returns its second operand where either is NaN, so a NaN goes to 0; `cvtpd2dq`
    /// rounds under the default round-to-nearest-even mode.
    #[inline(always)]
    fn round_clamped_f64_to_i32(self, f64s: Halves<Avx2F64>, high: Avx2F64) -> Avx2I32 {
        let [low, upper] = [f64s.low, f64s.high].map(|f64s| round4(f64s, high));
        Avx2I32(unsafe { _mm256_set_m128i(upper, low) })
    }

    /// Each quad rounded into a 128-bit half and the halves packed there: one lane crossing for the
    /// sixteen words, where packing the 256-bit [`Self::round_clamped_f64_to_i32`]s takes three.
    #[inline(always)]
    fn round_clamped_f64_to_u16(self, f64s: [Halves<Avx2F64>; 2], high: Avx2F64) -> Avx2U16 {
        let [low, upper] = f64s.map(|f64s| unsafe {
            _mm_packus_epi32(round4(f64s.low, high), round4(f64s.high, high))
        });
        Avx2U16(unsafe { _mm256_set_m128i(upper, low) })
    }

    /// As [`Self::round_clamped_f64_to_u16`], two packs deeper: one lane crossing for the
    /// thirty-two bytes, where packing the 256-bit [`Self::round_clamped_f64_to_i32`]s takes five.
    #[inline(always)]
    fn round_clamped_f64_to_u8(self, f64s: [Halves<Avx2F64>; 4], high: Avx2F64) -> Avx2U8 {
        let words = f64s.map(|f64s| unsafe {
            _mm_packs_epi32(round4(f64s.low, high), round4(f64s.high, high))
        });
        unsafe {
            Avx2U8(_mm256_set_m128i(
                _mm_packus_epi16(words[2], words[3]),
                _mm_packus_epi16(words[0], words[1]),
            ))
        }
    }

    /// The packs interleave their inputs per 128-bit half, which one permute undoes.
    #[inline(always)]
    fn narrow_i32_to_u8(self, ints: [Avx2I32; 4]) -> Avx2U8 {
        unsafe {
            let low = _mm256_packs_epi32(ints[0].0, ints[1].0);
            let high = _mm256_packs_epi32(ints[2].0, ints[3].0);
            let order = _mm256_setr_epi32(0, 4, 1, 5, 2, 6, 3, 7);
            Avx2U8(_mm256_permutevar8x32_epi32(
                _mm256_packus_epi16(low, high),
                order,
            ))
        }
    }

    #[inline(always)]
    fn lookup_f32(self, table: &[f32], idx: Avx2F32) -> Avx2F32 {
        let clamped = idx
            .min(self.splat_f32(lookup_last(table)))
            .max(self.splat_f32(0.0));
        Avx2F32(unsafe { _mm256_i32gather_ps::<4>(table.as_ptr(), _mm256_cvttps_epi32(clamped.0)) })
    }
}

/// The lanes a gather of `width` elements a lane from a table of `len` reads in one instruction
/// each: `whole`, those `valid` sets whose elements all lie in the table; `partial`, whether a
/// lane `valid` sets has an element outside it, or the table is shorter than one lane's read,
/// which sends the gather lane by lane. Only an image's first and last pixels run so.
#[derive(Debug, Clone, Copy)]
struct Reads {
    whole: __m256i,
    partial: bool,
}

impl Reads {
    #[inline(always)]
    fn new(len: usize, width: usize, start: Avx2I32, valid: Avx2Mask) -> Self {
        let whole = in_range(len.saturating_sub(width - 1), start, valid);
        let outside = unsafe { _mm256_andnot_si256(whole, _mm256_castps_si256(valid.0)) };
        Self {
            whole,
            partial: len < width || unsafe { _mm256_testz_si256(outside, outside) } == 0,
        }
    }

    /// Each lane's start where it reads whole, and the table's first element elsewhere: a
    /// start whose `width` elements lie in the table, as [`Self::new`] found them.
    #[inline(always)]
    fn starts(self, start: Avx2I32) -> [usize; I32_LANES] {
        let starts = Avx2I32(unsafe { _mm256_and_si256(start.0, self.whole) }).to_array();
        let mut lanes = [0; I32_LANES];
        for (lane, start) in lanes.iter_mut().zip(starts) {
            *lane = start.cast_unsigned() as usize;
        }
        lanes
    }
}

/// The lanes `valid` sets whose index is in `[0, end)`. An `end` past `i32::MAX` holds every
/// non-negative index.
#[inline(always)]
fn in_range(end: usize, idx: Avx2I32, valid: Avx2Mask) -> __m256i {
    let end = i32::try_from(end).unwrap_or(i32::MAX);
    unsafe {
        let below_end = _mm256_cmpgt_epi32(_mm256_set1_epi32(end), idx.0);
        let negative = _mm256_cmpgt_epi32(_mm256_setzero_si256(), idx.0);
        _mm256_and_si256(
            _mm256_castps_si256(valid.0),
            _mm256_andnot_si256(negative, below_end),
        )
    }
}
/// The eight words of `table` from `start`, widened: a function, not a closure, so the load stays
/// in line however many lanes call it.
#[inline(always)]
fn widened_at(table: *const u16, start: usize) -> __m256 {
    widened(unsafe { _mm_loadu_si128(table.add(start).cast()) })
}

/// Eight words widened, as `[words 0–3 | words 4–7]`.
#[inline(always)]
fn widened(words: __m128i) -> __m256 {
    unsafe { _mm256_castsi256_ps(_mm256_cvtepu16_epi32(words)) }
}

/// Eight lanes' eight samples, `[samples 0–3 | samples 4–7]` each, as eight vectors of a
/// sample each: the halves paired across lanes `i` and `i + 4`, then each set of four
/// transposed. Sample `k` is kept where `whole` and, for `k < SPLIT`, `first`, else `second`,
/// are set.
#[inline(always)]
fn pair_transposed<const SPLIT: usize>(
    lanes: [__m256; I32_LANES],
    whole: __m256i,
    first: Avx2Mask,
    second: Avx2Mask,
) -> [__m256; 8] {
    unsafe {
        let [s0, s1, s2, s3] = transpose4([
            _mm256_permute2f128_ps::<0x20>(lanes[0], lanes[4]),
            _mm256_permute2f128_ps::<0x20>(lanes[1], lanes[5]),
            _mm256_permute2f128_ps::<0x20>(lanes[2], lanes[6]),
            _mm256_permute2f128_ps::<0x20>(lanes[3], lanes[7]),
        ]);
        let [s4, s5, s6, s7] = transpose4([
            _mm256_permute2f128_ps::<0x31>(lanes[0], lanes[4]),
            _mm256_permute2f128_ps::<0x31>(lanes[1], lanes[5]),
            _mm256_permute2f128_ps::<0x31>(lanes[2], lanes[6]),
            _mm256_permute2f128_ps::<0x31>(lanes[3], lanes[7]),
        ]);
        let whole = _mm256_castsi256_ps(whole);
        let masks = [
            _mm256_and_ps(whole, first.0),
            _mm256_and_ps(whole, second.0),
        ];
        let samples = [s0, s1, s2, s3, s4, s5, s6, s7];
        let mut kept = samples;
        for (k, kept) in kept.iter_mut().enumerate() {
            *kept = _mm256_and_ps(samples[k], masks[usize::from(k >= SPLIT)]);
        }
        kept
    }
}

/// Four rows of two pixels' four samples, `[pᵢ | pᵢ₊₄]`, as four channels of eight pixels:
/// each 128-bit half transposed as a 4 × 4 block.
#[inline(always)]
fn transpose4(rows: [__m256; 4]) -> [__m256; 4] {
    unsafe {
        let low01 = _mm256_unpacklo_ps(rows[0], rows[1]);
        let high01 = _mm256_unpackhi_ps(rows[0], rows[1]);
        let low23 = _mm256_unpacklo_ps(rows[2], rows[3]);
        let high23 = _mm256_unpackhi_ps(rows[2], rows[3]);
        [
            _mm256_shuffle_ps::<0x44>(low01, low23),
            _mm256_shuffle_ps::<0xEE>(low01, low23),
            _mm256_shuffle_ps::<0x44>(high01, high23),
            _mm256_shuffle_ps::<0xEE>(high01, high23),
        ]
    }
}

/// [`Isa::round_clamped_f64_to_i32`] of one quad: max with zero first, so a NaN lane becomes 0.
#[inline(always)]
fn round4(f64s: Avx2F64, high: Avx2F64) -> __m128i {
    unsafe {
        _mm256_cvtpd_epi32(_mm256_min_pd(
            _mm256_max_pd(f64s.0, _mm256_setzero_pd()),
            high.0,
        ))
    }
}

/// A byte shuffle's control, one 16-byte table for both 128-bit halves.
#[inline(always)]
fn mask(table: &[u8; 16]) -> __m256i {
    unsafe { _mm256_broadcastsi128_si256(_mm_loadu_si128(table.as_ptr().cast())) }
}

/// The or of three shuffles, `sources[i]` through `tables[i]`: one byte of the result from one of
/// the three, where the other two tables write zero.
#[inline(always)]
fn gather3(sources: [__m256i; 3], tables: &[[u8; 16]; 3]) -> __m256i {
    unsafe {
        _mm256_or_si256(
            _mm256_or_si256(
                _mm256_shuffle_epi8(sources[0], mask(&tables[0])),
                _mm256_shuffle_epi8(sources[1], mask(&tables[1])),
            ),
            _mm256_shuffle_epi8(sources[2], mask(&tables[2])),
        )
    }
}

/// A shuffle index that writes zero.
const ZERO: u8 = 0x80;

/// The shuffle that groups four RGBA pixels by channel: `r0 r1 r2 r3 g0 … a3`, its own inverse.
const TRANSPOSE4: [u8; 16] = [0, 4, 8, 12, 1, 5, 9, 13, 2, 6, 10, 14, 3, 7, 11, 15];

/// `SCATTER3[block][channel]`: the bytes of block `block` that `channel` fills, each taken from its
/// pixel's lane.
const SCATTER3: [[[u8; 16]; 3]; 3] = {
    let mut tables = [[[ZERO; 16]; 3]; 3];
    let mut offset = 0;
    while offset < 48 {
        tables[offset / 16][offset % 3][offset % 16] = (offset / 3) as u8;
        offset += 1;
    }
    tables
};

/// The shuffle that keeps the first three bytes of each of four pixels, packed at the bottom of
/// each 128-bit half, and zeroes the last four.
const DROP_FOURTH: [u8; 16] = [
    0, 1, 2, 4, 5, 6, 8, 9, 10, 12, 13, 14, ZERO, ZERO, ZERO, ZERO,
];

/// The shuffle that spreads four pixels of three bytes to four bytes each, the fourth zero.
const SPREAD_THREE: [u8; 16] = [0, 1, 2, ZERO, 3, 4, 5, ZERO, 6, 7, 8, ZERO, 9, 10, 11, ZERO];

/// The fourth byte of each of four pixels.
const FOURTH_BYTES: [u8; 16] = [0, 0, 0, 0xFF, 0, 0, 0, 0xFF, 0, 0, 0, 0xFF, 0, 0, 0, 0xFF];

/// For a quad of three-byte pixels: the shuffles that put bytes 0 and 2, then byte 1 twice, of
/// each pixel into the 16-bit lane pairs [`weighted_quad`] multiplies.
const GATHER_PAIRS3: [[u8; 16]; 2] = [
    [
        0, ZERO, 2, ZERO, 3, ZERO, 5, ZERO, 6, ZERO, 8, ZERO, 9, ZERO, 11, ZERO,
    ],
    [
        1, ZERO, 1, ZERO, 4, ZERO, 4, ZERO, 7, ZERO, 7, ZERO, 10, ZERO, 10, ZERO,
    ],
];

/// [`GATHER_PAIRS3`] for a quad of four-byte pixels.
const GATHER_PAIRS4: [[u8; 16]; 2] = [
    [
        0, ZERO, 2, ZERO, 4, ZERO, 6, ZERO, 8, ZERO, 10, ZERO, 12, ZERO, 14, ZERO,
    ],
    [
        1, ZERO, 1, ZERO, 5, ZERO, 5, ZERO, 9, ZERO, 9, ZERO, 13, ZERO, 13, ZERO,
    ],
];

/// The weights of a weighted sum arranged for `pmaddwd`, with the shuffles that gather a quad's
/// bytes into the lanes they multiply.
///
/// `pmaddwd` multiplies signed 16-bit lanes and adds each pair into an `i32`: the first and third
/// weights ride one pair, and the second, which may pass `i16`, rides the other as two halves that
/// sum back to it. The sum is then the reference's exactly, in 32 bits.
#[derive(Debug, Clone, Copy)]
struct PairWeights {
    outer: __m128i,
    middle: __m128i,
    gather_outer: __m128i,
    gather_middle: __m128i,
}

impl PairWeights {
    #[inline(always)]
    fn new(weights: [u16; 3], gather: &[[u8; 16]; 2]) -> Self {
        let [w0, w1, w2] = weights;
        let (first, third) = (w0.cast_signed(), w2.cast_signed());
        let (low, high) = ((w1 / 2).cast_signed(), (w1 - w1 / 2).cast_signed());
        unsafe {
            Self {
                outer: _mm_setr_epi16(first, third, first, third, first, third, first, third),
                middle: _mm_setr_epi16(low, high, low, high, low, high, low, high),
                gather_outer: _mm_loadu_si128(gather[0].as_ptr().cast()),
                gather_middle: _mm_loadu_si128(gather[1].as_ptr().cast()),
            }
        }
    }
}

/// The weighted sums of the four pixels a quad holds from byte zero, one per `i32` lane, out of
/// Q16: `(sum + 0x7FFF + odd) >> 16`, `odd` the quotient's lowest bit.
#[inline(always)]
fn weighted_quad(quad: __m128i, weights: PairWeights) -> __m128i {
    unsafe {
        let sum = _mm_add_epi32(
            _mm_madd_epi16(_mm_shuffle_epi8(quad, weights.gather_outer), weights.outer),
            _mm_madd_epi16(
                _mm_shuffle_epi8(quad, weights.gather_middle),
                weights.middle,
            ),
        );
        let odd = _mm_and_si128(_mm_srli_epi32::<16>(sum), _mm_set1_epi32(1));
        _mm_srli_epi32::<16>(_mm_add_epi32(
            _mm_add_epi32(sum, _mm_set1_epi32(0x7FFF)),
            odd,
        ))
    }
}

/// Sixteen weighted sums, four `i32` lanes at a time, packed into 16 bytes: each is a byte
/// already, so neither saturating pack bites.
#[inline(always)]
fn pack_quads(sums: [__m128i; 4]) -> __m128i {
    unsafe {
        _mm_packus_epi16(
            _mm_packs_epi32(sums[0], sums[1]),
            _mm_packs_epi32(sums[2], sums[3]),
        )
    }
}

/// Thirty-two pixels of three bytes: each 128-bit half interleaves its own sixteen pixels, each
/// output block is three shuffles,
/// one from each channel, or-ed together, and the blocks go to their stores.
#[inline(always)]
fn store_rgb_u8(channels: [__m256i; 3], bytes: &mut [u8; 3 * U8_LANES]) {
    let (out, []) = bytes.as_chunks_mut::<32>() else {
        unreachable!("ninety-six bytes are three stores of thirty-two")
    };
    unsafe {
        let p0 = gather3(channels, &SCATTER3[0]);
        let p1 = gather3(channels, &SCATTER3[1]);
        let p2 = gather3(channels, &SCATTER3[2]);
        let stores = [
            _mm256_permute2x128_si256::<0x20>(p0, p1),
            _mm256_blend_epi32::<0xF0>(p2, p0),
            _mm256_permute2x128_si256::<0x31>(p1, p2),
        ];
        _mm256_storeu_si256(out[0].as_mut_ptr().cast(), stores[0]);
        _mm256_storeu_si256(out[1].as_mut_ptr().cast(), stores[1]);
        _mm256_storeu_si256(out[2].as_mut_ptr().cast(), stores[2]);
    }
}

/// Each 128-bit half interleaves its own sixteen pixels: a four-by-four transpose of the
/// channels' 32-bit groups, then one shuffle a block that puts each pixel's four bytes together.
#[inline(always)]
fn store_rgba_u8(channels: [__m256i; 4], bytes: &mut [u8; 4 * U8_LANES]) {
    let (out, []) = bytes.as_chunks_mut::<32>() else {
        unreachable!("a hundred and twenty-eight bytes are four stores of thirty-two")
    };
    let [r, g, b, a] = channels;
    unsafe {
        let (rg_low, rg_high) = (_mm256_unpacklo_epi32(r, g), _mm256_unpackhi_epi32(r, g));
        let (ba_low, ba_high) = (_mm256_unpacklo_epi32(b, a), _mm256_unpackhi_epi32(b, a));
        let group = mask(&TRANSPOSE4);
        let p0 = _mm256_shuffle_epi8(_mm256_unpacklo_epi64(rg_low, ba_low), group);
        let p1 = _mm256_shuffle_epi8(_mm256_unpackhi_epi64(rg_low, ba_low), group);
        let p2 = _mm256_shuffle_epi8(_mm256_unpacklo_epi64(rg_high, ba_high), group);
        let p3 = _mm256_shuffle_epi8(_mm256_unpackhi_epi64(rg_high, ba_high), group);
        _mm256_storeu_si256(
            out[0].as_mut_ptr().cast(),
            _mm256_permute2x128_si256::<0x20>(p0, p1),
        );
        _mm256_storeu_si256(
            out[1].as_mut_ptr().cast(),
            _mm256_permute2x128_si256::<0x20>(p2, p3),
        );
        _mm256_storeu_si256(
            out[2].as_mut_ptr().cast(),
            _mm256_permute2x128_si256::<0x31>(p0, p1),
        );
        _mm256_storeu_si256(
            out[3].as_mut_ptr().cast(),
            _mm256_permute2x128_si256::<0x31>(p2, p3),
        );
    }
}
