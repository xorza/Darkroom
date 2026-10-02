//! Every normalize backend the host has, against the scalar form.

use crate::io::raw::normalize::normalize_one;
#[cfg(target_arch = "aarch64")]
use crate::io::raw::normalize::simd::neon;
#[cfg(target_arch = "x86_64")]
use crate::io::raw::normalize::simd::{sse2, sse41};
use crate::testing::simd_check::backend::Backend;
use imaginarium::SimdTier;

type ChunkFn = unsafe fn(&[u16], &mut [f32], f32, f32);

/// A backend's two modes, named so neither can stand in for the other.
#[derive(Debug, Clone, Copy)]
struct Modes {
    clamped: ChunkFn,
    unclamped: ChunkFn,
}

/// SSE2 is the `x86_64` baseline, so its backend runs on every x86 host, though dispatch only
/// reaches it on one without SSE4.1.
const BACKENDS: &[Backend<Modes>] = &[
    #[cfg(target_arch = "x86_64")]
    Backend::new(
        SimdTier::Sse41,
        Modes {
            clamped: sse41::normalize_chunk_sse41::<true>,
            unclamped: sse41::normalize_chunk_sse41::<false>,
        },
    ),
    #[cfg(target_arch = "x86_64")]
    Backend::new(
        SimdTier::Sse2,
        Modes {
            clamped: sse2::normalize_chunk_sse2::<true>,
            unclamped: sse2::normalize_chunk_sse2::<false>,
        },
    ),
    #[cfg(target_arch = "aarch64")]
    Backend::new(
        SimdTier::Neon,
        Modes {
            clamped: neon::normalize_chunk_neon::<true>,
            unclamped: neon::normalize_chunk_neon::<false>,
        },
    ),
];

/// Every backend, in both modes, equals the scalar form bit for bit: it applies the same IEEE
/// operations in the same order. The samples span zero, below black, black, mid-range, the 14-bit
/// maximum, and above it; every length from 0 through 17 puts each lane count of the 4-lane
/// remainder after zero to four vectors.
#[test]
fn backends_match_scalar_bit_for_bit() {
    let (black, span) = (512.0, 16_383.0 - 512.0);
    let samples: Vec<u16> = [
        0, 1, 256, 511, 512, 513, 1000, 8191, 16_383, 16_384, 60_000, 65_535,
    ]
    .into_iter()
    .chain((0..39u16).map(|i| i.wrapping_mul(421)))
    .collect();
    for backend in Backend::supported(BACKENDS) {
        for len in (0..=17).chain([samples.len()]) {
            let input = &samples[..len];
            for (clamp, kernel) in [
                (true, backend.kernel.clamped),
                (false, backend.kernel.unclamped),
            ] {
                let expected: Vec<f32> = input
                    .iter()
                    .map(|&value| {
                        if clamp {
                            normalize_one::<true>(value, black, span)
                        } else {
                            normalize_one::<false>(value, black, span)
                        }
                    })
                    .collect();
                let mut got = vec![0.0f32; len];
                // SAFETY: `supported` yields only backends whose tier this CPU has, and the two
                // slices have one length.
                unsafe { kernel(input, &mut got, black, span) };
                assert_eq!(got, expected, "{} len={len} clamp={clamp}", backend.tier);
            }
        }
    }
}
