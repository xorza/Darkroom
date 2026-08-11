//! Tests for statistical functions.

use crate::math::statistics::*;
use crate::testing::prelude::*;

#[derive(Debug)]
struct MedianCase {
    values: &'static [f32],
    expected: f32,
}

/// The inputs too small or too flat for clipping to do anything, with the result hand-derived in
/// each case. These were eight tests — the `_stats_` prefixed half called the same function as
/// the other half with different constants.
#[test]
fn sigma_clipped_degenerate_inputs() {
    struct Case {
        values: &'static [f32],
        median: f32,
        /// `None` where the case is about the median only.
        sigma: Option<f32>,
        why: &'static str,
    }

    let cases = [
        Case {
            values: &[],
            median: 0.0,
            sigma: Some(0.0),
            why: "nothing to summarise",
        },
        Case {
            values: &[5.0],
            median: 5.0,
            sigma: Some(0.0),
            why: "one value is its own median, no spread",
        },
        Case {
            values: &[0.5],
            median: 0.5,
            sigma: Some(0.0),
            why: "same, at a different level",
        },
        // Even length averages the two middle elements, and iteration stops below three values.
        Case {
            values: &[2.0, 4.0],
            median: 3.0,
            sigma: None,
            why: "(2+4)/2",
        },
        Case {
            values: &[0.3, 0.7],
            median: 0.5,
            sigma: None,
            why: "(0.3+0.7)/2",
        },
        Case {
            values: &[5.0; 100],
            median: 5.0,
            sigma: Some(0.0),
            why: "identical values have zero MAD",
        },
        Case {
            values: &[0.3; 100],
            median: 0.3,
            sigma: Some(0.0),
            why: "same, at a different level",
        },
    ];

    for case in &cases {
        let mut values = case.values.to_vec();
        let mut deviations = Vec::new();
        let stats = ClippedStats::sigma_clipped(&mut values, &mut deviations, 3.0, 3);
        assert_close!(
            stats.median,
            case.median,
            1e-6,
            "{:?}: {}",
            case.values,
            case.why
        );
        if let Some(sigma) = case.sigma {
            assert_close!(
                stats.sigma,
                sigma,
                1e-6,
                "{:?} sigma: {}",
                case.values,
                case.why
            );
        }
    }
}

#[test]
fn median_f32_truth_table() {
    let cases = [
        MedianCase {
            values: &[1.0, 3.0, 2.0, 5.0, 4.0],
            expected: 3.0,
        },
        MedianCase {
            values: &[1.0, 2.0, 3.0, 4.0],
            expected: 2.5,
        },
        MedianCase {
            values: &[1.0, 5.0],
            expected: 3.0,
        },
        MedianCase {
            values: &[42.0],
            expected: 42.0,
        },
        MedianCase {
            values: &[-5.0, -3.0, -1.0, 2.0, 4.0],
            expected: -1.0,
        },
    ];

    for case in cases {
        let mut values = case.values.to_vec();
        let actual = median_f32_mut(&mut values);
        assert!((actual - case.expected).abs() < f32::EPSILON, "{case:?}");
    }
}

#[test]
fn median_and_mad_odd() {
    let mut values = [2.0f32, 4.0, 3.0];
    let stats = MedianMad::of_mut(&mut values);
    assert!((stats.median - 3.0).abs() < 1e-6);
    assert!((stats.mad - 1.0).abs() < 1e-6);
    // 1.4826 × 1.0, the Gaussian rescale MedianMad::sigma applies.
    assert!((stats.sigma() - 1.4826022).abs() < 1e-6);
}

#[test]
fn median_and_mad_uniform() {
    let mut values = [3.5f32, 3.5, 3.5, 3.5, 3.5];
    let stats = MedianMad::of_mut(&mut values);
    assert!((stats.median - 3.5).abs() < 1e-6);
    assert!(stats.mad.abs() < 1e-6);
    assert!(stats.sigma().abs() < 1e-6);
}

#[test]
fn mad_with_scratch() {
    let values = [2.0f32, 4.0, 3.0];
    let mut scratch = Vec::new();
    let mad = mad_f32_with_scratch(&values, 3.0, &mut scratch);
    assert!((mad - 1.0).abs() < 1e-6);
}

#[test]
fn mad_with_scratch_empty() {
    let values: [f32; 0] = [];
    let mut scratch = Vec::new();
    let mad = mad_f32_with_scratch(&values, 0.0, &mut scratch);
    assert!(mad.abs() < f32::EPSILON);
}

#[test]
fn median_with_nan_does_not_panic() {
    let mut values = [1.0f32, f32::NAN, 3.0, 2.0, 5.0];
    // Should not panic — NaN sorts to end via total_cmp
    let median = median_f32_mut(&mut values);
    assert!(!median.is_nan());
}

