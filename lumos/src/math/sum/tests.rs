//! Tests for sum operations.

use crate::math::sum::{mean_f32, scalar, sum_f32, weighted_mean_f32};

/// Lengths that straddle every gate and its remainder: under the 4-lane NEON minimum, under the
/// 8-lane AVX2 one, exactly on each, and well past both.
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

/// The f64 totals themselves may differ from sequential only by reassociation, which is bounded far
/// below one f32 ULP of the sum's magnitude.
#[test]
fn sum_f32_stays_within_reassociation_error_of_a_sequential_reference() {
    let values: Vec<f32> = (0..10_000).map(|i| 100.0 + (i as f32) * 0.01).collect();
    let reference = sequential_f64(&values);
    let error = (sum_f32(&values) - reference).abs();

    // n·2⁻⁵³ relative, with room to spare; an f32 accumulator would be ~2⁻²⁴ relative and fail.
    let bound = reference.abs() * (values.len() as f64) * f64::EPSILON;
    assert!(error <= bound, "error {error:e} exceeded bound {bound:e}");
}

/// A wide accumulator recovers small values a naive f32 sum drops on the floor. Naive f32 would
/// give 1e6 + 0.1 == 1e6 ten thousand times over, then 1e6 - 1e6 == 0.
#[test]
fn sum_f32_recovers_values_a_narrow_accumulator_would_lose() {
    let mut values = vec![1e6f32];
    values.extend(std::iter::repeat_n(0.1f32, 10_000));
    values.push(-1e6f32);

    // 10_000 × the f32 nearest 0.1, summed exactly in f64.
    let expected = f64::from(0.1f32) * 10_000.0;
    assert!((sum_f32(&values) - expected).abs() < 1e-9);
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

/// The combine reaches the same pixel through `mean_f32` (equal weights) or `weighted_mean_f32`
/// (frame weights), so the two must not disagree on the same data.
///
/// This is exact by construction, not by numerical luck: `v * 1.0` is exact in f64 and each
/// backend accumulates the weighted numerator with the same lane split, reduction order and scalar
/// tail as its own `sum_f32`, so both walk the identical values through the identical additions.
/// The lengths have to cross both gates — 4 on NEON, 8 on AVX2 — or the sweep never leaves the
/// scalar path on one of the two architectures.
#[test]
fn mean_agrees_bit_for_bit_with_the_unit_weighted_mean() {
    for len in LENGTHS {
        let values: Vec<f32> = (0..len).map(|i| 0.1 + (i as f32) * 0.0137).collect();
        let ones = vec![1.0f32; len];
        assert_eq!(
            mean_f32(&values).to_bits(),
            weighted_mean_f32(&values, &ones).to_bits(),
            "mean and unit-weighted mean disagree at len {len}"
        );
    }
}

/// Rounding once beats rounding twice, so `mean_f32` must not be reachable as `sum` then divide.
/// Exact mean is 0.47267352491617204; one rounding gives 0.472673535 (bits 1056047684), two give
/// 0.472673506 (bits 1056047683).
#[test]
fn mean_f32_rounds_once_not_twice() {
    let values = [0.98646706f32, 0.68272305, 0.3804413, 0.23075151, 0.08298469];
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

/// Crossing each backend's gate must not change the answer, so scalar and dispatched results have
/// to agree either side of it. Lengths 3/4/5 straddle NEON's, 7/8/9 AVX2's.
#[test]
fn weighted_mean_agrees_with_scalar_across_every_gate() {
    for len in LENGTHS {
        let values: Vec<f32> = (0..len).map(|i| 500.0 + (i as f32) * 0.03).collect();
        let weights: Vec<f32> = (0..len).map(|i| 2.0 - (i as f32) * 0.0001).collect();

        let sums = scalar::weighted_sums(&values, &weights);
        let expected = (sums.weighted_values / sums.weight_total) as f32;
        assert_eq!(weighted_mean_f32(&values, &weights), expected, "len={len}");
    }
}

/// Large values cancelling against varying weights still has to land on the f64 reference.
#[test]
fn weighted_mean_survives_catastrophic_cancellation() {
    let mut values = vec![1e6f32, -1e6f32];
    let mut weights = vec![1.0f32, 1.0f32];
    for i in 0..1000 {
        values.push(0.1);
        weights.push(1.0 + (i as f32) * 0.001);
    }

    let numerator: f64 = values
        .iter()
        .zip(&weights)
        .map(|(&v, &w)| f64::from(v) * f64::from(w))
        .sum();
    let denominator: f64 = weights.iter().map(|&w| f64::from(w)).sum();
    let expected = (numerator / denominator) as f32;

    assert!((weighted_mean_f32(&values, &weights) - expected).abs() < 1e-4);
}

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
