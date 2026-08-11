//! Sum and accumulation operations with SIMD acceleration.

pub(crate) mod scalar;
pub(crate) mod simd;

/// Mean of f32 values, rounded to f32 exactly once.
///
/// Accumulates and divides in f64 rather than reusing the SIMD [`simd::sum_f32`]. Two reasons, and
/// both are about the result rather than the loop:
///
/// - Rounding the sum to f32 and *then* dividing rounds twice, landing up to an extra ULP from
///   the exact mean.
/// - This is the unit-weight case of [`simd::weighted_mean_f32`], and the combine reaches the same
///   pixel through either function depending on whether frame weights are in play. Sharing the
///   f64 accumulate-then-divide shape makes the two agree bit-for-bit at every length and on every
///   architecture, so which entry point a stack takes cannot change its output.
///
/// Every caller averages over a frame count (tens), far below where the SIMD sum would pay — which
/// is why this one has no backend of its own and sits here rather than in [`simd`].
pub(crate) fn mean_f32(values: &[f32]) -> f32 {
    debug_assert!(!values.is_empty());
    (scalar::sum_f64(values) / values.len() as f64) as f32
}

#[cfg(test)]
mod tests;