/// NaN input is a contract violation, not a supported case: `partial_cmp` orders a NaN `Equal`
/// against everything, so the partition may return any element. Debug builds say so instead of
/// handing back a number that looks like a median. Release compiles the check out, so this test
/// only holds where `debug_assertions` is on.
#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "NaN-free")]
fn sigma_clip_rejects_nan_input() {
    let mut values = vec![10.0f32; 20];
    values[5] = f32::NAN;
    values[15] = f32::NAN;
    let mut deviations = Vec::new();
    ClippedStats::sigma_clipped(&mut values, &mut deviations, 3.0, 3);
}

/// How close a statistic has to land.
#[derive(Debug, Clone, Copy)]
struct Approx {
    value: f32,
    within: f32,
}

/// Bounds a case pins on sigma. Both `None` where the case says nothing about spread.
#[derive(Debug, Clone, Copy, Default)]
struct SigmaBounds {
    above: Option<f32>,
    below: Option<f32>,
}

/// One `ClippedStats::sigma_clipped` run and the statistics it must produce.
#[derive(Debug)]
struct ClipCase {
    name: &'static str,
    values: Vec<f32>,
    kappa: f32,
    iterations: usize,
    median: Approx,
    sigma: SigmaBounds,
    /// Where the case has a mean worth pinning. Most do not: `mean` is only meaningful alongside
    /// a positive sigma, for the reason the "three huge outliers" row records.
    mean: Option<Approx>,
}

