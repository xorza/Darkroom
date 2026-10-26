//! [`Isa`]: an instruction set, as a token that only exists on a CPU that has it.

use std::array;
use std::fmt::Debug;

use crate::f32x8::{F32_LANES, F32x8};
use crate::f64x4::{F64_LANES, F64x4};
use crate::halves::Halves;
use crate::i32x8::{I32_LANES, I32x8};
use crate::kernel::Kernel;
use crate::mask8::Mask8;
use crate::u8x32::{U8_LANES, U8x32};
use crate::u16x16::{U16_LANES, U16x16};

/// An instruction set, as a zero-sized token that only exists on a CPU that has it.
///
/// Its methods make vectors; the vectors' own methods ([`F32x8`], [`F64x4`], [`U8x32`],
/// [`U16x16`], [`I32x8`]) combine them.
///
/// A method with a body is a composite op: the body is its definition, in plain lane loops, and
/// `Portable` takes it as it is. A hardware Isa overrides it only where its own instructions beat
/// the default, and every override is held to the default lane for lane by the tests.
pub trait Isa: Copy + Debug {
    type F32: F32x8<F64 = Self::F64, I32 = Self::I32>;
    type F64: F64x4;
    type U8: U8x32<U16 = Self::U16>;
    type U16: U16x16<U8 = Self::U8, I32 = Self::I32>;
    type I32: I32x8<F32 = Self::F32, U16 = Self::U16>;

    /// `kernel` compiled for this Isa's features.
    fn run<K: Kernel>(self, kernel: K) -> K::Output;

    fn splat_f32(self, value: f32) -> Self::F32;

    fn splat_f64(self, value: f64) -> Self::F64;

    fn load_f32(self, lanes: &[f32; F32_LANES]) -> Self::F32;

    fn load_f64(self, lanes: &[f64; F64_LANES]) -> Self::F64;

    fn load_u8(self, lanes: &[u8; U8_LANES]) -> Self::U8;

    fn load_u16(self, lanes: &[u16; U16_LANES]) -> Self::U16;

    fn load_i32(self, lanes: &[i32; I32_LANES]) -> Self::I32;

    fn splat_u8(self, value: u8) -> Self::U8;

    fn splat_u16(self, value: u16) -> Self::U16;

    fn splat_i32(self, value: i32) -> Self::I32;

    /// In each lane `valid` sets whose `offset` lies in `bytes`, the four bytes from it as a
    /// little-endian `u32`, the bytes past the end of `bytes` read as zero; 0 in every other lane.
    /// A lane can never read outside `bytes`.
    #[inline(always)]
    fn gather_u32_le(self, bytes: &[u8], offset: Self::I32, valid: Mask<Self>) -> Self::I32 {
        let words = u32_le_lanes(bytes, lanes_of(offset), valid.to_bitmask());
        self.load_i32(&words.map(u32::cast_signed))
    }

    /// The eight samples of `table` from each lane's `start`, transposed: lane `i` of vector `k`
    /// is `table[startᵢ + k]`. They are two pixels side by side: samples `0..SPLIT` are read in
    /// the lanes `first` sets, and samples `SPLIT..8` in those `second` sets. A sample not read,
    /// or outside the table at either end, is +0: a pair may start before the table and end in
    /// it. A lane can never read outside `table`.
    #[inline(always)]
    fn gather_pair_f32<const SPLIT: usize>(
        self,
        table: &[f32],
        start: Self::I32,
        first: Mask<Self>,
        second: Mask<Self>,
    ) -> [Self::F32; 8] {
        let bits = [first, second].map(Mask8::to_bitmask);
        pair_lanes::<f32, SPLIT>(table, lanes_of(start), bits).map(|lanes| self.load_f32(&lanes))
    }

    /// [`Self::gather_pair_f32`] of `u16` samples, each zero-extended.
    #[inline(always)]
    fn gather_pair_u16<const SPLIT: usize>(
        self,
        table: &[u16],
        start: Self::I32,
        first: Mask<Self>,
        second: Mask<Self>,
    ) -> [Self::I32; 8] {
        let bits = [first, second].map(Mask8::to_bitmask);
        pair_lanes::<u16, SPLIT>(table, lanes_of(start), bits)
            .map(|lanes| self.load_i32(&lanes.map(i32::from)))
    }

