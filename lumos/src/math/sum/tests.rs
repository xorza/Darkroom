//! Tests for sum operations.

#[cfg(target_arch = "x86_64")]
use crate::math::sum::AVX2_SUM_F32_CROSSOVER;
use crate::math::sum::{mean_f32, scalar, sum_f32, weighted_mean_f32};
#[cfg(target_arch = "x86_64")]
use crate::simd::AVX2_F32_LANES;
#[cfg(target_arch = "x86_64")]
use crate::testing::simd_check;
use crate::testing::test_rng::TestRng;
#[cfg(target_arch = "x86_64")]
use imaginarium::SimdTier;
use std::iter;

/// Lengths that straddle every gate and its remainder: under the 4-lane NEON minimum, under the
/// 8-lane one `weighted_sums` uses on x86, under `sum_f32`'s measured crossover at 16, exactly on
/// each, and well past all three.
const LENGTHS: [usize; 15] = [1, 2, 3, 4, 5, 7, 8, 9, 16, 17, 63, 64, 65, 257, 1000];

/// Cycles 1e6 against -1e6 so a running total cancels catastrophically, with small values between
/// that a narrow accumulator would lose entirely.
fn cancelling_values(len: usize) -> Vec<f32> {
    (0..len)
        .map(|index| match index % 4 {
            0 => 1e6,
            1 => 0.25,
            2 => -1e6,
            _ => 0.5,
        })
        .collect()
}

fn sequential_f64(values: &[f32]) -> f64 {
    values.iter().map(|&value| f64::from(value)).sum()
}

/// The vector backends reassociate, so their f64 total is not bit-identical to a sequential one.
/// What they owe is the *rounded* result: an f64 accumulator carries n·2⁻⁵³ against f32's 2⁻²⁴
/// granularity, so a reassociated f64 sum has to round to the same f32 as the sequential one.
/// A tolerance here would pass an implementation accumulating in f32.
#[test]
fn sum_f32_rounds_to_the_same_f32_as_a_sequential_reference() {
    for len in LENGTHS {
        let values = cancelling_values(len);
        assert_eq!(
            sum_f32(&values) as f32,
            sequential_f64(&values) as f32,
            "len={len}"
        );
    }
}

/// The sum of `values` by Neumaier's compensated summation in f64: every addition's rounding error
/// carried in a second accumulator, which leaves it within `ε·|sum| + O(n·ε²)·Σ|xᵢ|` of the exact
/// total — orders of magnitude inside the reassociation bound it is the reference for.
fn exact_sum(values: &[f32]) -> f64 {
    let (mut sum, mut compensation) = (0.0f64, 0.0f64);
    for &value in values {
        let value = f64::from(value);
        let next = sum + value;
        compensation += if sum.abs() >= value.abs() {
            (sum - next) + value
        } else {
            (value - next) + sum
        };
        sum = next;
    }
    sum + compensation
}

/// Reassociating an f64 sum moves it by at most `(n − 1)·ε·Σ|xᵢ|` (Higham, *Accuracy and Stability*,
/// §4.2) from the exact total, whatever the order — the bound the backends' lane split must keep.
/// The fixture spreads its magnitudes over twenty decades, so f64 partial sums do round: the
/// sequential sum is off the exact one, and the bound is not met by exactness alone. An f32
/// accumulator would be off by `n·2⁻²⁴` relative and fail it.
#[test]
fn sum_f32_stays_within_reassociation_error_of_the_exact_sum() {
    let mut rng = TestRng::new(17);
    let values: Vec<f32> = (0..10_000)
        .map(|i| (rng.next_f32() - 0.3) * 10f32.powi(i % 20 - 10))
        .collect();
    let exact = exact_sum(&values);
    assert_ne!(
        sequential_f64(&values),
        exact,
        "the fixture's f64 sums round"
    );
    let absolute: f64 = values.iter().map(|&v| f64::from(v).abs()).sum();
    let bound = (values.len() - 1) as f64 * f64::EPSILON * absolute;
    for (path, total) in [
        ("sum_f32", sum_f32(&values)),
        ("scalar", scalar::sum_f32(&values)),
    ] {
        let error = (total - exact).abs();
        assert!(error <= bound, "{path}: {error:e} past {bound:e}");
    }
}

/// A wide accumulator recovers small values a naive f32 sum drops on the floor. Naive f32 would
/// give 1e6 + 0.1 == 1e6 ten thousand times over, then 1e6 - 1e6 == 0.
#[test]
fn sum_f32_recovers_values_a_narrow_accumulator_would_lose() {
    let mut values = vec![1e6f32];
    values.extend(iter::repeat_n(0.1f32, 10_000));
    values.push(-1e6f32);

    // 10 000 × the f32 nearest 0.1: every partial sum spans 2²⁰ down to 0.1's last bit at 2⁻²⁷, 47
    // bits, so f64 holds each one exactly and the total is exact.
    assert_eq!(sum_f32(&values), f64::from(0.1f32) * 10_000.0);
}

