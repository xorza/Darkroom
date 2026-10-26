//! [`Neon`]: the aarch64 [`Isa`], two 128-bit registers per vector.
//!
//! NEON is part of the aarch64 baseline, so a [`Neon`] needs no detection and [`Isa::run`] no
//! `#[target_feature]` entry, and every intrinsic here is sound to call on any operands. Loads and
//! stores take arrays of exactly one vector's width, split into the two registers' halves.
//!
//! NEON's own `vmaxq`/`vminq` return NaN where either lane is NaN, which is not the compare-swap
//! [`F32x8::max`] specifies, so they are built from a compare and a bit select instead; only
//! [`F32x8::min_positive`], whose bound is never NaN, takes `vminq`.

pub(crate) mod f32x8;
pub(crate) mod f64x4;
pub(crate) mod i32x8;
pub(crate) mod mask8;
pub(crate) mod u16x16;
pub(crate) mod u8x32;

use std::arch::aarch64::*;

use crate::backend::neon::f32x8::NeonF32;
use crate::backend::neon::f64x4::NeonF64;
use crate::backend::neon::i32x8::NeonI32;
use crate::backend::neon::mask8::NeonMask;
use crate::backend::neon::u8x32::NeonU8;
use crate::backend::neon::u16x16::NeonU16;
use crate::f32x8::F32_LANES;
use crate::f64x4::F64_LANES;
use crate::halves::Halves;
use crate::i32x8::{I32_LANES, I32x8};
use crate::isa::{Isa, pair_lanes, u32_le_lanes};
use crate::mask8::Mask8;
use crate::u8x32::U8_LANES;
use crate::u16x16::U16_LANES;

/// NEON, which every aarch64 CPU has.
#[derive(Debug, Default, Clone, Copy)]
pub struct Neon {
    _private: (),
}

impl Neon {
    pub const fn new() -> Self {
        Self { _private: () }
    }
}

impl Isa for Neon {
    type F32 = NeonF32;
    type F64 = NeonF64;
    type U8 = NeonU8;
    type U16 = NeonU16;
    type I32 = NeonI32;

    #[inline(always)]
    fn splat_f32(self, value: f32) -> NeonF32 {
        let lanes = unsafe { vdupq_n_f32(value) };
        NeonF32 {
            low: lanes,
            high: lanes,
        }
    }

    #[inline(always)]
    fn splat_f64(self, value: f64) -> NeonF64 {
        let lanes = unsafe { vdupq_n_f64(value) };
        NeonF64 {
            low: lanes,
            high: lanes,
        }
    }

    #[inline(always)]
    fn load_f32(self, lanes: &[f32; F32_LANES]) -> NeonF32 {
        let (halves, []) = lanes.as_chunks::<4>() else {
            unreachable!("eight lanes are two halves of four")
        };
        unsafe {
            NeonF32 {
                low: vld1q_f32(halves[0].as_ptr()),
                high: vld1q_f32(halves[1].as_ptr()),
            }
        }
    }

    #[inline(always)]
    fn load_f64(self, lanes: &[f64; F64_LANES]) -> NeonF64 {
        let (halves, []) = lanes.as_chunks::<2>() else {
            unreachable!("four lanes are two halves of two")
        };
        unsafe {
            NeonF64 {
                low: vld1q_f64(halves[0].as_ptr()),
                high: vld1q_f64(halves[1].as_ptr()),
            }
        }
    }

    #[inline(always)]
    fn load_u8(self, lanes: &[u8; U8_LANES]) -> NeonU8 {
        let (halves, []) = lanes.as_chunks::<16>() else {
            unreachable!("thirty-two lanes are two halves of sixteen")
        };
        unsafe {
            NeonU8 {
                low: vld1q_u8(halves[0].as_ptr()),
                high: vld1q_u8(halves[1].as_ptr()),
            }
        }
    }

    #[inline(always)]
    fn load_u16(self, lanes: &[u16; U16_LANES]) -> NeonU16 {
        let (halves, []) = lanes.as_chunks::<8>() else {
            unreachable!("sixteen lanes are two halves of eight")
        };
        unsafe {
            NeonU16 {
                low: vld1q_u16(halves[0].as_ptr()),
                high: vld1q_u16(halves[1].as_ptr()),
            }
        }
    }