/// `sigma_clipped` over every sample shape that mattered, as one table.
///
/// Nineteen tests fed this one function different constants under two naming families —
/// `sigma_clipped_*` and `sigma_clipped_stats_*` — that turned out to call the same thing. The
/// rows keep each fixture and its hand-derived expectations; what they gain is the length
/// invariant, asserted on every row where exactly one test checked it before.
#[test]
fn sigma_clipped_over_every_sample_shape() {
    fn flat(count: usize, value: f32) -> Vec<f32> {
        vec![value; count]
    }
    /// `count` values centred on `centre`, spaced `step` apart.
    fn spread(count: usize, centre: f32, step: f32) -> Vec<f32> {
        (0..count)
            .map(|i| centre + (i as f32 - count as f32 / 2.0) * step)
            .collect()
    }
    fn with(mut base: Vec<f32>, extra: &[f32]) -> Vec<f32> {
        base.extend_from_slice(extra);
        base
    }
    const NO_SIGMA: SigmaBounds = SigmaBounds {
        above: None,
        below: None,
    };

    let cases = vec![
        ClipCase {
            name: "smooth spread with nothing to clip",
            values: spread(100, 50.0, 0.1),
            kappa: 3.0,
            iterations: 3,
            median: Approx {
                value: 50.0,
                within: 1.0,
            },
            sigma: SigmaBounds {
                above: Some(0.0),
                below: Some(10.0),
            },
            mean: None,
        },
        // 97 identical values give MAD = 0, so the clip exits early (σ ≈ 0) *without* removing the
        // outliers and the mean covers the full contaminated sample: (97·10 + 6000)/100 = 69.7.
        // Consumers must treat `mean` as meaningful only alongside σ > 0 — the SExtractor sky
        // estimator's σ-gated crowding test does exactly that.
        ClipCase {
            name: "three huge outliers against a flat sample",
            values: with(flat(97, 10.0), &[1000.0, 2000.0, 3000.0]),
            kappa: 3.0,
            iterations: 3,
            median: Approx {
                value: 10.0,
                within: 0.1,
            },
            sigma: SigmaBounds {
                above: None,
                below: Some(1.0),
            },
            mean: Some(Approx {
                value: 69.7,
                within: 0.01,
            }),
        },
        // 100 is clipped in iteration 1 under any fast-median convention (threshold <= 13.3 while
        // |100 - median| >= 96). Survivors [1, 2, 4]: median 2, mean 7/3, MAD 1 so sigma = 1.4826.
        ClipCase {
            name: "asymmetric survivors",
            values: vec![1.0, 2.0, 4.0, 100.0],
            kappa: 3.0,
            iterations: 3,
            median: Approx {
                value: 2.0,
                within: 1e-6,
            },
            sigma: SigmaBounds {
                above: Some(1.4816),
                below: Some(1.4836),
            },
            mean: Some(Approx {
                value: 7.0 / 3.0,
                within: 1e-6,
            }),
        },
        ClipCase {
            name: "values straddling zero",
            values: vec![-10.0, -5.0, 0.0, 5.0, 10.0],
            kappa: 3.0,
            iterations: 3,
            median: Approx {
                value: 0.0,
                within: 0.1,
            },
            sigma: SigmaBounds {
                above: Some(0.0),
                below: None,
            },
            mean: None,
        },
        ClipCase {
            name: "outliers on both sides of a flat core",
            values: with(
                with(flat(90, 100.0), &[0.0, 1.0, 2.0, 198.0, 199.0, 200.0]),
                &[99.0, 100.0, 101.0, 102.0],
            ),
            kappa: 3.0,
            iterations: 3,
            median: Approx {
                value: 100.0,
                within: 2.0,
            },
            sigma: NO_SIGMA,
            mean: None,
        },
        // Zero iterations computes the statistics without clipping, so the outlier still counts.
        ClipCase {
            name: "zero iterations does not clip",
            values: vec![1.0, 2.0, 3.0, 1000.0],
            kappa: 3.0,
            iterations: 0,
            median: Approx {
                value: 2.5,
                within: 0.1,
            },
            sigma: NO_SIGMA,
            mean: None,
        },
        ClipCase {
            name: "zero iterations on a bimodal sample",
            values: vec![0.2, 0.2, 0.2, 0.9, 0.9],
            kappa: 3.0,
            iterations: 0,
            median: Approx {
                value: 0.2,
                within: 1e-6,
            },
            sigma: NO_SIGMA,
            mean: None,
        },
        ClipCase {
            name: "one iteration is enough for a single outlier",
            values: with(flat(10, 10.0), &[10000.0]),
            kappa: 3.0,
            iterations: 1,
            median: Approx {
                value: 10.0,
                within: 0.1,
            },
            sigma: SigmaBounds {
                above: None,
                below: Some(1.0),
            },
            mean: None,
        },
        ClipCase {
            name: "ten thousand values with one percent contaminated",
            values: {
                let mut values: Vec<f32> = (0..10000).map(|i| 100.0 + (i % 10) as f32).collect();
                for i in 0..100 {
                    values[i * 100] = 1000.0;
                }
                values
            },
            kappa: 3.0,
            iterations: 3,
            median: Approx {
                value: 105.0,
                within: 5.0,
            },
            sigma: SigmaBounds {
                above: Some(0.0),
                below: Some(20.0),
            },
            mean: None,
        },
        ClipCase {
            name: "one different among a thousand",
            values: with(flat(999, 42.0), &[9999.0]),
            kappa: 3.0,
            iterations: 3,
            median: Approx {
                value: 42.0,
                within: 0.01,
            },
            sigma: SigmaBounds {
                above: None,
                below: Some(0.01),
            },
            mean: None,
        },
        // Regression guard for the index mismatch where `select_nth_unstable_by` on the deviations
        // buffer broke its correspondence with the values buffer: with outliers on one side only,
        // that bug clipped the wrong values.
        ClipCase {
            name: "outliers on the high side only",
            values: with(flat(50, 100.0), &[500.0, 600.0, 700.0, 800.0, 900.0]),
            kappa: 2.5,
            iterations: 5,
            median: Approx {
                value: 100.0,
                within: 1.0,
            },
            sigma: SigmaBounds {
                above: None,
                below: Some(5.0),
            },
            mean: None,
        },
        ClipCase {
            name: "narrow spread near a half",
            values: spread(100, 0.5, 0.001),
            kappa: 3.0,
            iterations: 3,
            median: Approx {
                value: 0.5,
                within: 0.01,
            },
            sigma: SigmaBounds {
                above: Some(0.0),
                below: Some(0.1),
            },
            mean: None,
        },
        ClipCase {
            name: "high outliers",
            values: with(flat(90, 0.2), &[0.9; 10]),
            kappa: 3.0,
            iterations: 3,
            median: Approx {
                value: 0.2,
                within: 0.05,
            },
            sigma: NO_SIGMA,
            mean: None,
        },
        ClipCase {
            name: "low outliers",
            values: with(flat(90, 0.8), &[0.1; 10]),
            kappa: 3.0,
            iterations: 3,
            median: Approx {
                value: 0.8,
                within: 0.05,
            },
            sigma: NO_SIGMA,
            mean: None,
        },
        ClipCase {
            name: "both tails",
            values: with(with(flat(80, 0.5), &[0.05; 10]), &[0.95; 10]),
            kappa: 3.0,
            iterations: 3,
            median: Approx {
                value: 0.5,
                within: 0.05,
            },
            sigma: NO_SIGMA,
            mean: None,
        },
        // 101 values evenly spaced 0.4..0.6 in steps of 0.002 about a median of 0.5. Each absolute
        // deviation from 0.000 to 0.100 appears twice except 0.000, so the middle one is 0.050 and
        // sigma = 0.050 · 1.4826 = 0.0741. A high kappa keeps every value, isolating the
        // MAD-to-sigma conversion from any clipping.
        ClipCase {
            name: "mad to sigma conversion",
            values: (-50..=50).map(|i| 0.5 + i as f32 * 0.002).collect(),
            kappa: 10.0,
            iterations: 1,
            median: Approx {
                value: 0.5,
                within: 0.01,
            },
            sigma: SigmaBounds {
                above: Some(0.05 * 1.4826 - 0.002),
                below: Some(0.05 * 1.4826 + 0.002),
            },
            mean: None,
        },
        ClipCase {
            name: "one extreme outlier",
            values: with(flat(99, 0.5), &[100.0]),
            kappa: 3.0,
            iterations: 3,
            median: Approx {
                value: 0.5,
                within: 0.01,
            },
            sigma: NO_SIGMA,
            mean: None,
        },
        ClipCase {
            name: "negative core with positive outliers",
            values: with(flat(90, -0.5), &[0.5; 10]),
            kappa: 3.0,
            iterations: 3,
            median: Approx {
                value: -0.5,
                within: 0.05,
            },
            sigma: NO_SIGMA,
            mean: None,
        },
        ClipCase {
            name: "all same except one",
            values: with(flat(99, 0.4), &[0.9]),
            kappa: 3.0,
            iterations: 3,
            median: Approx {
                value: 0.4,
                within: 1e-6,
            },
            sigma: NO_SIGMA,
            mean: None,
        },
    ];

    let mut deviations = Vec::new();
    for case in cases {
        let ClipCase {
            name,
            mut values,
            kappa,
            iterations,
            median,
            sigma,
            mean,
        } = case;
        let count = values.len();
        deviations.clear();
        let stats = ClippedStats::sigma_clipped(&mut values, &mut deviations, kappa, iterations);

        // The caller keeps its sample: clipping selects, it does not truncate.
        assert_eq!(
            values.len(),
            count,
            "{name}: the input slice must keep its length"
        );
        assert!(
            (stats.median - median.value).abs() <= median.within,
            "{name}: median {} should be {} +- {}",
            stats.median,
            median.value,
            median.within
        );
        if let Some(floor) = sigma.above {
            assert!(
                stats.sigma > floor,
                "{name}: sigma {} should exceed {floor}",
                stats.sigma
            );
        }
        if let Some(ceiling) = sigma.below {
            assert!(
                stats.sigma < ceiling,
                "{name}: sigma {} should be under {ceiling}",
                stats.sigma
            );
        }
        if let Some(expected) = mean {
            assert!(
                (stats.mean - expected.value).abs() <= expected.within,
                "{name}: mean {} should be {} +- {}",
                stats.mean,
                expected.value,
                expected.within
            );
        }
    }
}

