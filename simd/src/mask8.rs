//! [`Mask8`]: the per-lane predicate of an [`F32x8`](crate::f32x8::F32x8) comparison.

use std::fmt::Debug;

/// A per-lane predicate from one of [`F32x8`](crate::f32x8::F32x8)'s comparisons.
pub trait Mask8<V>: Copy + Debug {
    /// `if_true` where the lane is set, `if_false` elsewhere.
    fn select(self, if_true: V, if_false: V) -> V;

    /// `value` where the lane is set, +0 elsewhere: `select(value, 0.0)` as one bitwise AND.
    fn keep(self, value: V) -> V;

    /// Set where both `self` and `other` are.
    #[must_use]
    fn and(self, other: Self) -> Self;

    /// Set where `self` or `other` is.
    #[must_use]
    fn or(self, other: Self) -> Self;

    /// Lane `i` at bit `i`.
    fn to_bitmask(self) -> u8;
}
