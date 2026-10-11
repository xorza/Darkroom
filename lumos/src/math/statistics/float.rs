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
    /// The signed integer of the same width: the bits of a value with its sign bit clear order
    /// as `total_cmp` orders the values, and [`Self::map_keys`] makes them so for any value.
    type Key: Ord + Copy + Default;

    /// What an empty input measures to.
    const ZERO: Self;
    /// Averages the two middle elements of an even-length slice.
    const HALF: Self;

    fn abs(self) -> Self;

    /// Whether any of `values` has its sign bit set, by one OR over their bits.
    fn any_sign_bit(values: &[Self]) -> bool;

    /// `values` read in place as their bits.
    fn as_keys(values: &mut [Self]) -> &mut [Self::Key];

    /// `keys` read in place as the values whose bits they are.
    fn as_values(keys: &mut [Self::Key]) -> &mut [Self];

    /// Flip the bits below the sign of every key whose sign bit is set: what puts a negative
    /// value's bits in `total_cmp`'s order, and, being its own inverse, takes them back out.
    fn map_keys(keys: &mut [Self::Key]);

    /// The value a key in `total_cmp`'s order stands for.
    fn from_key(key: Self::Key) -> Self;
}

/// Values read in place as keys in `total_cmp`'s order. A selection on the keys compares integers,
/// where one under `total_cmp` maps both operands of every comparison. Values with no sign bit
/// set, as sky levels and deviations are, are their own keys and are not written; any other slice
/// is mapped once in and once out.
#[derive(Debug)]
pub(crate) struct OrderKeys<'a, F: Float> {
    keys: &'a mut [F::Key],
    mapped: bool,
}

impl<'a, F: Float> OrderKeys<'a, F> {
    pub(crate) fn of(values: &'a mut [F]) -> Self {
        let mapped = F::any_sign_bit(values);
        let keys = F::as_keys(values);
        if mapped {
            F::map_keys(keys);
        }
        Self { keys, mapped }
    }

    /// Whether the values were mapped: when not, the keys are the values' own bits.
    pub(crate) const fn mapped(&self) -> bool {
        self.mapped
    }

    pub(crate) const fn keys(&mut self) -> &mut [F::Key] {
        self.keys
    }

    /// The values again, in the order the keys were left in.
    pub(crate) fn into_values(self) -> &'a mut [F] {
        if self.mapped {
            F::map_keys(self.keys);
        }
        F::as_values(self.keys)
    }
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
    fn any_sign_bit(values: &[Self]) -> bool {
        values.iter().fold(0, |bits, value| bits | value.to_bits()) >> 31 != 0
    }

    #[inline]
    fn as_keys(values: &mut [Self]) -> &mut [i32] {
        bytemuck::cast_slice_mut(values)
    }

    #[inline]
    fn as_values(keys: &mut [i32]) -> &mut [Self] {
        bytemuck::cast_slice_mut(keys)
    }

    #[inline]
    fn map_keys(keys: &mut [i32]) {
        for key in keys {
            *key = total_order_32(*key);
        }
    }

    #[inline]
    fn from_key(key: i32) -> Self {
        f32::from_bits(total_order_32(key).cast_unsigned())
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
    fn any_sign_bit(values: &[Self]) -> bool {
        values.iter().fold(0, |bits, value| bits | value.to_bits()) >> 63 != 0
    }

    #[inline]
    fn as_keys(values: &mut [Self]) -> &mut [i64] {
        bytemuck::cast_slice_mut(values)
    }

    #[inline]
    fn as_values(keys: &mut [i64]) -> &mut [Self] {
        bytemuck::cast_slice_mut(keys)
    }

    #[inline]
    fn map_keys(keys: &mut [i64]) {
        for key in keys {
            *key = total_order_64(*key);
        }
    }

    #[inline]
    fn from_key(key: i64) -> Self {
        f64::from_bits(total_order_64(key).cast_unsigned())
    }
}