/// A stricter kappa clips more, and lands closer to the true centre.
///
/// Two fixtures, because the two tests this replaces each built their own and asserted the same
/// property. The second pins exact medians: with 50 at 0.50, 30 at 0.54 and 20 at 0.80, the
/// approximate median is 0.54 and MAD 0.04, so sigma is 0.059. kappa 1.5 gives a threshold of
/// 0.089 and rejects the 0.80 group, converging on 0.50; kappa 5.0 gives 0.297, keeps them, and
/// stays at the biased 0.54.
#[test]
fn sigma_clipped_stricter_kappa_clips_harder() {
    let mut deviations = Vec::new();
    let clip = |values: &[f32], kappa: f32, deviations: &mut Vec<f32>| {
        let mut values = values.to_vec();
        deviations.clear();
        ClippedStats::sigma_clipped(&mut values, deviations, kappa, 3)
    };

    let wide = {
        let mut v = vec![50.0f32; 90];
        v.extend([20.0, 25.0, 75.0, 80.0, 0.0, 100.0]);
        v
    };
    let strict = clip(&wide, 1.5, &mut deviations);
    let loose = clip(&wide, 5.0, &mut deviations);
    assert!((strict.median - 50.0).abs() < 5.0);
    assert!((loose.median - 50.0).abs() < 5.0);
    assert!(
        strict.sigma <= loose.sigma,
        "strict sigma {} should not exceed loose {}",
        strict.sigma,
        loose.sigma
    );

    let biased = {
        let mut v = vec![0.50f32; 50];
        v.extend(vec![0.54; 30]);
        v.extend(vec![0.80; 20]);
        v
    };
    let strict = clip(&biased, 1.5, &mut deviations);
    let loose = clip(&biased, 5.0, &mut deviations);
    assert!(
        (strict.median - 0.5).abs() < 1e-6,
        "strict kappa should recover the true median 0.5, got {}",
        strict.median
    );
    assert!(
        (strict.median - 0.5).abs() < (loose.median - 0.5).abs(),
        "strict median {} should beat loose {}",
        strict.median,
        loose.median
    );
}

