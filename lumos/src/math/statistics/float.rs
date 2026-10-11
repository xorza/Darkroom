//! The float widths [`crate::math::statistics`]' order statistics run at.

use std::ops::{Add, Mul, Sub};

/// What a median or a MAD needs of the type it is measuring.
///
/// One bound rather than a `_f32`/`_f64` pair per operation. Every body in
/// this module is the same algorithm at two precisions, and monomorphization
/// gives each width exactly the code the hand-written pair compiled to. Both
/// widths are needed: the polynomial surface fit in `background_extraction`
/// works in `f64` throughout, so downcasting its residuals to run a
/// single-precision path would lose precision inside a fitting loop.
pub(crate) trait Float:
    Copy + PartialOrd + Add<Output = Self> + Sub<Output = Self> + Mul<Output = Self>
{
    /// The signed integer of the same width, which [`Self::into_keys`] maps a value to.
    type Key: Ord + Copy + Default;

    /// What an empty input measures to.
    const ZERO: Self;
    /// Averages the two middle elements of an even-length slice.
    const HALF: Self;

    fn abs(self) -> Self;

    /// `values` mapped in place to keys whose integer order is `total_cmp`'s: a negative
    /// value's bits below the sign flip. A selection on the keys compares integers, where one under
    /// `total_cmp` maps both operands of every comparison. The map is its own inverse, so
    /// [`Self::from_keys`] restores the values.
    fn into_keys(values: &mut [Self]) -> &mut [Self::Key];

    /// The values of keys [`Self::into_keys`] made, mapped back in place.
    fn from_keys(keys: &mut [Self::Key]) -> &mut [Self];

    /// The value one key stands for.
    fn from_key(key: Self::Key) -> Self;

    /// `magnitudes`, none of them with its sign bit set, read as their keys: such a value's bits
    /// are its key already, so nothing is mapped.
    fn magnitude_keys(magnitudes: &mut [Self]) -> &mut [Self::Key];
}

/// The bits of an `f32` whose `i32` order is [`f32::total_cmp`]'s, as `std` computes them, and
/// back: the flip keeps the sign bit, so a second one undoes the first.
const fn total_order_32(bits: i32) -> i32 {
    bits ^ ((bits >> 31).cast_unsigned() >> 1).cast_signed()
}

/// [`total_order_32`] for an `f64`.
const fn total_order_64(bits: i64) -> i64 {
    bits ^ ((bits >> 63).cast_unsigned() >> 1).cast_signed()
}

impl Float for f32 {
    type Key = i32;
    const ZERO: Self = 0.0;
    const HALF: Self = 0.5;

    #[inline]
    fn abs(self) -> Self {
        f32::abs(self)
    }

    #[inline]
    fn into_keys(values: &mut [Self]) -> &mut [i32] {
        let keys: &mut [i32] = bytemuck::cast_slice_mut(values);
        for key in keys.iter_mut() {
            *key = total_order_32(*key);
        }
        keys
    }

    #[inline]
    fn from_keys(keys: &mut [i32]) -> &mut [Self] {
        for key in keys.iter_mut() {
            *key = total_order_32(*key);
        }
        bytemuck::cast_slice_mut(keys)
    }

    #[inline]
    fn from_key(key: i32) -> Self {
        f32::from_bits(total_order_32(key).cast_unsigned())
    }

    #[inline]
    fn magnitude_keys(magnitudes: &mut [Self]) -> &mut [i32] {
        bytemuck::cast_slice_mut(magnitudes)
    }
}

impl Float for f64 {
    type Key = i64;
    const ZERO: Self = 0.0;
    const HALF: Self = 0.5;

    #[inline]
    fn abs(self) -> Self {
        f64::abs(self)
    }

    #[inline]
    fn into_keys(values: &mut [Self]) -> &mut [i64] {
        let keys: &mut [i64] = bytemuck::cast_slice_mut(values);
        for key in keys.iter_mut() {
            *key = total_order_64(*key);
        }
        keys
    }

    #[inline]
    fn from_keys(keys: &mut [i64]) -> &mut [Self] {
        for key in keys.iter_mut() {
            *key = total_order_64(*key);
        }
        bytemuck::cast_slice_mut(keys)
    }

    #[inline]
    fn from_key(key: i64) -> Self {
        f64::from_bits(total_order_64(key).cast_unsigned())
    }

    #[inline]
    fn magnitude_keys(magnitudes: &mut [Self]) -> &mut [i64] {
        bytemuck::cast_slice_mut(magnitudes)
    }
}