    #[inline(always)]
    fn splat_u8(self, value: u8) -> NeonU8 {
        let lanes = unsafe { vdupq_n_u8(value) };
        NeonU8 {
            low: lanes,
            high: lanes,
        }
    }

    /// `ld4` and `st3`, sixteen pixels at a time.
    #[inline(always)]
    fn drop_fourth_u8(self, pixels: &[u8; 4 * U8_LANES], out: &mut [u8; 3 * U8_LANES]) {
        let (sources, []) = pixels.as_chunks::<64>() else {
            unreachable!("thirty-two pixels are two halves of sixteen")
        };
        let (outs, []) = out.as_chunks_mut::<48>() else {
            unreachable!("thirty-two pixels are two halves of sixteen")
        };
        for (source, out) in sources.iter().zip(outs) {
            unsafe {
                let rgba = vld4q_u8(source.as_ptr());
                vst3q_u8(out.as_mut_ptr(), uint8x16x3_t(rgba.0, rgba.1, rgba.2));
            }
        }
    }

    /// Both `ld3`s, then [`interleave4`] on each half.
    #[inline(always)]
    fn append_fourth_u8(
        self,
        pixels: &[u8; 3 * U8_LANES],
        fourth: u8,
        out: &mut [u8; 4 * U8_LANES],
    ) {
        let (sources, []) = pixels.as_chunks::<48>() else {
            unreachable!("thirty-two pixels are two halves of sixteen")
        };
        let (low, high) = unsafe { (vld3q_u8(sources[0].as_ptr()), vld3q_u8(sources[1].as_ptr())) };
        let fourth = unsafe { vdupq_n_u8(fourth) };
        self.store_interleaved_u8(
            [
                NeonU8 {
                    low: low.0,
                    high: high.0,
                },
                NeonU8 {
                    low: low.1,
                    high: high.1,
                },
                NeonU8 {
                    low: low.2,
                    high: high.2,
                },
                NeonU8 {
                    low: fourth,
                    high: fourth,
                },
            ],
            out,
        );
    }

    /// `ld3`, then `umull`/`umlal`: the widening multiply-accumulate takes unsigned 16-bit
    /// weights whole, so no weight is split.
    #[inline(always)]
    fn weighted_sum3_u8(self, pixels: &[u8; 3 * U8_LANES], weights: [u16; 3]) -> NeonU8 {
        let (sources, []) = pixels.as_chunks::<48>() else {
            unreachable!("thirty-two pixels are two halves of sixteen")
        };
        NeonU8 {
            low: weighted_sum16_of3(&sources[0], weights),
            high: weighted_sum16_of3(&sources[1], weights),
        }
    }

    /// [`Self::weighted_sum3_u8`] through `ld4`, the fourth channel dropped.
    #[inline(always)]
    fn weighted_sum3_of4_u8(self, pixels: &[u8; 4 * U8_LANES], weights: [u16; 3]) -> NeonU8 {
        let (sources, []) = pixels.as_chunks::<64>() else {
            unreachable!("thirty-two pixels are two halves of sixteen")
        };
        NeonU8 {
            low: weighted_sum16_of4(&sources[0], weights),
            high: weighted_sum16_of4(&sources[1], weights),
        }
    }

    /// Three channels as an `st3` of each half, four as [`interleave4`] on each half.
    #[inline(always)]
    fn store_interleaved_u8<const C: usize, const G: usize>(
        self,
        channels: [NeonU8; C],
        bytes: &mut [u8; G],
    ) {
        const { assert!((C == 3 || C == 4) && G == C * U8_LANES) };
        let c = channels.as_slice();
        if C == 3 {
            let (halves, []) = bytes.as_chunks_mut::<48>() else {
                unreachable!("ninety-six bytes are two halves of forty-eight")
            };
            unsafe {
                vst3q_u8(
                    halves[0].as_mut_ptr(),
                    uint8x16x3_t(c[0].low, c[1].low, c[2].low),
                );
                vst3q_u8(
                    halves[1].as_mut_ptr(),
                    uint8x16x3_t(c[0].high, c[1].high, c[2].high),
                );
            }
        } else {
            let (halves, []) = bytes.as_chunks_mut::<64>() else {
                unreachable!("a hundred and twenty-eight bytes are two halves of sixty-four")
            };
            interleave4([c[0].low, c[1].low, c[2].low, c[3].low], &mut halves[0]);
            interleave4([c[0].high, c[1].high, c[2].high, c[3].high], &mut halves[1]);
        }
    }