    /// Eight bytes, each zero-extended to an `i32` lane: one load where [`U8x32::widen`] twice
    /// takes four shuffles.
    #[inline(always)]
    fn load_u8_as_i32(self, bytes: &[u8; I32_LANES]) -> Self::I32 {
        self.load_i32(&bytes.map(i32::from))
    }

    /// The eight lanes of `f64s`, low then high, each clamped to `[0, high]` and rounded to the
    /// nearest integer, ties to even: a NaN to 0. Each lane of `high` is an integer of at most
    /// `i32::MAX`; another `high` breaks the contract, which `Portable` asserts in a debug build.
    fn round_clamped(self, f64s: Halves<Self::F64>, high: Self::F64) -> Self::I32;

    /// [`Self::round_clamped`] of eight f32 lanes; each lane of `high` is an integer of at most
    /// 2²⁴, which f32 holds exactly, and `Portable` asserts it in a debug build.
    fn round_clamped_f32(self, values: Self::F32, high: Self::F32) -> Self::I32;

    /// The lanes of `ints`, in order, each in `[0, 255]`, as bytes. A lane outside it breaks the
    /// contract, which `Portable` asserts in a debug build; the other Isas then give an
    /// unspecified value. Each Isa narrows with its cheapest instruction.
    fn narrow_i32_to_u8(self, ints: [Self::I32; 4]) -> Self::U8;

    /// [`Self::round_clamped`] of `f64s`, in order, as words; `high` is at most 65535.
    #[inline(always)]
    fn round_clamped_u16(self, f64s: [Halves<Self::F64>; 2], high: Self::F64) -> Self::U16 {
        let [low, upper] = f64s.map(|f64s| self.round_clamped(f64s, high));
        low.narrow_u16(upper)
    }

    /// [`Self::round_clamped`] of `f64s`, in order, as bytes; `high` is at most 255.
    #[inline(always)]
    fn round_clamped_u8(self, f64s: [Halves<Self::F64>; 4], high: Self::F64) -> Self::U8 {
        self.narrow_i32_to_u8(f64s.map(|f64s| self.round_clamped(f64s, high)))
    }

    /// Thirty-two pixels of three bytes (RGBRGB…), byte `k` of each from `channels[k]`.
    #[inline(always)]
    fn store_interleaved3_u8(self, channels: [Self::U8; 3], bytes: &mut [u8; 3 * U8_LANES]) {
        let lanes = channels.map(u8_lanes);
        for (idx, byte) in bytes.iter_mut().enumerate() {
            *byte = lanes[idx % 3][idx / 3];
        }
    }

    /// Thirty-two pixels of four bytes (RGBARGBA…), byte `k` of each from `channels[k]`.
    #[inline(always)]
    fn store_interleaved4_u8(self, channels: [Self::U8; 4], bytes: &mut [u8; 4 * U8_LANES]) {
        let lanes = channels.map(u8_lanes);
        for (idx, byte) in bytes.iter_mut().enumerate() {
            *byte = lanes[idx % 4][idx / 4];
        }
    }

    /// Eight pixels of `C` samples, sample `k` of each from `channels[k]`: three or four.
    #[inline(always)]
    fn store_interleaved_f32<const C: usize, const G: usize>(
        self,
        channels: [Self::F32; C],
        samples: &mut [f32; G],
    ) {
        const { assert!((C == 3 || C == 4) && G == C * F32_LANES) };
        let lanes = channels.map(F32x8::to_array);
        for (idx, sample) in samples.iter_mut().enumerate() {
            *sample = lanes[idx % C][idx / C];
        }
    }

