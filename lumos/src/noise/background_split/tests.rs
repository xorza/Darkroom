use crate::noise::background_split::BackgroundSplit;
use crate::noise::binned_samples::GainBin;

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
