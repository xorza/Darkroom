use rayon::prelude::*;

mod simd;

/// Light-frame normalization: `clamp((value - black).max(0) * inv_range, 0, 1)`.
/// This bounds direct RAW sensor input; demosaic interpolation itself remains unclipped.
pub(crate) fn normalize_u16_to_f32_parallel(data: &[u16], black: f32, inv_range: f32) -> Vec<f32> {
    normalize_generic::<true>(data, black, inv_range)
}

/// Shared parallel driver. `CLAMP` is a compile-time switch so each variant
/// monomorphizes to branch-free SIMD — the light path keeps its `[0, 1]` clamp,
/// the calibration path drops it, with no duplicated kernel.
fn normalize_generic<const CLAMP: bool>(data: &[u16], black: f32, inv_range: f32) -> Vec<f32> {
    const CHUNK_SIZE: usize = 16384; // Process 64KB chunks (16K * 4 bytes)

    let mut result = vec![0.0f32; data.len()];

    result
        .par_chunks_mut(CHUNK_SIZE)
        .zip(data.par_chunks(CHUNK_SIZE))
        .for_each(|(out_chunk, in_chunk)| {
            normalize_u16_to_f32_into::<CLAMP>(in_chunk, out_chunk, black, inv_range);
        });

    result
}

/// Scalar form of the per-pixel transform, shared by the fallback and the SIMD
/// remainders. `CLAMP` gates the `[0, 1]` floor/ceil.
#[inline(always)]
fn normalize_one<const CLAMP: bool>(val: u16, black: f32, inv_range: f32) -> f32 {
    let subtracted = (val as f32) - black;
    if CLAMP {
        (subtracted.max(0.0) * inv_range).min(1.0)
    } else {
        subtracted * inv_range
    }
}

/// Normalize `input` directly into equally sized caller-owned storage.
#[inline]
pub(crate) fn normalize_u16_to_f32_into<const CLAMP: bool>(
    input: &[u16],
    output: &mut [f32],
    black: f32,
    inv_range: f32,
) {
    debug_assert_eq!(input.len(), output.len());
    simd::normalize_chunk::<CLAMP>(input, output, black, inv_range);
}

#[cfg(test)]
mod tests {
    use crate::io::raw::normalize::*;

    /// Pure-scalar reference for cross-checking the SIMD kernels.
    fn scalar_ref<const CLAMP: bool>(data: &[u16], black: f32, inv_range: f32) -> Vec<f32> {
        data.iter()
            .map(|&v| normalize_one::<CLAMP>(v, black, inv_range))
            .collect()
    }

    #[test]
    fn simd_matches_scalar() {
        // The dispatched kernel (NEON on aarch64, SSE4.1/SSE2 on x86) uses the same IEEE ops as
        // the scalar form, so results must be bit-identical for both the clamped (light) and
        // unclamped (calibration) paths. Values span zero, below-black, at-black, mid, max, and
        // above-max; the length is deliberately not a multiple of 4 so the remainder path runs.
        let black = 512.0;
        let inv_range = 1.0 / (16383.0 - 512.0);
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
        normalize_u16_to_f32_into::<true>(&data, &mut simd_clamped, black, inv_range);
        assert_eq!(
            simd_clamped,
            scalar_ref::<true>(&data, black, inv_range),
            "clamped SIMD must match scalar"
        );

        let mut simd_unclamped = vec![0.0f32; data.len()];
        normalize_u16_to_f32_into::<false>(&data, &mut simd_unclamped, black, inv_range);
        assert_eq!(
            simd_unclamped,
            scalar_ref::<false>(&data, black, inv_range),
            "unclamped SIMD must match scalar"
        );
    }
}
