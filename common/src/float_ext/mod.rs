//! Approximate float comparison at the scale of the values compared.

/// Slack allowed between two values before they count as different, in units
/// in the last place at the magnitude being compared. Eight covers the half a
/// ULP each arithmetic step can round away over a chain of a dozen-odd
/// operations, and stops far short of any difference a reader would call real.
const TOLERANCE_ULPS: u8 = 8;

/// Same-value comparison for floats, tolerant of the noise arithmetic leaves
/// behind.
///
/// The tolerance is *relative*: two values agree when they differ by no more
/// than `TOLERANCE_ULPS` units in the last place of the larger magnitude,
/// which is the shape float noise actually has — rounding error is
/// proportional to the operands. A fixed absolute tolerance is wrong at both
/// ends of the range: near `1e9` an `f32` cannot represent a difference of
/// `1e-6` at all, so the test collapses into `==`, while near `1e-9` that same
/// tolerance swallows a thousandfold difference.
///
/// This answers "did this value change", not "is this near zero". Zero is
/// approximately equal to nothing but zero, because a value that falls out of
/// the same computation twice falls out with the same bits. Asking whether a
/// difference has cancelled needs an absolute bound in the caller's own units
/// — `(a - b).abs() < HALF_PIXEL` — which only the caller can name.
pub trait FloatExt: Copy {
    /// Whether the two are the same value once the tolerance above is
    /// allowed for. NaN matches nothing, an infinity matches only itself.
    fn approximately_eq(self, other: Self) -> bool;
}

/// One impl per float width, generated rather than copied so each provably
/// carries its own machine epsilon: the `f32` constant applied to `f64` values
/// is a tolerance nine orders of magnitude looser than that type's noise floor.
macro_rules! impl_float_ext {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl FloatExt for $ty {
                fn approximately_eq(self, other: Self) -> bool {
                    // Exact first — the only branch that can call two
                    // infinities equal, since `INF - INF` is NaN and fails
                    // every tolerance test.
                    if self == other {
                        return true;
                    }
                    // `max` drops a single NaN, so this is finite whenever one
                    // operand is; only two NaNs or an infinity land here. Both
                    // must fail: an infinite magnitude makes the tolerance
                    // infinite too, and `INF <= INF` would call an infinity
                    // equal to every finite value.
                    let magnitude = self.abs().max(other.abs());
                    if !magnitude.is_finite() {
                        return false;
                    }
                    // `EPSILON` is one ULP at 1.0, so scaling it by the
                    // magnitude lands within a factor of two of the true ULP
                    // there. A lone NaN operand leaves the difference NaN,
                    // which fails `<=` and stays unequal to everything.
                    (self - other).abs()
                        <= <$ty>::from(TOLERANCE_ULPS) * <$ty>::EPSILON * magnitude
                }
            }
        )+
    };
}

impl_float_ext!(f32, f64);

/// Component-wise, each axis judged at its own magnitude. A canvas pan of
/// `(4096.0, 0.5)` carries noise in `x` at a scale `y` never reaches, and one
/// shared tolerance would have to be wrong for one of the two.
impl FloatExt for glam::Vec2 {
    fn approximately_eq(self, other: Self) -> bool {
        self.x.approximately_eq(other.x) && self.y.approximately_eq(other.y)
    }
}

#[cfg(test)]
mod tests;