    /// [`Self::store_interleaved_f32`] of words: each lane of `channels` in `[0, 65535]`. A lane
    /// outside it breaks the contract, which `Portable` asserts in a debug build; the other Isas
    /// then store an unspecified word.
    #[inline(always)]
    #[expect(
        clippy::cast_sign_loss,
        reason = "each lane is in [0, 65535] by contract"
    )]
    fn store_interleaved_u16<const C: usize, const G: usize>(
        self,
        channels: [Self::I32; C],
        words: &mut [u16; G],
    ) {
        const { assert!((C == 3 || C == 4) && G == C * I32_LANES) };
        let lanes = channels.map(lanes_of);
        for (idx, word) in words.iter_mut().enumerate() {
            let lane = lanes[idx % C][idx / C];
            debug_assert!(
                (0..=65535).contains(&lane),
                "a lane of {lane} stored as a u16"
            );
            *word = lane as u16;
        }
    }

    /// The first three bytes of each of thirty-two pixels of four (RGBARGBA…), the fourth dropped.
    #[inline(always)]
    fn drop_fourth_u8(self, pixels: &[u8; 4 * U8_LANES], out: &mut [u8; 3 * U8_LANES]) {
        for (idx, byte) in out.iter_mut().enumerate() {
            *byte = pixels[4 * (idx / 3) + idx % 3];
        }
    }

    /// Thirty-two pixels of three bytes (RGBRGB…), each with `fourth` appended.
    #[inline(always)]
    fn append_fourth_u8(
        self,
        pixels: &[u8; 3 * U8_LANES],
        fourth: u8,
        out: &mut [u8; 4 * U8_LANES],
    ) {
        for (idx, byte) in out.iter_mut().enumerate() {
            *byte = if idx % 4 == 3 {
                fourth
            } else {
                pixels[3 * (idx / 4) + idx % 4]
            };
        }
    }

    /// Per pixel of thirty-two of three bytes (RGBRGB…), `Σ byteₖ · weights[k]` out of Q16: the
    /// sum over 2¹⁶, rounded to nearest, ties to even. `weights[0]` and `weights[2]` are at most
    /// 32767, `weights[1]` at most 65534, and the three sum to at most 65536, so the result is a
    /// byte: the x86 multiply-add takes signed 16-bit weights, the first and third in one pair,
    /// the second split over the other. A weight past its bound breaks the contract, which the
    /// default asserts in a debug build.
    #[inline(always)]
    fn weighted_sum3_u8(self, pixels: &[u8; 3 * U8_LANES], weights: [u16; 3]) -> Self::U8 {
        let sums: [u8; U8_LANES] = array::from_fn(|pixel| {
            weighted_sum_q16(pixels[3 * pixel..][..3].try_into().unwrap(), weights)
        });
        self.load_u8(&sums)
    }

    /// [`Self::weighted_sum3_u8`] of the first three bytes of each of thirty-two pixels of four.
    #[inline(always)]
    fn weighted_sum3_of4_u8(self, pixels: &[u8; 4 * U8_LANES], weights: [u16; 3]) -> Self::U8 {
        let sums: [u8; U8_LANES] = array::from_fn(|pixel| {
            weighted_sum_q16(pixels[4 * pixel..][..3].try_into().unwrap(), weights)
        });
        self.load_u8(&sums)
    }

    /// `table[⌊idx⌋]` per lane, with `idx` first clamped to `[0, table.len() − 1]` (a NaN to
    /// 0), so no lane reads outside the table.
    ///
    /// The table holds between 1 and 2²⁴ entries, so every index in it is an exact f32.
    #[inline(always)]
    fn lookup_f32(self, table: &[f32], idx: Self::F32) -> Self::F32 {
        let last = lookup_last(table);
        self.load_f32(&idx.to_array().map(|idx| lookup_lane(table, last, idx)))
    }

    /// The [`F32_LANES`] samples of `samples` from `start`.
    #[inline(always)]
    fn load_f32_at(self, samples: &[f32], start: usize) -> Self::F32 {
        self.load_f32(
            samples[start..]
                .first_chunk()
                .expect("a full vector of samples from `start`"),
        )
    }

    /// Up to [`F32_LANES`] samples into the low lanes, zero in the rest; a full vector as one load.
    #[inline(always)]
    fn load_f32_partial(self, samples: &[f32]) -> Self::F32 {
        if let Ok(full) = <&[f32; F32_LANES]>::try_from(samples) {
            return self.load_f32(full);
        }
        let mut lanes = [0.0; F32_LANES];
        lanes[..samples.len()].copy_from_slice(samples);
        self.load_f32(&lanes)
    }

    /// Up to [`F64_LANES`] samples into the low lanes, zero in the rest.
    #[inline(always)]
    fn load_f64_partial(self, samples: &[f64]) -> Self::F64 {
        let mut lanes = [0.0; F64_LANES];
        lanes[..samples.len()].copy_from_slice(samples);
        self.load_f64(&lanes)
    }
}

