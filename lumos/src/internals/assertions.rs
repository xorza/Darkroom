//! One vocabulary for approximate float comparison in tests: an absolute bound, which the call
//! states. A bound relative to the expected value is that value times the relative bound, written
//! at the call — one meaning per number, so a tolerance cannot quietly loosen as values grow.

/// Whether `a` and `b` agree to the absolute bound `tol`.
///
/// Exact equality short-circuits, which is what makes `±inf` compare equal to itself — the
/// difference below would be `NaN` there, and every comparison against `NaN` is false. A `NaN`
/// operand is deliberately never close to anything, including another `NaN`.
pub(crate) fn is_close(a: f64, b: f64, tol: f64) -> bool {
    a == b || (a - b).abs() <= tol
}

/// The bit patterns of `values`, for comparing two float sequences exactly: `-0.0` and `0.0`
/// differ and NaN equals itself, as no tolerance-based comparison allows.
pub(crate) fn bits(values: &[f32]) -> Vec<u32> {
    values.iter().map(|value| value.to_bits()).collect()
}

/// Assert two floats agree to the absolute bound `tol`. Takes `f32` or `f64` on either
/// side. An optional trailing `format!` message is appended to the default one.
macro_rules! assert_close {
    ($a:expr, $b:expr, $tol:expr $(,)?) => {{
        let (a, b, tol) = (f64::from($a), f64::from($b), f64::from($tol));
        let diff = (a - b).abs();
        assert!(
            $crate::internals::assertions::is_close(a, b, tol),
            "{a} !~ {b} (tol {tol:e}, diff {diff:e})"
        );
    }};
    ($a:expr, $b:expr, $tol:expr, $($arg:tt)+) => {{
        let (a, b, tol) = (f64::from($a), f64::from($b), f64::from($tol));
        let diff = (a - b).abs();
        assert!(
            $crate::internals::assertions::is_close(a, b, tol),
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
        $crate::internals::assertions::assert_close_slice!($a, $b, $tol, "sequence")
    };
    ($a:expr, $b:expr, $tol:expr, $($arg:tt)+) => {{
        let (a, b, tol) = (&$a[..], &$b[..], f64::from($tol));
        let what = format!($($arg)+);
        assert_eq!(a.len(), b.len(), "{what}: lengths differ");
        for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
            let (x, y) = (f64::from(*x), f64::from(*y));
            let diff = (x - y).abs();
            assert!(
                $crate::internals::assertions::is_close(x, y, tol),
                "{what}[{i}]: {x} !~ {y} (tol {tol:e}, diff {diff:e})"
            );
        }
    }};
}
pub(crate) use assert_close_slice;

#[cfg(test)]
mod tests {
    use crate::internals::assertions::is_close;

    /// The bound is absolute at every magnitude: at 1e6 a miss of 1e-5 fails a 1e-10 bound.
    #[test]
    fn the_bound_is_absolute() {
        assert!(is_close(0.0, 1e-10, 1e-10));
        assert!(!is_close(0.0, 1.1e-10, 1e-10));
        assert!(!is_close(1e6, 1e6 + 1e-5, 1e-10));
        assert!(is_close(1e6, 1e6 + 1e-5, 1e-5));
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
        assert!(!is_close(2.5, 2.500_000_1, 0.0));
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
