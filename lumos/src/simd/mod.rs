//! Vector kernels written once, for every CPU.
//!
//! A kernel implements [`Kernel`]. Its body is generic over an [`Isa`] — a zero-sized token whose
//! existence proves the CPU has the instructions — and [`Kernel::dispatch`] runs it on the widest
//! [`Tier`] the CPU has: `Avx2Fma` on `x86_64` (lane loops on one older than Haswell), `Neon` on
//! aarch64, and `Portable` lane loops on any other CPU.
//!
//! Every Isa has the same logical width, [`F32_LANES`] f32 lanes and [`F64_LANES`] f64 lanes,
//! whatever its registers hold: AVX2 keeps a vector in one register, NEON in two. The lane split,
//! the tails, the reduction order and every crossover are therefore one choice rather than one per
//! CPU, and every op is specified to the bit (see [`F32x8`] and [`F64x4`]). A kernel built from
//! these ops alone computes the same values on every Isa — the same bits, save a NaN's sign and
//! payload, which IEEE 754 leaves to the hardware. The tests hold each hardware Isa to `Portable`
//! op by op.
//!
//! Kernels hold no `unsafe`. Loads take a `&[f32; 8]` from `as_chunks` or a checked slice, and the
//! one `unsafe` call — into the `#[target_feature]` function [`Isa::run`] enters — rests on the
//! token alone.
//!
//! Everything [`Kernel::run`] reaches is `#[inline(always)]`, and no closure in it calls a vector
//! op. A function that is not compiled into the `#[target_feature]` entry runs without those
//! features, so each intrinsic in it becomes a call: the result stays right and the speed falls by
//! an order of magnitude, which no test notices. `lumos/AGENTS.md` has the assembly check that
//! does.

#[cfg(target_arch = "x86_64")]
pub(crate) mod avx2_fma;
pub(crate) mod math;
#[cfg(target_arch = "aarch64")]
pub(crate) mod neon;
#[cfg(any(test, not(target_arch = "aarch64")))]
pub(crate) mod portable;
pub(crate) mod tier;

use std::fmt::Debug;
use std::ops::{Add, Div, Mul, Sub};

use crate::simd::tier::Tier;

/// f32 lanes in one [`F32x8`], on every Isa.
pub(crate) const F32_LANES: usize = 8;

/// f64 lanes in one [`F64x4`], on every Isa: one f32 vector widens into two of these.
pub(crate) const F64_LANES: usize = 4;

/// A vector kernel: one body, generic over the Isa it runs on.
pub(crate) trait Kernel: Sized {
    type Output;

    /// The kernel on `isa`. Every implementation is `#[inline(always)]`, and so is everything it
    /// calls (see the module documentation).
    fn run<S: Isa>(self, isa: S) -> Self::Output;

    /// The kernel on the widest Isa this CPU has.
    #[inline]
    fn dispatch(self) -> Self::Output {
        Tier::widest().run(self)
    }
}

/// An instruction set, as a zero-sized token that only exists on a CPU that has it.
///
/// Its methods make vectors; the vectors' own methods ([`F32x8`], [`F64x4`]) combine them.
pub(crate) trait Isa: Copy + Debug {
    type F32: F32x8<F64 = Self::F64>;
    type F64: F64x4;

    /// `kernel` compiled for this Isa's features.
    fn run<K: Kernel>(self, kernel: K) -> K::Output;

    fn splat_f32(self, value: f32) -> Self::F32;

    fn splat_f64(self, value: f64) -> Self::F64;

    fn load_f32(self, lanes: &[f32; F32_LANES]) -> Self::F32;

    fn load_f64(self, lanes: &[f64; F64_LANES]) -> Self::F64;

