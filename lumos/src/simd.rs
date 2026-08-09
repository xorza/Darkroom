//! The crate's one architecture-dispatch prologue.
//!
//! Every hand-written SIMD kernel is reached through [`dispatch!`], which expands to the x86_64
//! feature ladder, the aarch64 NEON arm and the scalar fallback in one fixed shape. Each kernel
//! family used to write its own ladder, and they disagreed on arm order, on where the `unsafe`
//! block and its SAFETY note went, and on which of four spellings kept the compiler from calling
//! the fallback unreachable.

/// Selects a SIMD backend by architecture and runtime CPU features, falling back to scalar.
///
/// ```ignore
/// dispatch! {
///     x86: avx2   if width >= AVX2_MIN_ROW_WIDTH  => x86::median_row_avx2(rows, out, width),
///     x86: sse4_1 if width >= SSE41_MIN_ROW_WIDTH => x86::median_row_sse41(rows, out, width),
///     aarch64     if width >= NEON_MIN_ROW_WIDTH  => neon::median_row_neon(rows, out, width),
///     scalar => median_row_scalar(rows, out, width),
/// }
/// ```
///
/// Arms are tried in the order written and each one returns from the enclosing function, so the
/// invocation has to be in tail position. The ISA token is one of `avx2`, `avx2_fma`, `sse4_1` or
/// `sse2`; `if <guard>` is optional on every arm, and the `aarch64` arm may be left out entirely
/// for a kernel with no NEON backend.
///
/// # Safety
/// Every arm's expression is evaluated inside an `unsafe` block, so an arm must call a backend
/// whose only precondition is the ISA the macro has just established — an `x86` arm runs with its
/// feature detected, the `aarch64` arm runs only on aarch64, where NEON is unconditional. A
/// backend with any further precondition (a minimum length, a bound) states it in `if <guard>`,
/// which is checked in the same expression.
macro_rules! dispatch {
    (@available avx2) => {
        ::imaginarium::cpu_features::has_avx2()
    };
    (@available avx2_fma) => {
        ::imaginarium::cpu_features::has_avx2_fma()
    };
    (@available sse4_1) => {
        ::imaginarium::cpu_features::has_sse4_1()
    };
    (@available sse2) => {
        ::imaginarium::cpu_features::has_sse2()
    };

    (@x86 $feat:ident, $call:expr) => {
        if $crate::simd::dispatch!(@available $feat) {
            return unsafe { $call };
        }
    };
    (@x86 $feat:ident, $call:expr, $guard:expr) => {
        if $guard && $crate::simd::dispatch!(@available $feat) {
            return unsafe { $call };
        }
    };

    (@neon $call:expr) => {
        return unsafe { $call };
    };
    (@neon $call:expr, $guard:expr) => {
        if $guard {
            return unsafe { $call };
        }
    };

    (
        $( x86: $feat:ident $( if $x86_guard:expr )? => $x86_call:expr, )*
        $( aarch64 $( if $neon_guard:expr )? => $neon_call:expr, )?
        scalar => $scalar:expr $(,)?
    ) => {{
        $(
            #[cfg(target_arch = "x86_64")]
            {
                $crate::simd::dispatch!(@x86 $feat, $x86_call $(, $x86_guard)?);
            }
        )*
        $(
            #[cfg(target_arch = "aarch64")]
            {
                $crate::simd::dispatch!(@neon $neon_call $(, $neon_guard)?);
            }
        )?
        #[allow(unreachable_code)]
        return $scalar;
    }};
}

pub(crate) use dispatch;

#[cfg(test)]
mod tests {
    /// Every arm returns the tag of the backend it stands for, so a test can name which rung the
    /// ladder took on the machine it is running on without any SIMD in the picture.
    fn taken(force_scalar: bool) -> &'static str {
        dispatch! {
            x86: avx2_fma if !force_scalar => avx2_fma_backend(),
            x86: sse4_1 if !force_scalar => sse41_backend(),
            aarch64 if !force_scalar => neon_backend(),
            scalar => "scalar",
        }
    }

    #[cfg(target_arch = "x86_64")]
    unsafe fn avx2_fma_backend() -> &'static str {
        "avx2_fma"
    }

    #[cfg(target_arch = "x86_64")]
    unsafe fn sse41_backend() -> &'static str {
        "sse4_1"
    }

    #[cfg(target_arch = "aarch64")]
    unsafe fn neon_backend() -> &'static str {
        "neon"
    }

    /// A guard that is false on every arm has to reach the fallback whatever the CPU offers, and
    /// the same ladder with the guard true has to leave it — otherwise `dispatch!` would be
    /// silently scalar everywhere.
    #[test]
    fn a_false_guard_falls_through_to_scalar_and_a_true_one_does_not() {
        assert_eq!(taken(true), "scalar");

        let expected = if cfg!(target_arch = "aarch64") {
            "neon"
        } else if cfg!(target_arch = "x86_64") && imaginarium::cpu_features::has_avx2_fma() {
            "avx2_fma"
        } else if cfg!(target_arch = "x86_64") && imaginarium::cpu_features::has_sse4_1() {
            "sse4_1"
        } else {
            "scalar"
        };
        assert_eq!(taken(false), expected);
    }

    /// The ladder is ordered, not "best available": an arm whose guard fails hands the same input
    /// to the next rung down rather than to the fallback.
    #[test]
    fn a_failing_rung_falls_to_the_next_rung_not_to_scalar() {
        fn skip_first(skip: bool) -> &'static str {
            dispatch! {
                x86: avx2_fma if !skip => avx2_fma_backend(),
                x86: sse4_1 => sse41_backend(),
                aarch64 if !skip => neon_backend(),
                scalar => "scalar",
            }
        }

        #[cfg(target_arch = "x86_64")]
        if imaginarium::cpu_features::has_sse4_1() {
            assert_eq!(skip_first(true), "sse4_1");
        }
        #[cfg(target_arch = "aarch64")]
        assert_eq!(skip_first(true), "scalar");
    }
}