/// The deviations buffer is scratch the caller owns and reuses across calls of different sizes.
#[test]
fn sigma_clipped_reuses_the_deviations_buffer() {
    let mut deviations = Vec::with_capacity(100);
    let mut first = vec![1.0, 2.0, 3.0, 4.0, 5.0];
    ClippedStats::sigma_clipped(&mut first, &mut deviations, 3.0, 2);
    let after_first = deviations.capacity();
    assert!(after_first >= first.len(), "the buffer was not used");

    // A shorter sample must not shrink the allocation the longer one earned.
    let mut second = vec![10.0, 20.0, 30.0];
    ClippedStats::sigma_clipped(&mut second, &mut deviations, 3.0, 2);
    assert!(
        deviations.capacity() >= after_first,
        "capacity dropped from {after_first} to {}",
        deviations.capacity()
    );
}

#[derive(Debug)]
struct AbsoluteDeviationCase {
    values: &'static [f32],
    center: f32,
    expected: &'static [f32],
}

#[test]
fn absolute_deviation_truth_table() {
    let cases = [
        AbsoluteDeviationCase {
            values: &[1.0, 2.0, 3.0, 4.0, 5.0],
            center: 3.0,
            expected: &[2.0, 1.0, 0.0, 1.0, 2.0],
        },
        AbsoluteDeviationCase {
            values: &[-4.0, -2.0, 0.0, 2.0, 4.0],
            center: 0.0,
            expected: &[4.0, 2.0, 0.0, 2.0, 4.0],
        },
        AbsoluteDeviationCase {
            values: &[5.0],
            center: 3.0,
            expected: &[2.0],
        },
        AbsoluteDeviationCase {
            values: &[],
            center: 0.0,
            expected: &[],
        },
    ];

    for case in cases {
        let mut values = case.values.to_vec();
        abs_deviation_inplace(&mut values, case.center);
        assert_eq!(values, case.expected, "{case:?}");
    }
}

/// The single-precision factor is a cast of the canonical `f64` one, so a unit MAD returns that
/// cast exactly — and it must still be the value the `f32` paths have always multiplied by. The
/// literal is the guard: extend `MAD_TO_SIGMA`'s digits far enough to shift its nearest `f32` and
/// this fires instead of every `f32` statistic moving silently.
#[test]
fn mad_to_sigma_known_value() {
    assert_eq!(mad_to_sigma(1.0), MAD_TO_SIGMA as f32);
    assert_eq!(mad_to_sigma(1.0), 1.4826022f32);
}

#[test]
fn mad_with_scratch_single() {
    let values = [5.0f32];
    let mut scratch = Vec::new();
    let mad = mad_f32_with_scratch(&values, 5.0, &mut scratch);
    assert!(mad.abs() < f32::EPSILON);
}

#[test]
fn mad_with_scratch_two_elements() {
    let values = [2.0f32, 8.0];
    let mut scratch = Vec::new();
    // median of [2, 8] = 5, deviations = [3, 3], MAD = 3
    let mad = mad_f32_with_scratch(&values, 5.0, &mut scratch);
    assert!((mad - 3.0).abs() < 1e-6);
}

/// Stack scratch must give the same answer as heap scratch — the property the separate `ArrayVec`
/// entry point used to exist to provide, now carried by the two `DeviationScratch` impls.
#[test]
fn sigma_clipped_is_agnostic_to_where_the_scratch_lives() {
    let base: Vec<f32> = vec![1.0, 2.0, 3.0, 100.0, 4.0, 5.0, 6.0, 200.0];

    let mut heap_values = base.clone();
    let mut heap_scratch: Vec<f32> = Vec::new();
    let heap = ClippedStats::sigma_clipped(&mut heap_values, &mut heap_scratch, 3.0, 3);

    let mut stack_values = base.clone();
    let mut stack_scratch: arrayvec::ArrayVec<f32, 16> = arrayvec::ArrayVec::new();
    let stack = ClippedStats::sigma_clipped(&mut stack_values, &mut stack_scratch, 3.0, 3);

    assert_eq!(heap.median, stack.median);
    assert_eq!(heap.sigma, stack.sigma);
    assert_eq!(heap.mean, stack.mean);
}

/// A heap scratch grows to fit; a fixed one cannot, and says so while sizing rather than
/// misbehaving deeper in the clip.
#[test]
#[should_panic(expected = "capacity")]
fn sigma_clipped_stack_scratch_too_small_panics() {
    let mut values = vec![1.0f32, 2.0, 3.0, 4.0, 5.0];
    let mut deviations: arrayvec::ArrayVec<f32, 4> = arrayvec::ArrayVec::new();
    let _ = ClippedStats::sigma_clipped(&mut values, &mut deviations, 3.0, 2);
}