    /// `fcvtnu` rounds to nearest, ties to even, and saturates a negative or NaN lane to 0.
    #[inline(always)]
    fn round_clamped_f32_to_i32(self, values: NeonF32, high: NeonF32) -> NeonI32 {
        NeonI32 {
            low: round_clamped_f32_to_i32(values.low, high.low),
            high: round_clamped_f32_to_i32(values.high, high.high),
        }
    }

    #[inline(always)]
    fn load_i32(self, lanes: &[i32; I32_LANES]) -> NeonI32 {
        let (halves, []) = lanes.as_chunks::<4>() else {
            unreachable!("eight lanes are two halves of four")
        };
        unsafe {
            NeonI32 {
                low: vld1q_s32(halves[0].as_ptr()),
                high: vld1q_s32(halves[1].as_ptr()),
            }
        }
    }

    #[inline(always)]
    fn splat_u16(self, value: u16) -> NeonU16 {
        let lanes = unsafe { vdupq_n_u16(value) };
        NeonU16 {
            low: lanes,
            high: lanes,
        }
    }

    #[inline(always)]
    fn splat_i32(self, value: i32) -> NeonI32 {
        let lanes = unsafe { vdupq_n_s32(value) };
        NeonI32 {
            low: lanes,
            high: lanes,
        }
    }

    /// `st4` and `st3` interleave four or three registers into memory in one instruction, once
    /// for each half.
    #[inline(always)]
    fn store_interleaved_f32<const C: usize, const G: usize>(
        self,
        channels: [NeonF32; C],
        samples: &mut [f32; G],
    ) {
        const { assert!((C == 3 || C == 4) && G == C * F32_LANES) };
        let (at, c) = (samples.as_mut_ptr(), channels.as_slice());
        unsafe {
            if C == 4 {
                vst4q_f32(at, float32x4x4_t(c[0].low, c[1].low, c[2].low, c[3].low));
                vst4q_f32(
                    at.add(16),
                    float32x4x4_t(c[0].high, c[1].high, c[2].high, c[3].high),
                );
            } else {
                vst3q_f32(at, float32x4x3_t(c[0].low, c[1].low, c[2].low));
                vst3q_f32(at.add(12), float32x4x3_t(c[0].high, c[1].high, c[2].high));
            }
        }
    }

    /// Each channel narrowed to eight words, then one `st4` or `st3`.
    #[inline(always)]
    fn store_interleaved_u16<const C: usize, const G: usize>(
        self,
        channels: [NeonI32; C],
        words: &mut [u16; G],
    ) {
        const { assert!((C == 3 || C == 4) && G == C * I32_LANES) };
        let channels = channels.as_slice();
        unsafe {
            if C == 4 {
                vst4q_u16(
                    words.as_mut_ptr(),
                    uint16x8x4_t(
                        channels[0].narrowed(),
                        channels[1].narrowed(),
                        channels[2].narrowed(),
                        channels[3].narrowed(),
                    ),
                );
            } else {
                vst3q_u16(
                    words.as_mut_ptr(),
                    uint16x8x3_t(
                        channels[0].narrowed(),
                        channels[1].narrowed(),
                        channels[2].narrowed(),
                    ),
                );
            }
        }
    }

    #[inline(always)]
    fn load_u8_as_i32(self, bytes: &[u8; I32_LANES]) -> NeonI32 {
        unsafe {
            let words = vmovl_u8(vld1_u8(bytes.as_ptr()));
            NeonI32 {
                low: vreinterpretq_s32_u32(vmovl_u16(vget_low_u16(words))),
                high: vreinterpretq_s32_u32(vmovl_high_u16(words)),
            }
        }
    }

