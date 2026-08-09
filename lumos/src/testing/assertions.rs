//! One vocabulary for approximate float comparison in tests.
//!
//! Replaces four hand-rolled `approx_eq` helpers that disagreed on both signature and semantics:
//! two took a fixed tolerance, one took it as an argument, and only one handled near-zero values.
//!
//! The comparison here accepts either an absolute or a relative miss. A plain `|a - b| < tol`
//! fails for large magnitudes, where the tolerance sits below the values' own f64 resolution
//! — `1e-10` against values near `1e6` is already at the edge of what f64 can represent. A plain
//! relative test divides by zero when both sides are. Taking whichever passes keeps both ends
//! usable with one tolerance number.

/// Whether `a` and `b` agree to `tol`, absolutely or relatively.
///
/// Exact equality short-circuits, which is what makes `±inf` compare equal to itself — the
/// difference below would be `NaN` there, and every comparison against `NaN` is false. A `NaN`
/// operand is deliberately never close to anything, including another `NaN`.
pub(crate) fn is_close(a: f64, b: f64, tol: f64) -> bool {
    if a == b {
        return true;
    }
    let diff = (a - b).abs();
    // A non-finite gap is never close, and has to be rejected before the relative branch: that
    // one compares `inf <= tol * inf`, which is true, so opposite infinities would slip through.
    if !diff.is_finite() {
        return false;
    }
    diff <= tol || diff <= tol * a.abs().max(b.abs())
}

/// Assert two floats agree to `tol`, absolutely or relatively. Takes `f32` or `f64` on either
/// side. An optional trailing `format!` message is appended to the default one.
macro_rules! assert_close {
    ($a:expr, $b:expr, $tol:expr $(,)?) => {{
        let (a, b, tol) = (f64::from($a), f64::from($b), f64::from($tol));
        let diff = (a - b).abs();
        assert!(
            $crate::testing::assertions::is_close(a, b, tol),
            "{a} !~ {b} (tol {tol:e}, diff {diff:e})"
        );
    }};
    ($a:expr, $b:expr, $tol:expr, $($arg:tt)+) => {{
        let (a, b, tol) = (f64::from($a), f64::from($b), f64::from($tol));
        let diff = (a - b).abs();
        assert!(
            $crate::testing::assertions::is_close(a, b, tol),
            "{a} !~ {b} (tol {tol:e}, diff {diff:e}): {}",
            format_args!($($arg)+)
        );
    }};
}
pub(crate) use assert_close;

/// Assert two float sequences agree elementwise to `tol`, naming the first index that does not.
/// An optional trailing `format!` message labels which sequence it was, for tests that compare
/// several in a row.
macro_rules! assert_close_slice {
    ($a:expr, $b:expr, $tol:expr $(,)?) => {
        $crate::testing::assertions::assert_close_slice!($a, $b, $tol, "sequence")
    };
    ($a:expr, $b:expr, $tol:expr, $($arg:tt)+) => {{
        let (a, b, tol) = (&$a[..], &$b[..], f64::from($tol));
        let what = format!($($arg)+);
        assert_eq!(a.len(), b.len(), "{what}: lengths differ");
        for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
            let (x, y) = (f64::from(*x), f64::from(*y));
            let diff = (x - y).abs();
            assert!(
                $crate::testing::assertions::is_close(x, y, tol),
                "{what}[{i}]: {x} !~ {y} (tol {tol:e}, diff {diff:e})"
            );
        }
    }};
}
pub(crate) use assert_close_slice;

#[cfg(test)]
mod tests {
    use crate::testing::assertions::is_close;

    #[test]
    fn absolute_and_relative_are_both_accepted() {
        // Absolute: the relative miss is 1.0, far outside tol, but the absolute one is at it.
        assert!(is_close(0.0, 1e-10, 1e-10));
        assert!(!is_close(0.0, 1.1e-10, 1e-10));

        // Relative: 1e6 vs 1e6+1e-5 misses absolutely by 1e-5 but relatively by 1e-11.
        assert!(is_close(1e6, 1e6 + 1e-5, 1e-10));
        assert!(!is_close(1e6, 1e6 + 1e-3, 1e-10));

        // The absolute-only form the four old helpers used would reject that first relative case,
        // and it is the one f64 cannot actually resolve: 1e6's own ulp is ~1.2e-10.
        assert!((1e6f64 - (1e6 + 1e-5)).abs() > 1e-10);
    }

    #[test]
    fn exact_equality_and_non_finite_values() {
        assert!(is_close(f64::INFINITY, f64::INFINITY, 0.0));
        assert!(is_close(f64::NEG_INFINITY, f64::NEG_INFINITY, 0.0));
        assert!(!is_close(f64::INFINITY, f64::NEG_INFINITY, 1e300));
        // NaN is close to nothing, itself included — an assertion on it must fire.
        assert!(!is_close(f64::NAN, f64::NAN, 1.0));
        assert!(!is_close(f64::NAN, 0.0, 1.0));
        // Zero tolerance still admits exact equality.
        assert!(is_close(2.5, 2.5, 0.0));
        assert!(!is_close(2.5, 2.5000001, 0.0));
    }

    #[test]
    fn macros_accept_f32_and_f64() {
        assert_close!(1.0f32, 1.0f32 + f32::EPSILON, 1e-6);
        assert_close!(1.0f64, 1.0f64, 0.0);
        assert_close!(0.1f32 + 0.2f32, 0.3f32, 1e-6, "context {}", 7);
        assert_close_slice!([1.0f32, 2.0, 3.0], [1.0f32, 2.0, 3.0 + 1e-8], 1e-6);
        assert_close_slice!(vec![1.0f64, 2.0], vec![1.0f64, 2.0], 0.0);
    }
}
