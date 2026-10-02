use rayon::prelude::*;

mod simd;

/// Light-frame normalization: `clamp((value - black).max(0) / span, 0, 1)`.
///
/// Divided, not multiplied by a reciprocal: `1 / span` is inexact in f32 for most spans, and the
/// product would round twice.
/// This bounds direct RAW sensor input; demosaic interpolation itself remains unclipped.
pub(crate) fn normalize_u16_to_f32_parallel(data: &[u16], black: f32, span: f32) -> Vec<f32> {
    normalize_generic::<true>(data, black, span)
}

/// Shared parallel driver. `CLAMP` is a compile-time switch so each variant
/// monomorphizes to branch-free SIMD — the light path keeps its `[0, 1]` clamp,
/// the calibration path drops it, with no duplicated kernel.
fn normalize_generic<const CLAMP: bool>(data: &[u16], black: f32, span: f32) -> Vec<f32> {
    const CHUNK_SIZE: usize = 16384; // Process 64KB chunks (16K * 4 bytes)

    let mut result = vec![0.0f32; data.len()];

    result
        .par_chunks_mut(CHUNK_SIZE)
        .zip(data.par_chunks(CHUNK_SIZE))
        .for_each(|(out_chunk, in_chunk)| {
            normalize_u16_to_f32_into::<CLAMP>(in_chunk, out_chunk, black, span);
        });

    result
}

/// Scalar form of the per-pixel transform, shared by the fallback and the SIMD
/// remainders. `CLAMP` gates the `[0, 1]` floor/ceil.
#[inline(always)]
fn normalize_one<const CLAMP: bool>(val: u16, black: f32, span: f32) -> f32 {
    let subtracted = f32::from(val) - black;
    if CLAMP {
        (subtracted.max(0.0) / span).min(1.0)
    } else {
        subtracted / span
    }
}

/// Normalize `input` directly into equally sized caller-owned storage.
#[inline]
pub(crate) fn normalize_u16_to_f32_into<const CLAMP: bool>(
    input: &[u16],
    output: &mut [f32],
    black: f32,
    span: f32,
) {
    debug_assert_eq!(input.len(), output.len());
    simd::normalize_chunk::<CLAMP>(input, output, black, span);
}

#[cfg(test)]
mod tests {
    use crate::io::raw::normalize::*;

    /// Pure-scalar reference for cross-checking the SIMD kernels.
    fn scalar_ref<const CLAMP: bool>(data: &[u16], black: f32, span: f32) -> Vec<f32> {
        data.iter()
            .map(|&v| normalize_one::<CLAMP>(v, black, span))
            .collect()
    }

    #[test]
    fn simd_matches_scalar() {
        // The dispatched kernel (NEON on aarch64, SSE4.1/SSE2 on x86) uses the same IEEE ops as
        // the scalar form, so results must be bit-identical for both the clamped (light) and
        // unclamped (calibration) paths. Values span zero, below-black, at-black, mid, max, and
        // above-max; the length is deliberately not a multiple of 4 so the remainder path runs.
        let black = 512.0;
        let span = 16383.0 - 512.0;
        let mut data: Vec<u16> = vec![
            0, 1, 256, 511, 512, 513, 1000, 8191, 16383, 16384, 60000, 65535,
        ];
        for i in 0..39u16 {
            data.push(i.wrapping_mul(421));
        }
        assert!(
            !data.len().is_multiple_of(4),
            "length must exercise the SIMD remainder path"
        );

        let mut simd_clamped = vec![0.0f32; data.len()];
        normalize_u16_to_f32_into::<true>(&data, &mut simd_clamped, black, span);
        assert_eq!(
            simd_clamped,
            scalar_ref::<true>(&data, black, span),
            "clamped SIMD must match scalar"
        );

        let mut simd_unclamped = vec![0.0f32; data.len()];
        normalize_u16_to_f32_into::<false>(&data, &mut simd_unclamped, black, span);
        assert_eq!(
            simd_unclamped,
            scalar_ref::<false>(&data, black, span),
            "unclamped SIMD must match scalar"
        );
    }

    /// Every 14-bit sample normalizes to the correctly rounded quotient `(value − black) / span`:
    /// the f64 quotient of two integers below 2^24, rounded once to f32. Multiplying by an f32
    /// `1 / span` instead rounds twice; for black 1023 and span 15360 that misses the quotient on
    /// 10 756 of these 16 384 values (measured by this sweep).
    #[test]
    fn normalization_is_the_correctly_rounded_quotient() {
        let (black, span) = (1023.0f32, 15_360.0f32);
        let data: Vec<u16> = (0..=16_383).collect();
        let mut got = vec![0.0; data.len()];
        normalize_u16_to_f32_into::<false>(&data, &mut got, black, span);
        let mut reciprocal_misses = 0;
        for (&value, &normalized) in data.iter().zip(&got) {
            let exact = ((f64::from(value) - f64::from(black)) / f64::from(span)) as f32;
            assert_eq!(normalized.to_bits(), exact.to_bits(), "value {value}");
            if (f32::from(value) - black) * (1.0 / span) != exact {
                reciprocal_misses += 1;
            }
        }
        assert_eq!(
            reciprocal_misses, 10_756,
            "the double rounding this replaced"
        );
    }
}