/// 100k ones is exactly representable at every partial sum, so any correct accumulator is exact.
#[test]
fn sum_f32_is_exact_on_exactly_representable_input() {
    let n = 100_000;
    assert_eq!(sum_f32(&vec![1.0f32; n]), n as f64);
    assert_eq!(sum_f32(&[]), 0.0);
}

/// `mean_f32` divides an f64 accumulation once and rounds once.
#[test]
fn mean_f32_matches_a_rounded_f64_reference_at_every_length() {
    for len in LENGTHS {
        let values = cancelling_values(len);
        let expected = (sequential_f64(&values) / len as f64) as f32;
        assert_eq!(mean_f32(&values), expected, "len={len}");
    }
}

/// Wherever `mean_f32` and `weighted_mean_f32` reach the same rung, the unit-weighted mean is the
/// plain mean bit for bit — by construction, not by numerical luck: `v * 1.0` is exact in f64 and
/// each backend accumulates the weighted numerator with the same lane split, reduction order and
/// scalar tail as its own `sum_f32`, so both walk the identical values through the identical
/// additions. The lengths have to cross every gate — 4 on NEON, 8 and 16 on x86 — or the sweep
/// never leaves the scalar path on one of the architectures.
#[test]
fn mean_agrees_bit_for_bit_with_the_unit_weighted_mean() {
    for len in LENGTHS.into_iter().filter(|&len| !gates_differ(len)) {
        let values: Vec<f32> = (0..len).map(|i| 0.1 + (i as f32) * 0.0137).collect();
        let ones = vec![1.0f32; len];
        assert_eq!(
            mean_f32(&values).to_bits(),
            weighted_mean_f32(&values, &ones).to_bits(),
            "mean and unit-weighted mean disagree at len {len}"
        );
    }
}

/// Lengths where the two entry points take different rungs, so their sums associate differently.
///
/// On x86 `weighted_sums` goes vector at the 8-lane minimum while `sum_f32` waits for its measured
/// crossover at 16. Without the AVX2 rung — a pre-AVX2 CPU, or Rosetta, which reports sse4.1 and no
/// avx2 — both are scalar and the window closes, as it is closed on every other architecture.
fn gates_differ(len: usize) -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        SimdTier::Avx2.is_supported() && (AVX2_F32_LANES..AVX2_SUM_F32_CROSSOVER).contains(&len)
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = len;
        false
    }
}

/// The window `gates_differ` excludes is a real divergence, not a technicality worth ignoring, and
/// pinning it keeps that exclusion honest: unify the gates again and this stops finding its witness
/// and fails, rather than the exclusion quietly covering nothing.
///
/// Eight elements is one full AVX2 chunk with no tail. The `±1e7` pairs annihilate, leaving
/// `2·0.001 + 2·0.0003` ≈ 3.25e-4 against terms of 1e7 — a ratio of 3e10. Neither residue is a power
/// of two, so their low mantissa bits fall below the running total's f64 ULP of 2⁻²⁹ and get rounded
/// away — differently by each order. `mean_f32` adds left to right; `weighted_mean_f32` sums lanes
/// 0-3 and 4-7 apart before pairing them. The two land 2 f32 ULPs apart.
#[test]
#[cfg(target_arch = "x86_64")]
fn the_split_gate_window_is_where_the_two_entry_points_diverge() {
    if !simd_check::runs_here(SimdTier::Avx2) {
        return;
    }

    assert_eq!(
        (1..64).filter(|&len| gates_differ(len)).collect::<Vec<_>>(),
        (8..16).collect::<Vec<_>>()
    );

    let values = [1e7f32, 0.001, -1e7, 0.0003, 1e7, 0.001, -1e7, 0.0003];
    assert_eq!(mean_f32(&values).to_bits(), 967_468_226);
    assert_eq!(
        weighted_mean_f32(&values, &[1.0f32; 8]).to_bits(),
        967_468_224
    );
}