/// The comparison result of an Isa's f32 vectors, which also picks the lanes a gather reads.
pub type Mask<S> = <<S as Isa>::F32 as F32x8>::Mask;

/// The highest index of a table [`Isa::lookup_f32`] may read, as the exact f32 it clamps to.
///
/// A release assert: past 2²⁴ entries the f32 nearest an index may lie past the table's end.
#[inline(always)]
pub(crate) fn lookup_last(table: &[f32]) -> f32 {
    assert!(
        (1..=1 << 24).contains(&table.len()),
        "a lookup table holds 1 to 2^24 entries, not {}",
        table.len()
    );
    (table.len() - 1) as f32
}

/// One lane of [`Isa::lookup_f32`]: the clamp in the order a gather takes it, `min` then `max`, so
/// a NaN index reads entry 0.
#[inline(always)]
#[expect(
    clippy::cast_sign_loss,
    reason = "the index is clamped to `[0, last]` first"
)]
fn lookup_lane(table: &[f32], last: f32, idx: f32) -> f32 {
    let below_end = if idx > last { last } else { idx };
    let clamped = if below_end > 0.0 { below_end } else { 0.0 };
    table[clamped as usize]
}

/// [`Isa::gather_u32_le`] lane by lane: the lanes `bits` sets read their up to four bytes.
#[inline(always)]
pub(crate) fn u32_le_lanes(bytes: &[u8], offsets: [i32; I32_LANES], bits: u8) -> [u32; I32_LANES] {
    array::from_fn(|lane| {
        let mut word = [0; 4];
        if let Some(from) = usize::try_from(offsets[lane])
            .ok()
            .filter(|_| (bits >> lane) & 1 == 1)
            .and_then(|offset| bytes.get(offset..))
        {
            let len = from.len().min(4);
            word[..len].copy_from_slice(&from[..len]);
        }
        u32::from_le_bytes(word)
    })
}

/// [`Isa::gather_pair_f32`] lane by lane: sample `k` of a lane is read where `bits[0]`, for
/// the first `SPLIT`, or `bits[1]`, for the rest, sets the lane.
#[inline(always)]
pub(crate) fn pair_lanes<T: Copy + Default, const SPLIT: usize>(
    table: &[T],
    starts: [i32; I32_LANES],
    bits: [u8; 2],
) -> [[T; I32_LANES]; 8] {
    const { assert!(SPLIT <= 8) };
    array::from_fn(|k| {
        let bits = bits[usize::from(k >= SPLIT)];
        array::from_fn(|lane| {
            if (bits >> lane) & 1 == 0 {
                return T::default();
            }
            usize::try_from(i64::from(starts[lane]) + k as i64)
                .ok()
                .and_then(|idx| table.get(idx))
                .copied()
                .unwrap_or_default()
        })
    })
}

/// The lanes of `ints`, as an array.
#[inline(always)]
pub(crate) fn lanes_of<V: I32x8>(ints: V) -> [i32; I32_LANES] {
    let mut lanes = [0; I32_LANES];
    ints.store(&mut lanes);
    lanes
}

/// The lanes of `vector`, as an array.
#[inline(always)]
fn u8_lanes<V: U8x32>(vector: V) -> [u8; U8_LANES] {
    let mut lanes = [0; U8_LANES];
    vector.store(&mut lanes);
    lanes
}

/// [`Isa::weighted_sum3_u8`] of one pixel: the sum, then `(sum + 0x7FFF + odd) >> 16` with `odd`
/// the quotient's lowest bit, which is nearest with ties to even.
#[inline(always)]
fn weighted_sum_q16(bytes: [u8; 3], weights: [u16; 3]) -> u8 {
    let [w0, w1, w2] = weights.map(u32::from);
    debug_assert!(
        w0 <= 32767 && w2 <= 32767 && w1 <= 65534 && w0 + w1 + w2 <= 1 << 16,
        "weights {weights:?} past the weighted sum's bounds"
    );
    let sum = u32::from(bytes[0]) * w0 + u32::from(bytes[1]) * w1 + u32::from(bytes[2]) * w2;
    let odd = (sum >> 16) & 1;
    ((sum + 0x7FFF + odd) >> 16) as u8
}
