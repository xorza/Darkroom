//! Tests for statistical functions.

use std::f64::consts::{PI, SQRT_2};

use crate::internals::prelude::*;
use crate::math::statistics::*;

#[derive(Debug)]
struct MedianCase {
    values: &'static [f32],
    expected: f32,
}

/// The inputs too small or too flat for clipping to do anything, with the result hand-derived in
/// each case.
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

/// The median averages the two middles on even length rather than picking a side, as a sorted
/// median does.
///
/// One table at both widths, because [`median_mut`] is one implementation. Every literal is a
/// small dyadic rational, so both precisions must land on the expectation exactly.
#[test]
fn median_truth_table_holds_at_both_widths() {
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
        let mut single = case.values.to_vec();
        assert_eq!(median_mut(&mut single), case.expected, "f32 {case:?}");

        let mut double: Vec<f64> = case.values.iter().copied().map(f64::from).collect();
        assert_eq!(
            median_mut(&mut double),
            f64::from(case.expected),
            "f64 {case:?}"
        );
    }
}

/// `MedianMad::of_mut` on [2, 4, 3] — median 3, deviations [1, 1, 0], MAD 1, σ the MAD rescaled —
/// and on a flat run, where every deviation and so σ is 0.
#[test]
fn median_and_mad_of_a_sample() {
    let stats = MedianMad::of_mut(&mut [2.0f32, 4.0, 3.0]);
    assert_eq!((stats.median, stats.mad), (3.0, 1.0));
    assert_eq!(stats.sigma(), mad_to_sigma(1.0));

    let flat = MedianMad::of_mut(&mut [3.5f32; 5]);
    assert_eq!((flat.median, flat.mad, flat.sigma()), (3.5, 0.0, 0.0));
}

/// [`mad_with_scratch`] over the lengths that bound it: the ordinary odd case, both degenerate
/// lengths, the empty answer, and an even case whose two middles differ.
#[test]
fn mad_with_scratch_over_every_length() {
    let cases: [(&[f32], f32, f32); 5] = [
        // |r - 3| over [2, 4, 3] = [1, 1, 0]; median of those = 1.
        (&[2.0, 4.0, 3.0], 3.0, 1.0),
        // Nothing to deviate from.
        (&[], 0.0, 0.0),
        // |r - 5| over [5] = [0].
        (&[5.0], 5.0, 0.0),
        // The shortest even input: |r - 5| over [2, 8] = [3, 3].
        (&[2.0, 8.0], 5.0, 3.0),
        // Even length averages the two middles rather than taking a side: |r - 5| over
        // [1, 4, 6, 9] = [4, 1, 1, 4], ranked [1, 1, 4, 4], so (1 + 4) / 2 = 2.5 and the
        // upper-middle convention would have answered 4.
        (&[1.0, 4.0, 6.0, 9.0], 5.0, 2.5),
    ];

    let mut scratch = Vec::new();
    for (values, median, expected) in cases {
        assert_eq!(
            mad_with_scratch(values, median, &mut scratch),
            expected,
            "{values:?} about {median}"
        );
    }
}

/// `median_mut` orders by `total_cmp`, where NaN sorts after every number: [1, 2, 3, 5, NaN] has
/// the median 3.
#[test]
fn median_mut_orders_nan_last() {
    assert_eq!(median_mut(&mut [1.0f32, f32::NAN, 3.0, 2.0, 5.0]), 3.0);
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

/// Statistics a case's hand derivation gives, where it gives them.
#[derive(Debug, Clone, Copy)]
struct Anchor {
    median: f32,
    sigma: f32,
    mean: f32,
}

/// One `ClippedStats::sigma_clipped` run.
#[derive(Debug)]
struct ClipCase {
    name: &'static str,
    values: Vec<f32>,
    kappa: f32,
    anchor: Option<Anchor>,
    iterations: usize,
}

/// The median by sorting: the middle value, or the midpoint of the two middles — the
/// convention both selection medians keep.
fn sorted_median(values: &[f32]) -> f32 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f32::total_cmp);
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        sorted[mid]
    } else {
        f32::midpoint(sorted[mid - 1], sorted[mid])
    }
}

