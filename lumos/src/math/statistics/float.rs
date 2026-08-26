//! The float widths [`crate::math::statistics`]' order statistics run at.

use std::cmp::Ordering;
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
    /// What an empty input measures to.
    const ZERO: Self;
    /// Averages the two middle elements of an even-length slice.
    const HALF: Self;

    /// A total order, so a slice holding NaN still selects a meaningful rank.
    /// Shaped like `std`'s own so it passes straight to `select_nth_unstable_by`.
    fn total_cmp(&self, other: &Self) -> Ordering;

    /// `partial_cmp` with NaN folded to `Equal` — an order at all only on NaN-free data.
    ///
    /// Cheaper than [`Self::total_cmp`], which first maps both operands' bit patterns onto a
    /// total order. Measured at roughly 15% off a 4096-sample median.
    fn fast_cmp(&self, other: &Self) -> Ordering;

    fn abs(self) -> Self;

    /// The larger of the two, dropping a lone NaN — `std`'s own `max`.
    fn max(self, other: Self) -> Self;

    fn is_nan(self) -> bool;
}

/// One impl per width, generated rather than copied. The pair this trait
/// replaced had drifted into two doc comments explaining they were twins.
macro_rules! impl_float {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl Float for $ty {
                const ZERO: Self = 0.0;
                const HALF: Self = 0.5;

                #[inline]
                fn total_cmp(&self, other: &Self) -> Ordering {
                    <$ty>::total_cmp(self, other)
                }

                #[inline]
                fn fast_cmp(&self, other: &Self) -> Ordering {
                    self.partial_cmp(other).unwrap_or(Ordering::Equal)
                }

                #[inline]
                fn abs(self) -> Self {
                    <$ty>::abs(self)
                }

                #[inline]
                fn max(self, other: Self) -> Self {
                    <$ty>::max(self, other)
                }

                #[inline]
                fn is_nan(self) -> bool {
                    <$ty>::is_nan(self)
                }
            }
        )+
    };
}

impl_float!(f32, f64);