/// Rounding once beats rounding twice, so `mean_f32` must not be reachable as `sum` then divide.
/// Exact mean is 0.47267352491617204; one rounding gives 0.472673535 (bits 1056047684), two give
/// 0.472673506 (bits 1056047683).
#[test]
fn mean_f32_rounds_once_not_twice() {
    let values = [
        0.986_467_06_f32,
        0.682_723_05,
        0.380_441_3,
        0.230_751_51,
        0.082_984_69,
    ];
    let once = mean_f32(&values);
    let twice = (sum_f32(&values) as f32) / values.len() as f32;
    assert_ne!(
        once.to_bits(),
        twice.to_bits(),
        "witness no longer distinguishes one rounding from two"
    );

    let exact = sequential_f64(&values) / values.len() as f64;
    assert!(
        (f64::from(once) - exact).abs() < (f64::from(twice) - exact).abs(),
        "rounding once must land closer to the exact mean than rounding twice"
    );
}

/// Hand-computed weighted means, including the cases where the weights carry the answer.
#[test]
fn weighted_mean_matches_hand_computed_values() {
    // (10·3 + 20·1) / 4 = 12.5
    assert_eq!(weighted_mean_f32(&[10.0, 20.0], &[3.0, 1.0]), 12.5);
    // Uniform weights collapse to the plain mean of 1..=5.
    assert_eq!(
        weighted_mean_f32(&[1.0, 2.0, 3.0, 4.0, 5.0], &[1.0; 5]),
        3.0
    );
    // Only the nonzero weight contributes.
    assert_eq!(
        weighted_mean_f32(&[10.0, 20.0, 30.0], &[0.0, 5.0, 0.0]),
        20.0
    );
    // A single sample is its own mean whatever its weight.
    assert_eq!(weighted_mean_f32(&[42.0], &[5.0]), 42.0);
    // Symmetric values with equal weights cancel exactly.
    assert_eq!(weighted_mean_f32(&[-10.0, 10.0], &[1.0, 1.0]), 0.0);
}

/// Every frame distrusted is data, not a fault: the pixel has no information, which is 0.0.
#[test]
fn weighted_mean_of_zero_total_weight_is_zero() {
    assert_eq!(weighted_mean_f32(&[1.0, 2.0, 3.0], &[0.0, 0.0, 0.0]), 0.0);
}

/// Crossing each backend's gate must not change the answer, on a smooth ramp and on large values
/// cancelling against varying weights. Every product is exact in f64, and the sums err by at most
/// `n·ε·1e6` ≈ 2e-7 absolute — 2e-10 in a mean over weights summing past 1 — far under a ulp of
/// either mean in f32, so every order of summation rounds to the sequential f64 reference's f32.
/// Lengths 3/4/5 straddle NEON's gate, 7/8/9 AVX2's.
#[test]
fn weighted_mean_agrees_with_the_f64_reference_across_every_gate() {
    for len in LENGTHS {
        let ramp: Vec<f32> = (0..len).map(|i| 500.0 + (i as f32) * 0.03).collect();
        let ramp_weights: Vec<f32> = (0..len).map(|i| 2.0 - (i as f32) * 0.0001).collect();
        let cancelling_weights: Vec<f32> = (0..len).map(|i| 1.0 + (i as f32) * 0.001).collect();
        for (name, values, weights) in [
            ("ramp", ramp, ramp_weights),
            ("cancelling", cancelling_values(len), cancelling_weights),
        ] {
            let numerator: f64 = values
                .iter()
                .zip(&weights)
                .map(|(&v, &w)| f64::from(v) * f64::from(w))
                .sum();
            let denominator: f64 = weights.iter().map(|&w| f64::from(w)).sum();
            let expected = (numerator / denominator) as f32;
            assert_eq!(
                weighted_mean_f32(&values, &weights),
                expected,
                "{name}, len={len}"
            );
        }
    }
}

/// Every screen below is a `debug_assert!`: the combine runs once per output pixel, so a release
/// build drops them and the calls return a number instead of panicking.
#[cfg(debug_assertions)]
mod contract {
    use super::*;

    #[test]
    #[should_panic(expected = "must have the same length")]
    fn weighted_mean_rejects_mismatched_lengths() {
        // `zip` would silently truncate to the shorter slice and return a mean over a subset.
        let _ = weighted_mean_f32(&[1.0, 2.0, 3.0, 4.0, 5.0], &[1.0, 1.0, 1.0]);
    }

    #[test]
    #[should_panic(expected = "cannot sum negative")]
    fn weighted_mean_rejects_negative_weights() {
        let _ = weighted_mean_f32(&[1.0, 2.0], &[1.0, -2.0]);
    }

    #[test]
    #[should_panic(expected = "empty slice")]
    fn weighted_mean_rejects_an_empty_slice() {
        let _ = weighted_mean_f32(&[], &[]);
    }

    #[test]
    #[should_panic(expected = "empty slice")]
    fn mean_rejects_an_empty_slice() {
        let _ = mean_f32(&[]);
    }
}