/// `ClippedStats::sigma_clipped` as its documentation states it, by sorting and filtering rather
/// than by selecting and partitioning in place: up to `iterations` passes, each taking the median,
/// the MAD about it and σ = `mad_to_sigma(MAD)`, stopping when σ is below one ulp of the median
/// (reported as no spread) or when nothing lies past κσ, and otherwise keeping only what does not;
/// a pass needs three values. The mean is of the survivors, accumulated in f64.
fn reference_clip(values: &[f32], kappa: f32, iterations: usize) -> Anchor {
    let mut active = values.to_vec();
    let mut converged = None;
    for _ in 0..iterations {
        if active.len() < 3 {
            break;
        }
        let median = sorted_median(&active);
        let deviations: Vec<f32> = active.iter().map(|&v| (v - median).abs()).collect();
        let mad = sorted_median(&deviations);
        let sigma = mad_to_sigma(mad);
        if sigma <= median.abs() * f32::EPSILON {
            converged = Some((median, 0.0));
            break;
        }
        let kept: Vec<f32> = active
            .iter()
            .copied()
            .filter(|&v| (v - median).abs() <= kappa * sigma)
            .collect();
        if kept.len() == active.len() {
            converged = Some((median, mad));
            break;
        }
        active = kept;
    }
    let (median, mad) = converged.unwrap_or_else(|| {
        let median = sorted_median(&active);
        let deviations: Vec<f32> = active.iter().map(|&v| (v - median).abs()).collect();
        (median, sorted_median(&deviations))
    });
    let sum: f64 = active.iter().map(|&v| f64::from(v)).sum();
    Anchor {
        median,
        sigma: mad_to_sigma(mad),
        mean: (sum / active.len() as f64) as f32,
    }
}