    /// Eight scalar loads, each lane's four bytes from its offset or, in a lane not read, from
    /// the first byte, then the lanes not read cleared: NEON has no gather. A lane whose four
    /// bytes run past the end, rare, sends the vector lane by lane.
    #[inline(always)]
    fn gather_u32_le(self, bytes: &[u8], offset: NeonI32, valid: NeonMask) -> NeonI32 {
        let reads = Reads::new(bytes.len(), 4, offset, valid);
        if reads.partial {
            return self.load_i32(
                &u32_le_lanes(bytes, offset.to_array(), valid.to_bitmask()).map(u32::cast_signed),
            );
        }
        let offsets = reads.starts(offset);
        let mut words = [0; I32_LANES];
        for (word, offset) in words.iter_mut().zip(offsets) {
            *word =
                i32::from_le(unsafe { bytes.as_ptr().add(offset).cast::<i32>().read_unaligned() });
        }
        reads.keep(self.load_i32(&words))
    }

    /// Sixteen loads, a lane's eight samples each from its start or, in a lane not read, from
    /// the table's first, then four 4 × 4 transposes.
    #[inline(always)]
    fn gather_pair_f32<const SPLIT: usize>(
        self,
        table: &[f32],
        start: NeonI32,
        first: NeonMask,
        second: NeonMask,
    ) -> [NeonF32; 8] {
        let reads = Reads::new(table.len(), 8, start, first.or(second));
        if reads.partial {
            let bits = [first.to_bitmask(), second.to_bitmask()];
            return pair_lanes::<f32, SPLIT>(table, start.to_array(), bits)
                .map(|lanes| self.load_f32(&lanes));
        }
        let starts = reads.starts(start);
        let lanes = [
            f32_pair_at(table, starts[0]),
            f32_pair_at(table, starts[1]),
            f32_pair_at(table, starts[2]),
            f32_pair_at(table, starts[3]),
            f32_pair_at(table, starts[4]),
            f32_pair_at(table, starts[5]),
            f32_pair_at(table, starts[6]),
            f32_pair_at(table, starts[7]),
        ];
        let [s0, s1, s2, s3, s4, s5, s6, s7] =
            pair_transposed::<SPLIT>(lanes, reads.whole, first, second);
        [
            s0.as_f32(),
            s1.as_f32(),
            s2.as_f32(),
            s3.as_f32(),
            s4.as_f32(),
            s5.as_f32(),
            s6.as_f32(),
            s7.as_f32(),
        ]
    }

    /// [`Self::gather_pair_f32`]'s loads and transposes, each lane's eight words widened first.
    #[inline(always)]
    fn gather_pair_u16<const SPLIT: usize>(
        self,
        table: &[u16],
        start: NeonI32,
        first: NeonMask,
        second: NeonMask,
    ) -> [NeonI32; 8] {
        let reads = Reads::new(table.len(), 8, start, first.or(second));
        if reads.partial {
            let bits = [first.to_bitmask(), second.to_bitmask()];
            return pair_lanes::<u16, SPLIT>(table, start.to_array(), bits)
                .map(|lanes| self.load_i32(&lanes.map(i32::from)));
        }
        let starts = reads.starts(start);
        let lanes = [
            u16_pair_at(table, starts[0]),
            u16_pair_at(table, starts[1]),
            u16_pair_at(table, starts[2]),
            u16_pair_at(table, starts[3]),
            u16_pair_at(table, starts[4]),
            u16_pair_at(table, starts[5]),
            u16_pair_at(table, starts[6]),
            u16_pair_at(table, starts[7]),
        ];
        pair_transposed::<SPLIT>(lanes, reads.whole, first, second)
    }

    /// `fcvtnu` rounds to nearest, ties to even, and saturates a negative or NaN lane to 0, so
    /// only the high bound is a separate step; a lane up to `i32::MAX` then narrows exactly.
    #[inline(always)]
    fn round_clamped_f64_to_i32(self, f64s: Halves<NeonF64>, high: NeonF64) -> NeonI32 {
        NeonI32 {
            low: round_clamped_f64_quad(f64s.low, high),
            high: round_clamped_f64_quad(f64s.high, high),
        }
    }

