use crate::math::noise::background_split::{BackgroundSplit, BinnedSamples, GainBin, GainBins};

fn bins(points: &[(f64, f64)]) -> Vec<GainBin> {
    points
        .iter()
        .map(|&(gain, variance)| GainBin {
            gain,
            variance,
            samples: 100,
        })
        .collect()
}

/// Bins on `A·g² + S·g` with `A` = 1/4 and `S` = 1/2 at gains 1, 1.5, 2 and 3 — variances 3/4,
/// 21/16, 2 and 15/4 — give back `σ²` = 3/4 and `ρ` = 1/3, to the f64 solve and the one rounding
/// to f32 each. At gain 2 the split reads `3/4·2·(⅓·2 + ⅔)` = 2, which is `A·4 + S·2`; all read
/// noise would read 3, all sky 3/2.
#[test]
fn bins_on_the_two_terms_give_them_back() {
    let split = BackgroundSplit::fit(&bins(&[
        (1.0, 0.75),
        (1.5, 1.3125),
        (2.0, 2.0),
        (3.0, 3.75),
    ]));
    assert!((split.variance - 0.75).abs() <= 2e-7, "{split:?}");
    assert!((split.read_share - 1.0 / 3.0).abs() <= 2e-7, "{split:?}");
    assert!((split.at(2.0) - 2.0).abs() <= 1e-6, "{split:?}");
    for (read_share, expected) in [(1.0, 3.0), (0.0, 1.5)] {
        let split = BackgroundSplit {
            variance: 0.75,
            read_share,
        };
        assert_eq!(split.at(2.0), expected, "ρ {read_share}");
    }
}

/// Variances 2 and 4.5 at gains 1 and 3 fit `A` = −1/4 and `S` = 9/4 unconstrained, so a one-term
/// fit stands. Weighed `n/v²`, 25 and 400/81: the sky term `Σw·v·g / Σw·g²` = (350/3)/(625/9) =
/// 1.68 leaves a weighted residual of 4.0, the read term `Σw·v·g² / Σw·g⁴` = 250/425 leaves 52.9,
/// so `σ²` = 1.68 with `ρ` = 0. One bin cannot tell the terms apart and goes to the sky: 3 at gain
/// 2 is `S` = 3/2. No bin reads 0.
#[test]
fn a_negative_term_falls_back_to_the_better_one_term_fit() {
    let split = BackgroundSplit::fit(&bins(&[(1.0, 2.0), (3.0, 4.5)]));
    assert_eq!(split.read_share, 0.0);
    assert!((split.variance - 1.68).abs() <= 2e-7, "{split:?}");
    assert_eq!(
        BackgroundSplit::fit(&bins(&[(2.0, 3.0)])),
        BackgroundSplit {
            variance: 1.5,
            read_share: 0.0,
        }
    );
    assert_eq!(BackgroundSplit::fit(&[]), BackgroundSplit::unflattened(0.0));
}

/// Sixteen gains 1 to 16 cut into eight bins of two: the bounds are the gains at ranks 2, 4, …, 14,
/// so 3, 5, …, 15, and a gain falls in the bin whose bound it has not reached.
#[test]
fn gain_bins_hold_equal_counts() {
    let mut gains: Vec<f32> = (1..=16).rev().map(|gain| gain as f32).collect();
    let bins = GainBins::of(&mut gains);
    for (gain, bin) in [
        (1.0, 0),
        (2.0, 0),
        (3.0, 1),
        (14.0, 6),
        (15.0, 7),
        (16.0, 7),
    ] {
        assert_eq!(bins.bin(gain), bin, "gain {gain}");
    }
}

/// Bins under 256 samples merge with their neighbours up in gain: over bounds 3, 5, …, 15, 300
/// samples at gain 1 fill bin 0 alone; 100 each at gains 3, 5 and 7 fill bins 1 to 3 only together,
/// 300; and the 50 at gain 15 in bin 7, short at the end, join that run, 350 samples of mean gain
/// (300 + 500 + 700 + 750)/350 = 2250/350. A hundred samples in all are one run.
#[test]
fn short_bins_merge_up_in_gain() {
    let mut gains: Vec<f32> = (1..=16).map(|gain| gain as f32).collect();
    let bins = GainBins::of(&mut gains);
    let mut binned = BinnedSamples::new();
    for (gain, count) in [(1.0, 300), (3.0, 100), (5.0, 100), (7.0, 100), (15.0, 50)] {
        for _ in 0..count {
            binned.push(&bins, gain, ());
        }
    }
    let measured = binned.measure(|samples| samples.len() as f64);
    let runs: Vec<(usize, f64)> = measured.iter().map(|bin| (bin.samples, bin.gain)).collect();
    assert_eq!(runs, [(300, 1.0), (350, 2250.0 / 350.0)]);
    assert_eq!(measured[1].variance, 350.0);

    let mut few = BinnedSamples::new();
    for _ in 0..100 {
        few.push(&bins, 1.0, ());
    }
    let measured = few.measure(|samples| samples.len() as f64);
    assert_eq!(measured.len(), 1);
    assert_eq!(measured[0].samples, 100);
}