/// `sigma_clipped` over every sample shape that mattered, held exactly to [`reference_clip`] —
/// the same answer reached without its selection, partitioning or buffer reuse, where its bugs
/// have lived — and, where a case derives them by hand, to those values too. Every case also
/// keeps the caller's slice at its length: clipping selects, it does not truncate.
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
    let cases = vec![
        ClipCase {
            name: "smooth spread with nothing to clip",
            values: spread(100, 50.0, 0.1),
            kappa: 3.0,
            anchor: None,
            iterations: 3,
        },
        // 97 identical values give MAD = 0, so the clip exits early (σ ≈ 0) *without* removing the
        // outliers and the mean covers the full contaminated sample: (97·10 + 6000)/100 = 69.7.
        // Consumers must treat `mean` as meaningful only alongside σ > 0 — the SExtractor sky
        // estimator's σ-gated crowding test does exactly that.
        ClipCase {
            name: "three huge outliers against a flat sample",
            values: with(flat(97, 10.0), &[1000.0, 2000.0, 3000.0]),
            kappa: 3.0,
            anchor: Some(Anchor {
                median: 10.0,
                sigma: 0.0,
                mean: 69.7,
            }),
            iterations: 3,
        },
        // 100 is clipped in iteration 1 under any fast-median convention (threshold <= 13.3 while
        // |100 - median| >= 96). Survivors [1, 2, 4]: median 2, mean 7/3, MAD 1 so sigma = 1.4826.
        ClipCase {
            name: "asymmetric survivors",
            values: vec![1.0, 2.0, 4.0, 100.0],
            kappa: 3.0,
            anchor: Some(Anchor {
                median: 2.0,
                sigma: MAD_TO_SIGMA as f32,
                mean: 7.0 / 3.0,
            }),
            iterations: 3,
        },
        ClipCase {
            name: "values straddling zero",
            values: vec![-10.0, -5.0, 0.0, 5.0, 10.0],
            kappa: 3.0,
            anchor: None,
            iterations: 3,
        },
        ClipCase {
            name: "outliers on both sides of a flat core",
            values: with(
                with(flat(90, 100.0), &[0.0, 1.0, 2.0, 198.0, 199.0, 200.0]),
                &[99.0, 100.0, 101.0, 102.0],
            ),
            kappa: 3.0,
            anchor: None,
            iterations: 3,
        },
        // Zero iterations computes the statistics without clipping, so the outlier still counts.
        ClipCase {
            name: "zero iterations does not clip",
            values: vec![1.0, 2.0, 3.0, 1000.0],
            kappa: 3.0,
            anchor: None,
            iterations: 0,
        },
        ClipCase {
            name: "zero iterations on a bimodal sample",
            values: vec![0.2, 0.2, 0.2, 0.9, 0.9],
            kappa: 3.0,
            anchor: None,
            iterations: 0,
        },
        ClipCase {
            name: "one iteration is enough for a single outlier",
            values: with(flat(10, 10.0), &[10000.0]),
            kappa: 3.0,
            anchor: None,
            iterations: 1,
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
            anchor: None,
            iterations: 3,
        },
        ClipCase {
            name: "one different among a thousand",
            values: with(flat(999, 42.0), &[9999.0]),
            kappa: 3.0,
            anchor: None,
            iterations: 3,
        },
        // Outliers on one side only: selecting on the deviations must not lose which value each
        // deviation belongs to, or the clip removes the wrong values.
        ClipCase {
            name: "outliers on the high side only",
            values: with(flat(50, 100.0), &[500.0, 600.0, 700.0, 800.0, 900.0]),
            kappa: 2.5,
            anchor: None,
            iterations: 5,
        },
        ClipCase {
            name: "narrow spread near a half",
            values: spread(100, 0.5, 0.001),
            kappa: 3.0,
            anchor: None,
            iterations: 3,
        },
        ClipCase {
            name: "high outliers",
            values: with(flat(90, 0.2), &[0.9; 10]),
            kappa: 3.0,
            anchor: None,
            iterations: 3,
        },
        ClipCase {
            name: "low outliers",
            values: with(flat(90, 0.8), &[0.1; 10]),
            kappa: 3.0,
            anchor: None,
            iterations: 3,
        },
        ClipCase {
            name: "both tails",
            values: with(with(flat(80, 0.5), &[0.05; 10]), &[0.95; 10]),
            kappa: 3.0,
            anchor: None,
            iterations: 3,
        },
        // 101 values evenly spaced 0.4..0.6 in steps of 0.002 about a median of 0.5. Each absolute
        // deviation from 0.000 to 0.100 appears twice except 0.000, so the middle one is 0.050 and
        // sigma = 0.050 · 1.4826 = 0.0741. A high kappa keeps every value, isolating the
        // MAD-to-sigma conversion from any clipping.
        ClipCase {
            name: "mad to sigma conversion",
            values: (-50..=50).map(|i| 0.5 + i as f32 * 0.002).collect(),
            kappa: 10.0,
            anchor: None,
            iterations: 1,
        },
        ClipCase {
            name: "one extreme outlier",
            values: with(flat(99, 0.5), &[100.0]),
            kappa: 3.0,
            anchor: None,
            iterations: 3,
        },
        ClipCase {
            name: "negative core with positive outliers",
            values: with(flat(90, -0.5), &[0.5; 10]),
            kappa: 3.0,
            anchor: None,
            iterations: 3,
        },
        ClipCase {
            name: "all same except one",
            values: with(flat(99, 0.4), &[0.9]),
            kappa: 3.0,
            anchor: None,
            iterations: 3,
        },
    ];

    let mut deviations = Vec::new();
    for case in cases {
        let ClipCase {
            name,
            mut values,
            kappa,
            anchor,
            iterations,
        } = case;
        let expected = reference_clip(&values, kappa, iterations);
        let count = values.len();
        deviations.clear();
        let stats = ClippedStats::sigma_clipped(&mut values, &mut deviations, kappa, iterations);
        assert_eq!(values.len(), count, "{name}: the slice keeps its length");
        assert_eq!(
            (stats.median, stats.sigma, stats.mean),
            (expected.median, expected.sigma, expected.mean),
            "{name}"
        );
        if let Some(anchor) = anchor {
            assert_eq!(
                (stats.median, stats.sigma),
                (anchor.median, anchor.sigma),
                "{name}: by hand"
            );
            assert!(
                (stats.mean - anchor.mean).abs() <= f32::EPSILON * anchor.mean,
                "{name}: mean {} by hand {}",
                stats.mean,
                anchor.mean
            );
        }
    }
}