    #[inline(always)]
    fn narrow_i32_to_u8(self, ints: [NeonI32; 4]) -> NeonU8 {
        unsafe {
            NeonU8 {
                low: vcombine_u8(vmovn_u16(ints[0].narrowed()), vmovn_u16(ints[1].narrowed())),
                high: vcombine_u8(vmovn_u16(ints[2].narrowed()), vmovn_u16(ints[3].narrowed())),
            }
        }
    }
}

/// [`weighted_sum16`] of sixteen pixels of three bytes.
#[inline(always)]
fn weighted_sum16_of3(pixels: &[u8; 48], weights: [u16; 3]) -> uint8x16_t {
    let rgb = unsafe { vld3q_u8(pixels.as_ptr()) };
    weighted_sum16(rgb.0, rgb.1, rgb.2, weights)
}

/// [`weighted_sum16`] of sixteen pixels of four bytes, the fourth dropped.
#[inline(always)]
fn weighted_sum16_of4(pixels: &[u8; 64], weights: [u16; 3]) -> uint8x16_t {
    let rgba = unsafe { vld4q_u8(pixels.as_ptr()) };
    weighted_sum16(rgba.0, rgba.1, rgba.2, weights)
}

/// [`Isa::round_clamped_f32_to_i32`] of four lanes.
#[inline(always)]
fn round_clamped_f32_to_i32(values: float32x4_t, high: float32x4_t) -> int32x4_t {
    unsafe { vreinterpretq_s32_u32(vcvtnq_u32_f32(vminq_f32(values, high))) }
}

/// [`Isa::round_clamped_f64_to_i32`] of four lanes, bounded by `high`'s lanes as its own.
#[inline(always)]
fn round_clamped_f64_quad(f64s: NeonF64, high: NeonF64) -> int32x4_t {
    unsafe {
        vreinterpretq_s32_u32(vcombine_u32(
            round_clamped_pair(f64s.low, high.low),
            round_clamped_pair(f64s.high, high.high),
        ))
    }
}

/// [`Isa::round_clamped_f64_to_i32`] of two lanes.
#[inline(always)]
fn round_clamped_pair(values: float64x2_t, high: float64x2_t) -> uint32x2_t {
    unsafe { vmovn_u64(vcvtnq_u64_f64(vminq_f64(values, high))) }
}

/// The eight samples of `table` from `start`, a lane of a gather [`Reads`] admitted whole.
#[inline(always)]
fn f32_pair_at(table: &[f32], start: usize) -> [uint32x4_t; 2] {
    debug_assert!(start + 8 <= table.len(), "a whole lane's eight samples");
    unsafe {
        let pair = vld1q_f32_x2(table.as_ptr().add(start));
        [vreinterpretq_u32_f32(pair.0), vreinterpretq_u32_f32(pair.1)]
    }
}

/// The eight words of `table` from `start`, widened, a lane of a gather [`Reads`] admitted whole.
#[inline(always)]
fn u16_pair_at(table: &[u16], start: usize) -> [uint32x4_t; 2] {
    debug_assert!(start + 8 <= table.len(), "a whole lane's eight words");
    unsafe {
        let words = vld1q_u16(table.as_ptr().add(start));
        [vmovl_u16(vget_low_u16(words)), vmovl_high_u16(words)]
    }
}

/// The sixteen bytes of `channel` widened, as four quarters of four words.
#[inline(always)]
fn quarters(channel: uint8x16_t) -> [uint16x4_t; 4] {
    unsafe {
        let low = vmovl_u8(vget_low_u8(channel));
        let high = vmovl_high_u8(channel);
        [
            vget_low_u16(low),
            vget_high_u16(low),
            vget_low_u16(high),
            vget_high_u16(high),
        ]
    }
}

/// `weighted_sum_q16` of four pixels given as their channels' quarters.
#[inline(always)]
fn weighted_sum4(channels: [uint16x4_t; 3], weights: [u16; 3]) -> uint16x4_t {
    let [a, b, c] = channels;
    let [w0, w1, w2] = weights;
    unsafe {
        let sum = vmlal_n_u16(vmlal_n_u16(vmull_n_u16(a, w0), b, w1), c, w2);
        let odd = vandq_u32(vshrq_n_u32::<16>(sum), vdupq_n_u32(1));
        vshrn_n_u32::<16>(vaddq_u32(vaddq_u32(sum, vdupq_n_u32(0x7FFF)), odd))
    }
}