#[test]
fn median_f32_fast_truth_table() {
    let cases = [
        MedianCase {
            values: &[5.0, 2.0, 8.0, 1.0, 3.0],
            expected: 3.0,
        },
        MedianCase {
            values: &[5.0, 2.0, 8.0, 1.0],
            expected: 5.0,
        },
        MedianCase {
            values: &[42.0],
            expected: 42.0,
        },
        MedianCase {
            values: &[7.0, 3.0],
            expected: 7.0,
        },
        MedianCase {
            values: &[5.0; 20],
            expected: 5.0,
        },
        MedianCase {
            values: &[3.0, -5.0, 7.0, -10.0, -2.0],
            expected: -2.0,
        },
    ];

    for case in cases {
        let mut values = case.values.to_vec();
        let actual = median_f32_fast(&mut values);
        assert!((actual - case.expected).abs() < f32::EPSILON, "{case:?}");
    }
}

#[test]
fn median_f32_fast_differs_from_exact_on_even() {
    // Sorted: [1, 3, 7, 9], mid=2
    // Exact: (3+7)/2 = 5.0
    // Fast: values[2] = 7.0
    let mut values_fast = [9.0f32, 1.0, 7.0, 3.0];
    let mut values_exact = values_fast;
    let fast = median_f32_fast(&mut values_fast);
    let exact = median_f32_mut(&mut values_exact);
    assert_eq!(exact, 5.0);
    assert_eq!(fast, 7.0);
    assert!(
        (fast - exact).abs() > 1.0,
        "fast and exact should differ for even N"
    );
}

#[test]
fn median_f32_fast_agrees_with_exact_on_odd() {
    // For odd N, both return the same middle element
    // Sorted: [2, 4, 6, 8, 10], mid=2, median=6
    let mut values_fast = [10.0f32, 4.0, 6.0, 2.0, 8.0];
    let mut values_exact = values_fast;
    let fast = median_f32_fast(&mut values_fast);
    let exact = median_f32_mut(&mut values_exact);
    assert!((fast - exact).abs() < f32::EPSILON);
    assert_eq!(fast, 6.0);
}

#[test]
fn mad_f32_fast_hand_computed() {
    // values = [2, 3, 4], median = 3
    // deviations = |2-3|, |3-3|, |4-3| = [1, 0, 1]
    // sorted deviations: [0, 1, 1], mid=1, MAD = 1
    let values = [2.0f32, 3.0, 4.0];
    let mut scratch = Vec::new();
    let mad = mad_f32_fast(&values, 3.0, &mut scratch);
    assert_eq!(mad, 1.0);
}

#[test]
fn mad_f32_fast_five_values() {
    // values = [1, 2, 3, 4, 5], median = 3
    // deviations = [2, 1, 0, 1, 2]
    // sorted deviations: [0, 1, 1, 2, 2], mid=2, MAD = 1
    let values = [1.0f32, 2.0, 3.0, 4.0, 5.0];
    let mut scratch = Vec::new();
    let mad = mad_f32_fast(&values, 3.0, &mut scratch);
    assert_eq!(mad, 1.0);
}

#[test]
fn mad_f32_fast_uniform() {
    // All same → all deviations = 0 → MAD = 0
    let values = [7.0f32; 10];
    let mut scratch = Vec::new();
    let mad = mad_f32_fast(&values, 7.0, &mut scratch);
    assert!(mad.abs() < f32::EPSILON);
}

#[test]
fn mad_f32_fast_empty() {
    let values: [f32; 0] = [];
    let mut scratch = Vec::new();
    let mad = mad_f32_fast(&values, 0.0, &mut scratch);
    assert!(mad.abs() < f32::EPSILON);
}

#[test]
fn mad_f32_fast_single() {
    // Single value: deviation = 0, MAD = 0
    let values = [5.0f32];
    let mut scratch = Vec::new();
    let mad = mad_f32_fast(&values, 5.0, &mut scratch);
    assert!(mad.abs() < f32::EPSILON);
}