/// Every exit reports the same median: a run that converges and one that never iterates both give
/// (2 + 3)/2 for [1, 2, 3, 4], not the upper middle 3.
#[test]
fn sigma_clipped_reports_one_median_on_every_exit() {
    let mut deviations = Vec::new();
    for iterations in [0, 3] {
        let mut values = vec![1.0f32, 2.0, 3.0, 4.0];
        let stats = ClippedStats::sigma_clipped(&mut values, &mut deviations, 3.0, iterations);
        assert_eq!(stats.median, 2.5, "{iterations} iterations");
    }
}

/// A stricter kappa clips an outlier a looser one keeps, so the two land on different medians.
#[test]
fn sigma_clipped_stricter_kappa_clips_harder() {
    let mut deviations = Vec::new();
    let clip = |values: &[f32], kappa: f32, deviations: &mut Vec<f32>| {
        let mut values = values.to_vec();
        deviations.clear();
        ClippedStats::sigma_clipped(&mut values, deviations, kappa, 3)
    };

    // 0..39 plus one outlier at 60. Both runs start at median 20: the deviations |x − 20| are
    // 0, then 1..19 twice, then 20 and 40, so rank 20 of the 41 is 10 and σ = 1.4826·10 = 14.83.
    // κ = 1.5 gives 22.2, clipping the 60; the 40 survivors' deviations |x − 19.5| are 0.5..19.5
    // twice, so ranks 19 and 20 are 9.5 and 10.5 and the MAD stays 10 — nothing more clips, and
    // the median is 19.5. κ = 5 gives 74, keeping the 60: the median stays 20.
    let values: Vec<f32> = (0..40).map(|v| v as f32).chain([60.0]).collect();
    let strict = clip(&values, 1.5, &mut deviations);
    let loose = clip(&values, 5.0, &mut deviations);
    assert_eq!(strict.median, 19.5);
    assert_eq!(loose.median, 20.0);
    assert_eq!(strict.sigma, mad_to_sigma(10.0f32));
    assert_eq!(loose.sigma, mad_to_sigma(10.0f32));
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

/// Stack scratch gives the same answer as heap scratch: the two `DeviationScratch` impls.
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

/// `median_fast` and `median_mut` are one median: the middle element for an odd count, the mean of
/// the two middle ones for an even count. Sorted [1, 3, 7, 9] → (3 + 7)/2; sorted [2, 4, 6, 8, 10]
/// → 6.
#[test]
fn median_fast_matches_median_mut_at_both_parities() {
    for (values, expected) in [
        (vec![9.0f32, 1.0, 7.0, 3.0], 5.0),
        (vec![10.0, 4.0, 6.0, 2.0, 8.0], 6.0),
    ] {
        let fast = median_fast(&mut values.clone());
        let exact = median_mut(&mut values.clone());
        assert_eq!(fast, expected, "{values:?}");
        assert_eq!(exact, expected, "{values:?}");
    }
}

/// [`mad_fast`] over odd and even lengths, a uniform run, both degenerate lengths, and data whose
/// outlier the MAD is supposed to ignore.
///
/// One table at both widths, because [`mad_fast`] is one implementation. The cases run in order
/// against one scratch buffer, so a shorter call after a longer one also proves the buffer is
/// truncated rather than left holding the previous call's tail.
#[test]
fn mad_fast_truth_table_holds_at_both_widths() {
    let cases: [(&[f32], f32, f32); 8] = [
        // |r - 3| over [2, 3, 4] = [1, 0, 1]; ranked [0, 1, 1], index 1 = 1.
        (&[2.0, 3.0, 4.0], 3.0, 1.0),
        // |r - 3| over [1, 2, 3, 4, 5] = [2, 1, 0, 1, 2]; ranked [0, 1, 1, 2, 2], index 2 = 1.
        (&[1.0, 2.0, 3.0, 4.0, 5.0], 3.0, 1.0),
        // |r - 3| over [1, 2, 3, 4, 100] = [2, 1, 0, 1, 97]; ranked [0, 1, 1, 2, 97], index 2 = 1.
        (&[1.0, 2.0, 3.0, 4.0, 100.0], 3.0, 1.0),
        // Even count averages the two middles: [2, 1, 1, 97] ranked [1, 1, 2, 97] → (1 + 2)/2.
        (&[1.0, 2.0, 4.0, 100.0], 3.0, 1.5),
        // A short call after the long ones above measures only its own deviations:
        // |r - 20| = [10, 0, 10] ranked [0, 10, 10], index 1 = 10.
        (&[10.0, 20.0, 30.0], 20.0, 10.0),
        // Every deviation zero.
        (&[7.0; 10], 7.0, 0.0),
        (&[5.0], 5.0, 0.0),
        (&[], 0.0, 0.0),
    ];

    let mut single_scratch = Vec::new();
    let mut double_scratch = Vec::new();
    for (values, median, expected) in cases {
        assert_eq!(
            mad_fast(values, median, &mut single_scratch),
            expected,
            "f32 {values:?} about {median}"
        );

        let double: Vec<f64> = values.iter().copied().map(f64::from).collect();
        assert_eq!(
            mad_fast(&double, f64::from(median), &mut double_scratch),
            f64::from(expected),
            "f64 {values:?} about {median}"
        );
    }
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
    //   median_mut(100): avg(values[50], max(values[0..50])) = avg(0.32, 0.32) = 0.32.
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

    assert_eq!(
        median_0iter, 0.32,
        "unclipped, the outliers pull the median"
    );
    assert_eq!(median_3iter, 0.30, "clipped, it is the core's");
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

/// One selection gives the same median a full sort does, at both parities and with duplicates
/// present — what SIP's clip relies on when it trades the sort away.
#[test]
fn median_fast_equals_the_sorted_median() {
    for len in 1..40usize {
        let data: Vec<f64> = (0..len)
            .map(|i| (i * 37 % len) as f64 * 0.1 - 1.5)
            .collect();
        let mut sorted = data.clone();
        sorted.sort_unstable_by(f64::total_cmp);
        let expected = if len % 2 == 1 {
            sorted[len / 2]
        } else {
            f64::midpoint(sorted[len / 2 - 1], sorted[len / 2])
        };
        assert_eq!(median_fast(&mut data.clone()), expected, "len = {len}");
    }
}

/// The fast path's NaN contract, exercised at `f64` — see [`sigma_clip_rejects_nan_input`] for
/// why a NaN is a contract violation rather than a case.
#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "NaN-free")]
fn median_fast_rejects_nan_input() {
    median_fast(&mut [1.0f64, f64::NAN, 3.0]);
}

#[test]
fn robust_sigma_f64_scales_the_mad_and_leaves_its_input_alone() {
    // median([1, 2, 3, 4, 100]) = 3; |r − 3| = [2, 1, 0, 1, 97]; median of those = 1.
    // So sigma = 1.4826022 × 1.
    let data = [1.0f64, 2.0, 3.0, 4.0, 100.0];
    let mut scratch = Vec::new();
    let sigma = robust_sigma_f64(&data, &mut scratch);
    // Exactly the constant: the MAD is 1.0, and the factor is `f64` end to end.
    assert_eq!(sigma, MAD_TO_SIGMA, "sigma = {sigma}");
    assert_eq!(data, [1.0, 2.0, 3.0, 4.0, 100.0], "input must be intact");

    // Doubling every deviation doubles sigma — proves the MAD is measured, not a constant.
    let spread = [1.0f64, 3.0, 5.0, 7.0, 199.0];
    let wide = robust_sigma_f64(&spread, &mut scratch);
    assert_eq!(wide, 2.0 * sigma);

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
}

/// `MAD_TO_SIGMA` is `1 / Φ⁻¹(3/4)`: the standard normal CDF at its reciprocal is 3/4. Φ is `½·(1 +
/// erf(x/√2))` with erf summed from its Maclaurin series, whose terms at `x/√2` ≈ 0.477 fall below
/// 1e-17 within twenty; the sum then carries a few ulps of rounding, and 4·ε holds them.
#[test]
fn mad_to_sigma_is_the_reciprocal_normal_quartile() {
    let z = (1.0 / MAD_TO_SIGMA) / SQRT_2;
    let mut term = z;
    let mut erf_sum = 0.0;
    for n in 0..30u32 {
        erf_sum += term / f64::from(2 * n + 1);
        term *= -z * z / f64::from(n + 1);
    }
    let erf = 2.0 / PI.sqrt() * erf_sum;
    let cdf = f64::midpoint(1.0, erf);
    assert!((cdf - 0.75).abs() <= 4.0 * f64::EPSILON, "Φ = {cdf}");
    assert_eq!(MAD_TO_SIGMA as f32, 1.482_602_2_f32);
}