/// Sixteen pixels of four channels into `out`, as two rounds of `zip` and one `st1` of four
/// registers. Two `st4`s store the same bytes, but measured up to a sixth slower on an Apple core
/// in one loop body, by the order the compiler gives them.
#[inline(always)]
fn interleave4(channels: [uint8x16_t; 4], out: &mut [u8; 64]) {
    let [r, g, b, a] = channels;
    unsafe {
        let rg = [
            vreinterpretq_u16_u8(vzip1q_u8(r, g)),
            vreinterpretq_u16_u8(vzip2q_u8(r, g)),
        ];
        let ba = [
            vreinterpretq_u16_u8(vzip1q_u8(b, a)),
            vreinterpretq_u16_u8(vzip2q_u8(b, a)),
        ];
        vst1q_u8_x4(
            out.as_mut_ptr(),
            uint8x16x4_t(
                vreinterpretq_u8_u16(vzip1q_u16(rg[0], ba[0])),
                vreinterpretq_u8_u16(vzip2q_u16(rg[0], ba[0])),
                vreinterpretq_u8_u16(vzip1q_u16(rg[1], ba[1])),
                vreinterpretq_u8_u16(vzip2q_u16(rg[1], ba[1])),
            ),
        );
    }
}

/// [`Isa::weighted_sum3_u8`] of sixteen pixels given as their channels.
#[inline(always)]
fn weighted_sum16(c0: uint8x16_t, c1: uint8x16_t, c2: uint8x16_t, weights: [u16; 3]) -> uint8x16_t {
    let (a, b, c) = (quarters(c0), quarters(c1), quarters(c2));
    let words = [
        weighted_sum4([a[0], b[0], c[0]], weights),
        weighted_sum4([a[1], b[1], c[1]], weights),
        weighted_sum4([a[2], b[2], c[2]], weights),
        weighted_sum4([a[3], b[3], c[3]], weights),
    ];
    unsafe {
        vcombine_u8(
            vmovn_u16(vcombine_u16(words[0], words[1])),
            vmovn_u16(vcombine_u16(words[2], words[3])),
        )
    }
}

/// The lanes a gather of `width` elements a lane from a table of `len` loads whole: `whole`,
/// those `valid` sets whose elements all lie in the table; `partial`, whether a lane `valid` sets
/// has an element outside it, or the table is shorter than one lane's read, which sends the
/// gather lane by lane. Only an image's first and last pixels run so.
#[derive(Debug, Clone, Copy)]
struct Reads {
    whole: NeonMask,
    partial: bool,
}

impl Reads {
    #[inline(always)]
    fn new(len: usize, width: usize, start: NeonI32, valid: NeonMask) -> Self {
        let whole = in_range(len.saturating_sub(width - 1), start, valid);
        let outside = unsafe {
            vmaxvq_u32(vorrq_u32(
                vbicq_u32(valid.low, whole.low),
                vbicq_u32(valid.high, whole.high),
            ))
        };
        Self {
            whole,
            partial: len < width || outside != 0,
        }
    }

    /// Each lane's start where it reads whole, and the table's first element elsewhere: a
    /// start whose `width` elements lie in the table, as [`Self::new`] found them.
    #[inline(always)]
    fn starts(self, start: NeonI32) -> [usize; I32_LANES] {
        let starts = self.keep(start).to_array();
        let mut lanes = [0; I32_LANES];
        for (lane, start) in lanes.iter_mut().zip(starts) {
            *lane = start.cast_unsigned() as usize;
        }
        lanes
    }

    /// `ints` in the lanes read whole, 0 elsewhere.
    #[inline(always)]
    fn keep(self, ints: NeonI32) -> NeonI32 {
        keep(self.whole, ints)
    }
}

/// The lanes `valid` sets whose index is in `[0, end)`. An `end` past `i32::MAX` holds every
/// non-negative index.
#[inline(always)]
fn in_range(end: usize, idx: NeonI32, valid: NeonMask) -> NeonMask {
    let end = unsafe { vdupq_n_s32(i32::try_from(end).unwrap_or(i32::MAX)) };
    NeonMask {
        low: in_range4(end, idx.low, valid.low),
        high: in_range4(end, idx.high, valid.high),
    }
}