    /// `table[⌊index⌋]` per lane, with `index` first clamped to `[0, table.len() − 1]` (a NaN to
    /// 0), so no lane reads outside the table.
    ///
    /// The table holds between 1 and 2²⁴ entries, so every index in it is an exact f32.
    fn lookup_f32(self, table: &[f32], index: Self::F32) -> Self::F32;

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

/// Eight f32 lanes. Every op is defined per lane to the bit, so every Isa computes the same value:
///
/// - `+ − × ÷` and [`F32x8::sqrt`] are IEEE 754, correctly rounded; [`F32x8::mul_add`] rounds
///   once, on every Isa.
/// - [`F32x8::max`] and [`F32x8::min`] are the two halves of a compare-swap — `a > b ? a : b` and
///   `a > b ? b : a` — so a NaN, or a zero of either sign, decides the same way on every Isa and
///   in a scalar network written with `if a > b { swap }`. `x.max(floor)` sends a NaN `x` to
///   `floor`.
/// - Comparisons are ordered: false where either lane is NaN.
pub(crate) trait F32x8:
    Copy + Debug + Add<Output = Self> + Sub<Output = Self> + Mul<Output = Self> + Div<Output = Self>
{
    type F64: F64x4;
    type Mask: Mask8<Self>;

    /// `self · mul + add`, rounded once.
    fn mul_add(self, mul: Self, add: Self) -> Self;

    /// `self > other ? self : other`.
    fn max(self, other: Self) -> Self;

    /// `self > other ? other : self`.
    fn min(self, other: Self) -> Self;

    fn sqrt(self) -> Self;

    fn lanes_gt(self, other: Self) -> Self::Mask;

    fn lanes_lt(self, other: Self) -> Self::Mask;

    fn lanes_eq(self, other: Self) -> Self::Mask;

    /// `((l0 + l4) + (l1 + l5)) + ((l2 + l6) + (l3 + l7))`: the fold every Isa takes.
    fn reduce_sum(self) -> f32;

    /// Lanes 0–3 and lanes 4–7, each converted exactly to f64.
    fn widen(self) -> Halves<Self::F64>;

    /// `self = mantissa · 2^exponent`, mantissa in `[0.5, 1)`, for lanes that hold a positive
    /// normal number. Every other lane follows the same bit formula — the exponent field less 126,
    /// and the bits with that field set to 126 — so it is garbage, but the same garbage on every
    /// Isa.
    fn frexp(self) -> Frexp<Self>;

    fn store(self, lanes: &mut [f32; F32_LANES]);

    /// The low `lanes.len()` lanes, at most [`F32_LANES`]; all of them as one store.
    #[inline(always)]
    fn store_partial(self, lanes: &mut [f32]) {
        if let Ok(full) = <&mut [f32; F32_LANES]>::try_from(&mut *lanes) {
            self.store(full);
            return;
        }
        let all = self.to_array();
        lanes.copy_from_slice(&all[..lanes.len()]);
    }

    #[inline(always)]
    fn to_array(self) -> [f32; F32_LANES] {
        let mut lanes = [0.0; F32_LANES];
        self.store(&mut lanes);
        lanes
    }
}

/// Four f64 lanes, specified as [`F32x8`] is.
pub(crate) trait F64x4:
    Copy + Debug + Add<Output = Self> + Sub<Output = Self> + Mul<Output = Self> + Div<Output = Self>
{
    /// `self · mul + add`, rounded once.
    fn mul_add(self, mul: Self, add: Self) -> Self;

    /// `self > other ? self : other`.
    fn max(self, other: Self) -> Self;

    /// `self > other ? other : self`.
    fn min(self, other: Self) -> Self;

    fn sqrt(self) -> Self;

    fn floor(self) -> Self;

    /// `2^self` for lanes that hold an integer in `[−1022, 1023]`; other lanes are garbage.
    fn pow2i(self) -> Self;

    /// `(l0 + l1) + (l2 + l3)`: the fold every Isa takes.
    fn reduce_sum(self) -> f64;

    fn to_array(self) -> [f64; F64_LANES];
}

/// A per-lane predicate from one of [`F32x8`]'s comparisons.
pub(crate) trait Mask8<V>: Copy + Debug {
    /// `if_true` where the lane is set, `if_false` elsewhere.
    fn select(self, if_true: V, if_false: V) -> V;

    /// `value` where the lane is set, +0 elsewhere: `select(value, 0.0)` as one bitwise AND.
    fn keep(self, value: V) -> V;

    /// Lane `i` at bit `i`.
    fn to_bitmask(self) -> u8;
}

/// The highest index of a table [`Isa::lookup_f32`] may read, as the exact f32 it clamps to.
///
/// A release assert: past 2²⁴ entries the f32 nearest an index may lie past the table's end.
#[inline(always)]
fn lookup_last(table: &[f32]) -> f32 {
    assert!(
        (1..=1 << 24).contains(&table.len()),
        "a lookup table holds 1 to 2^24 entries, not {}",
        table.len()
    );
    (table.len() - 1) as f32
}

/// One lane of [`Isa::lookup_f32`], for an Isa that has no gather: the clamp in the order the
/// vector one takes it, `min` then `max`, so a NaN index reads entry 0.
#[inline(always)]
#[expect(
    clippy::cast_sign_loss,
    reason = "the index is clamped to `[0, last]` first"
)]
fn lookup_lane(table: &[f32], last: f32, index: f32) -> f32 {
    let below_end = if index > last { last } else { index };
    let clamped = if below_end > 0.0 { below_end } else { 0.0 };
    table[clamped as usize]
}

/// One [`F32x8`] widened: lanes 0–3 and lanes 4–7.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Halves<V> {
    pub(crate) low: V,
    pub(crate) high: V,
}

/// [`F32x8::frexp`]'s two parts, the exponent as an f32.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Frexp<V> {
    pub(crate) mantissa: V,
    pub(crate) exponent: V,
}

#[cfg(test)]
mod tests;
