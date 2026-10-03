//! Every Isa against the scalar form.

use crate::io::raw::normalize::normalize_one;
use crate::io::raw::normalize::simd::NormalizeChunk;
use crate::simd::tier::Tier;

/// Every Isa, in both modes, equals the scalar form bit for bit: it applies the same IEEE
/// operations in the same order. The samples span zero, below black, black, mid-range, the 14-bit
/// maximum, and above it; every length from 0 through 17 puts each lane count of the 8-lane
/// remainder after zero, one and two vectors.
#[test]
fn every_tier_matches_scalar_bit_for_bit() {
    let (black, span) = (512.0, 16_383.0 - 512.0);
    let samples: Vec<u16> = [
        0, 1, 256, 511, 512, 513, 1000, 8191, 16_383, 16_384, 60_000, 65_535,
    ]
    .into_iter()
    .chain((0..39u16).map(|i| i.wrapping_mul(421)))
    .collect();
    for tier in Tier::supported() {
        for len in (0..=17).chain([samples.len()]) {
            let input = &samples[..len];
            for clamp in [true, false] {
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
                if clamp {
                    tier.run(NormalizeChunk::<true> {
                        input,
                        output: &mut got,
                        black,
                        span,
                    });
                } else {
                    tier.run(NormalizeChunk::<false> {
                        input,
                        output: &mut got,
                        black,
                        span,
                    });
                }
                assert_eq!(got, expected, "{tier} len={len} clamp={clamp}");
            }
        }
    }
}