/// [`in_range`] of four lanes, `end` splat.
#[inline(always)]
fn in_range4(end: int32x4_t, idx: int32x4_t, valid: uint32x4_t) -> uint32x4_t {
    unsafe {
        let below_end = vcltq_s32(idx, end);
        let non_negative = vcgezq_s32(idx);
        vandq_u32(valid, vandq_u32(below_end, non_negative))
    }
}

/// Eight lanes' four samples, pixels 0–3 then 4–7, as four channels of eight lanes: each half a
/// 4 × 4 transpose.
#[inline(always)]
fn transposed(pixels: [uint32x4_t; I32_LANES]) -> [NeonI32; 4] {
    let [l0, l1, l2, l3] = transposed4([pixels[0], pixels[1], pixels[2], pixels[3]]);
    let [h0, h1, h2, h3] = transposed4([pixels[4], pixels[5], pixels[6], pixels[7]]);
    [
        NeonI32 { low: l0, high: h0 },
        NeonI32 { low: l1, high: h1 },
        NeonI32 { low: l2, high: h2 },
        NeonI32 { low: l3, high: h3 },
    ]
}

/// Eight lanes' eight samples, two quads each, as eight vectors of a sample each. Sample `k` is
/// kept where `whole` and, for `k < SPLIT`, `first`, else `second`, are set.
#[inline(always)]
fn pair_transposed<const SPLIT: usize>(
    lanes: [[uint32x4_t; 2]; I32_LANES],
    whole: NeonMask,
    first: NeonMask,
    second: NeonMask,
) -> [NeonI32; 8] {
    let [s0, s1, s2, s3] = transposed([
        lanes[0][0],
        lanes[1][0],
        lanes[2][0],
        lanes[3][0],
        lanes[4][0],
        lanes[5][0],
        lanes[6][0],
        lanes[7][0],
    ]);
    let [s4, s5, s6, s7] = transposed([
        lanes[0][1],
        lanes[1][1],
        lanes[2][1],
        lanes[3][1],
        lanes[4][1],
        lanes[5][1],
        lanes[6][1],
        lanes[7][1],
    ]);
    let masks = [whole.and(first), whole.and(second)];
    let samples = [s0, s1, s2, s3, s4, s5, s6, s7];
    let mut kept = samples;
    for (k, kept) in kept.iter_mut().enumerate() {
        *kept = keep(masks[usize::from(k >= SPLIT)], samples[k]);
    }
    kept
}

/// `ints` in the lanes `mask` sets, 0 elsewhere.
#[inline(always)]
fn keep(mask: NeonMask, ints: NeonI32) -> NeonI32 {
    unsafe {
        NeonI32 {
            low: vandq_s32(vreinterpretq_s32_u32(mask.low), ints.low),
            high: vandq_s32(vreinterpretq_s32_u32(mask.high), ints.high),
        }
    }
}

/// Four rows of four lanes as four columns: `trn1`/`trn2` of the 32-bit pairs, then of the
/// 64-bit halves.
#[inline(always)]
fn transposed4(rows: [uint32x4_t; 4]) -> [int32x4_t; 4] {
    unsafe {
        let even01 = vreinterpretq_u64_u32(vtrn1q_u32(rows[0], rows[1]));
        let odd01 = vreinterpretq_u64_u32(vtrn2q_u32(rows[0], rows[1]));
        let even23 = vreinterpretq_u64_u32(vtrn1q_u32(rows[2], rows[3]));
        let odd23 = vreinterpretq_u64_u32(vtrn2q_u32(rows[2], rows[3]));
        [
            vreinterpretq_s32_u64(vtrn1q_u64(even01, even23)),
            vreinterpretq_s32_u64(vtrn1q_u64(odd01, odd23)),
            vreinterpretq_s32_u64(vtrn2q_u64(even01, even23)),
            vreinterpretq_s32_u64(vtrn2q_u64(odd01, odd23)),
        ]
    }
}
