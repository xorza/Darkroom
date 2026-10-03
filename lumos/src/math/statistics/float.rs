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

impl Float for f32 {
    const ZERO: Self = 0.0;
    const HALF: Self = 0.5;

    #[inline]
    fn total_cmp(&self, other: &Self) -> Ordering {
        f32::total_cmp(self, other)
    }

    #[inline]
    fn fast_cmp(&self, other: &Self) -> Ordering {
        self.partial_cmp(other).unwrap_or(Ordering::Equal)
    }

    #[inline]
    fn abs(self) -> Self {
        f32::abs(self)
    }

    #[inline]
    fn max(self, other: Self) -> Self {
        f32::max(self, other)
    }

    #[inline]
    fn is_nan(self) -> bool {
        f32::is_nan(self)
    }
}

impl Float for f64 {
    const ZERO: Self = 0.0;
    const HALF: Self = 0.5;

    #[inline]
    fn total_cmp(&self, other: &Self) -> Ordering {
        f64::total_cmp(self, other)
    }

    #[inline]
    fn fast_cmp(&self, other: &Self) -> Ordering {
        self.partial_cmp(other).unwrap_or(Ordering::Equal)
    }

    #[inline]
    fn abs(self) -> Self {
        f64::abs(self)
    }

    #[inline]
    fn max(self, other: Self) -> Self {
        f64::max(self, other)
    }

    #[inline]
    fn is_nan(self) -> bool {
        f64::is_nan(self)
    }
}