#[test]
fn mad_f32_fast_scratch_reused() {
    // Verify scratch buffer is reused (capacity preserved across calls)
    let mut scratch = Vec::new();

    let values1 = [1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
    mad_f32_fast(&values1, 5.5, &mut scratch);
    let cap = scratch.capacity();
    assert!(cap >= 10);

    let values2 = [1.0f32, 2.0, 3.0];
    mad_f32_fast(&values2, 2.0, &mut scratch);
    assert!(scratch.capacity() >= cap, "capacity should not shrink");
}

#[test]
fn mad_f32_fast_matches_regular_on_odd() {
    // For odd N, median_f32_fast and median_f32_mut agree,
    // so mad_f32_fast should match mad_f32_with_scratch exactly.
    let values = [10.0f32, 2.0, 7.0, 15.0, 3.0];
    let median = 7.0; // sorted: [2, 3, 7, 10, 15], mid=2
    let mut scratch1 = Vec::new();
    let mut scratch2 = Vec::new();
    let mad_fast = mad_f32_fast(&values, median, &mut scratch1);
    let mad_regular = mad_f32_with_scratch(&values, median, &mut scratch2);
    // deviations = |10-7|, |2-7|, |7-7|, |15-7|, |3-7| = [3, 5, 0, 8, 4]
    // sorted = [0, 3, 4, 5, 8], mid=2 → MAD = 4
    assert!(
        (mad_fast - mad_regular).abs() < f32::EPSILON,
        "fast={mad_fast}, regular={mad_regular}"
    );
    assert_eq!(mad_fast, 4.0);
}

#[test]
fn sigma_clipped_stats_iterations_improve_result() {
    // Good values: 41 at 0.30, 40 at 0.32 (true median = 0.30, odd count = 81)
    // Outliers: 10 at 0.60, 9 at 1.50
    //
    // Approx median of all 100 = 0.32 (value[50]).
    // MAD = 0.02 (devs: 41×0.02, 40×0.00, 10×0.28, 9×1.18, index 50 = 0.02).
    // sigma = 0.02 * 1.4826 = 0.0297.
    //
    // 0 iterations (no clipping): compute_final_stats on 100 values.
    //   median_f32_mut(100): avg(values[50], max(values[0..50])) = avg(0.32, 0.32) = 0.32.
    //
    // 3 iterations (with clipping):
    //   Iter 1: kappa=2.5, threshold = 0.074. Rejects 0.60 and 1.50 → 81 remain.
    //   Iter 2: 81 values (odd). approx median = value[40] = 0.30.
    //     MAD = 0.00, sigma = 0 → converge at 0.30.
    let base_values: Vec<f32> = {
        let mut v = vec![0.30; 41];
        v.extend(vec![0.32; 40]);
        v.extend(vec![0.60; 10]);
        v.extend(vec![1.50; 9]);
        v
    };

    let mut values_0iter = base_values.clone();
    let mut values_3iter = base_values.clone();
    let mut deviations: Vec<f32> = vec![];

    let ClippedStats {
        median: median_0iter,
        ..
    } = ClippedStats::sigma_clipped(&mut values_0iter, &mut deviations, 2.5, 0);
    deviations.clear();
    let ClippedStats {
        median: median_3iter,
        ..
    } = ClippedStats::sigma_clipped(&mut values_3iter, &mut deviations, 2.5, 3);

    // 0 iterations: no clipping, median biased to 0.32 by outlier presence
    assert!(
        (median_0iter - 0.32).abs() < 1e-6,
        "0 iterations should give 0.32, got {}",
        median_0iter
    );
    // 3 iterations: clipping removes outliers, converges to true median 0.30
    assert!(
        (median_3iter - 0.30).abs() < 1e-6,
        "3 iterations should recover true median 0.30, got {}",
        median_3iter
    );
    // Clipping brings result closer to true center
    assert!(
        (median_3iter - 0.30).abs() < (median_0iter - 0.30).abs(),
        "3 iterations median {} should be closer to 0.30 than 0 iterations {}",
        median_3iter,
        median_0iter
    );
}

#[test]
fn mad_floored_raises_only_a_spread_below_the_floor() {
    // Floor active: a spread below the floor is raised to floor_fraction * center.
    assert_eq!(mad_floored(0.1, 10.0, 0.5), 5.0);
    // Floor inactive: a real spread above the floor passes through unchanged.
    assert_eq!(mad_floored(8.0, 10.0, 0.5), 8.0);
    // Exactly at the floor.
    assert_eq!(mad_floored(5.0, 10.0, 0.5), 5.0);
}

#[derive(Debug)]
struct MedianF64Case {
    values: &'static [f64],
    expected: f64,
}

/// Same truth table as [`median_f32_truth_table`]: the `f64` median must average the two middles
/// on even length rather than pick a side, which is what the quickselect form has to reproduce
/// now that it replaced a full sort.
#[test]
fn median_f64_truth_table() {
    let cases = [
        MedianF64Case {
            values: &[1.0, 3.0, 2.0, 5.0, 4.0],
            expected: 3.0,
        },
        MedianF64Case {
            values: &[1.0, 2.0, 3.0, 4.0],
            expected: 2.5,
        },
        MedianF64Case {
            values: &[1.0, 5.0],
            expected: 3.0,
        },
        MedianF64Case {
            values: &[42.0],
            expected: 42.0,
        },
        MedianF64Case {
            values: &[-5.0, -3.0, -1.0, 2.0, 4.0],
            expected: -1.0,
        },
    ];

    for case in cases {
        let mut values = case.values.to_vec();
        assert_eq!(median_f64_mut(&mut values), case.expected, "{case:?}");
    }
}

/// The upper-middle convention, and the equivalence a caller trades a full sort for.
#[test]
fn median_f64_fast_takes_the_upper_middle_of_a_sorted_run() {
    // Sorted: [1, 3, 7, 9]. Index len/2 = 2 holds 7.0, where averaging gives (3 + 7)/2 = 5.0.
    assert_eq!(median_f64_fast(&mut [9.0f64, 1.0, 7.0, 3.0]), 7.0);
    assert_eq!(median_f64_mut(&mut [9.0f64, 1.0, 7.0, 3.0]), 5.0);

    // What SIP's clip relies on: whatever a full sort leaves at `len / 2`, one selection returns
    // bit-identically, at both parities and with duplicates present.
    for len in 1..40usize {
        let data: Vec<f64> = (0..len)
            .map(|i| (i * 37 % len) as f64 * 0.1 - 1.5)
            .collect();
        let mut sorted = data.clone();
        sorted.sort_unstable_by(f64::total_cmp);
        let mut fast = data.clone();
        assert_eq!(median_f64_fast(&mut fast), sorted[len / 2], "len = {len}");
    }
}

#[test]
fn mad_f64_fast_hand_computed() {
    let mut scratch = Vec::new();
    // |r − 3| over [1, 2, 3, 4, 100] = [2, 1, 0, 1, 97]; sorted [0, 1, 1, 2, 97], index 2 = 1.
    let data = [1.0f64, 2.0, 3.0, 4.0, 100.0];
    assert_eq!(mad_f64_fast(&data, 3.0, &mut scratch), 1.0);
    // Even count: |r − 3| over [1, 2, 4, 100] = [2, 1, 1, 97]; sorted [1, 1, 2, 97], index 2 = 2.
    assert_eq!(
        mad_f64_fast(&[1.0, 2.0, 4.0, 100.0], 3.0, &mut scratch),
        2.0
    );
    // A shorter call after a longer one measures only its own deviations: |r − 20| = [10, 0, 10],
    // sorted [0, 10, 10], index 1 = 10. Stale scratch entries would push the rank off.
    assert_eq!(mad_f64_fast(&[10.0, 20.0, 30.0], 20.0, &mut scratch), 10.0);
    assert_eq!(mad_f64_fast(&[], 0.0, &mut scratch), 0.0);
}

/// The `f64` fast path carries the same contract as its `f32` twin — see
/// [`sigma_clip_rejects_nan_input`] for why a NaN is a contract violation rather than a case.
#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "NaN-free")]
fn median_f64_fast_rejects_nan_input() {
    median_f64_fast(&mut [1.0f64, f64::NAN, 3.0]);
}

#[test]
fn robust_sigma_f64_scales_the_mad_and_leaves_its_input_alone() {
    // median([1, 2, 3, 4, 100]) = 3; |r − 3| = [2, 1, 0, 1, 97]; median of those = 1.
    // So sigma = 1.4826022 × 1.
    let data = [1.0f64, 2.0, 3.0, 4.0, 100.0];
    let mut scratch = Vec::new();
    let sigma = robust_sigma_f64(&data, &mut scratch);
    // Exactly the constant, not merely near it: the MAD is 1.0, and the factor is `f64` end to end
    // now rather than an `f32` widened back up.
    assert_eq!(sigma, MAD_TO_SIGMA, "sigma = {sigma}");
    assert_eq!(data, [1.0, 2.0, 3.0, 4.0, 100.0], "input must be intact");

    // Doubling every deviation doubles sigma — proves the MAD is measured, not a constant.
    let spread = [1.0f64, 3.0, 5.0, 7.0, 199.0];
    let wide = robust_sigma_f64(&spread, &mut scratch);
    assert!((wide - 2.0 * sigma).abs() < 1e-12, "wide = {wide}");

    // A constant sample has zero spread, and an empty one has nothing to measure.
    assert_eq!(robust_sigma_f64(&[7.0; 9], &mut scratch), 0.0);
    assert_eq!(robust_sigma_f64(&[], &mut scratch), 0.0);
}

/// The χ² quantile is a distribution fact, not a tuning knob, so it is checked against the closed
/// form rather than against a copy of itself: for k = 2 the CDF is `1 − exp(−x/2)`, which makes the
/// p-quantile `−2·ln(1 − p)`.
#[test]
fn chi2_99_2dof_is_the_one_percent_tail_of_the_two_dof_distribution() {
    assert!((CHI2_99_2DOF - (-2.0 * 0.01_f64.ln())).abs() < 1e-12);

    // Round trip through the CDF: exactly 1% of the distribution lies beyond it.
    let tail = (-CHI2_99_2DOF / 2.0).exp();
    assert!((tail - 0.01).abs() < 1e-12, "tail mass {tail} is not 1%");

    // The rounded 9.21 this replaced is inside a ten-thousandth, which is why the two copies of it
    // could disagree for so long without any test noticing.
    assert!((CHI2_99_2DOF - 9.21).abs() < 1e-3);
}
