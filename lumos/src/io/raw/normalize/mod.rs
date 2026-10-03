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
/// monomorphizes to a branch-free kernel — the light path keeps its `[0, 1]` clamp,
/// the calibration path drops it, with no duplicated kernel.
fn normalize_generic<const CLAMP: bool>(data: &[u16], black: f32, span: f32) -> Vec<f32> {
    const CHUNK_SIZE: usize = 16384;

    let mut result = vec![0.0f32; data.len()];

    result
        .par_chunks_mut(CHUNK_SIZE)
        .zip(data.par_chunks(CHUNK_SIZE))
        .for_each(|(out_chunk, in_chunk)| {
            normalize_u16_to_f32_into::<CLAMP>(in_chunk, out_chunk, black, span);
        });

    result
}

/// Scalar form of the per-pixel transform: the kernel's remainder, and the reference it is tested
/// against. `CLAMP` gates the `[0, 1]` floor/ceil.
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

    /// The light path floors below black at 0 and caps above the maximum at 1, and between them
    /// is the correctly rounded quotient above; the calibration path keeps both tails. The
    /// parallel driver works in 16 384-sample chunks, so 100 000 samples cross six chunk joins,
    /// and it matches one call over the whole input bit for bit.
    #[test]
    fn light_path_clamps_and_calibration_path_keeps_the_tails() {
        let (black, span) = (512.0f32, 15_871.0f32);
        let quotient = |value: u16| ((f64::from(value) - 512.0) / 15_871.0) as f32;
        for (value, light, calibration) in [
            (0u16, 0.0, quotient(0)),
            (511, 0.0, quotient(511)),
            (512, 0.0, 0.0),
            (8447, quotient(8447), quotient(8447)),
            (16_383, 1.0, 1.0),
            (20_000, 1.0, quotient(20_000)),
        ] {
            let mut clamped = [0.0];
            normalize_u16_to_f32_into::<true>(&[value], &mut clamped, black, span);
            let mut unclamped = [0.0];
            normalize_u16_to_f32_into::<false>(&[value], &mut unclamped, black, span);
            assert_eq!(clamped[0].to_bits(), light.to_bits(), "light {value}");
            assert_eq!(
                unclamped[0].to_bits(),
                calibration.to_bits(),
                "calibration {value}"
            );
        }
        assert!(quotient(0) < 0.0 && quotient(20_000) > 1.0);

        let data: Vec<u16> = (0..100_000u32)
            .map(|index| (index * 7 % 65_536) as u16)
            .collect();
        let parallel = normalize_u16_to_f32_parallel(&data, black, span);
        let mut single = vec![0.0; data.len()];
        normalize_u16_to_f32_into::<true>(&data, &mut single, black, span);
        assert!(
            parallel
                .iter()
                .zip(&single)
                .all(|(a, b)| a.to_bits() == b.to_bits())
        );
    }
}
