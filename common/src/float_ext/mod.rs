//! Approximate float comparison at the scale of the values compared.

/// Slack between two values before they differ, in ULPs at the larger magnitude: eight covers the
/// half ULP each step of a dozen-odd operations can round away, and stays far short of a real
/// difference.
const TOLERANCE_ULPS: u8 = 8;

/// Same-value comparison for floats, tolerant of the noise arithmetic leaves
/// behind.
///
/// The tolerance is relative, as rounding error is proportional to the operands. A fixed absolute
/// one is wrong at both ends: near `1e9` an `f32` cannot hold a difference of `1e-6`, so it
/// collapses into `==`, while near `1e-9` it swallows a thousandfold difference.
///
/// This answers "did this value change", not "is this near zero": zero equals only zero, as a
/// value falling out of the same computation twice has the same bits. A cancelled difference needs
/// an absolute bound in the caller's units, `(a - b).abs() < HALF_PIXEL`.
pub trait FloatExt: Copy {
    /// Whether the two are the same value within the tolerance. NaN matches nothing, an infinity
    /// only itself.
    fn approximately_eq(self, other: Self) -> bool;
}

/// Generated per width so each carries its own machine epsilon: `f32`'s on `f64` values is nine
/// orders of magnitude too loose.
macro_rules! impl_float_ext {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl FloatExt for $ty {
                fn approximately_eq(self, other: Self) -> bool {
                    // Only this branch can call two infinities equal: `INF - INF` is NaN.
                    if self == other {
                        return true;
                    }
                    // An infinite magnitude makes the tolerance infinite, and `INF <= INF` would
                    // equal an infinity to every finite value; `max` drops a lone NaN.
                    let magnitude = self.abs().max(other.abs());
                    if !magnitude.is_finite() {
                        return false;
                    }
                    // `EPSILON` times the magnitude is within a factor of two of the true ULP. A
                    // lone NaN leaves the difference NaN, which fails `<=`.
                    (self - other).abs()
                        <= <$ty>::from(TOLERANCE_ULPS) * <$ty>::EPSILON * magnitude
                }
            }
        )+
    };
}

impl_float_ext!(f32, f64);

/// Component-wise, each axis at its own magnitude: a pan of `(4096.0, 0.5)` has noise in `x` at a
/// scale `y` never reaches.
impl FloatExt for glam::Vec2 {
    fn approximately_eq(self, other: Self) -> bool {
        self.x.approximately_eq(other.x) && self.y.approximately_eq(other.y)
    }
}

#[cfg(test)]
mod tests;
