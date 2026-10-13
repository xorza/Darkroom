use crate::combine::rejection::normal_scores::{CACHE_BYTES, NormalScores};

/// `Φ⁻¹(1.625/2.25)` from `scipy.stats.norm.ppf`.
const SCIPY_TWO: f64 = 0.589_455_797_849_778_3;

/// One sample sits at the median: Φ⁻¹(0.625/1.25) = Φ⁻¹(0.5) = 0. Two sit at
/// ±Φ⁻¹(1.625/2.25), which SciPy gives as 0.5894558; statrs computes Φ⁻¹ through erfc⁻¹ with a
/// relative error near 1e-15, so 1e-12 holds the value and the odd symmetry about the middle. A
/// smaller count filled after a larger one finds its own slice.
#[test]
fn scores_are_blom_positions_for_each_count() {
    let mut scores = NormalScores::default();
    let five = scores.of_count(5).to_vec();
    assert_eq!(scores.of_count(1), [0.0]);
    let two = scores.of_count(2).to_vec();
    assert!((two[1] - SCIPY_TWO).abs() < 1e-12, "{two:?}");
    assert!((two[0] + two[1]).abs() < 1e-12, "{two:?}");
    for rank in 0..5 {
        assert!((five[rank] + five[4 - rank]).abs() < 1e-12, "{five:?}");
    }
    assert_eq!(scores.of_count(5), five);
}

/// The cache holds a row per count up to 362 frames: counts 1 to 362 fall in distinct rows of
/// 362, and 362 rows of 362 scores are 1 048 352 B, within 1 MiB. At 5000 frames it holds
/// ⌊1 048 576 / 40 000⌋ = 26 rows, so counts 5000 and 8 share row 8 and refill it, and each
/// still finds its own scores.
#[test]
fn the_cache_stays_within_its_bytes_and_keeps_every_count() {
    let mut small = NormalScores::default();
    small.reserve(362);
    assert!(
        small.counts.is_empty(),
        "no row before a score is asked for"
    );
    small.of_count(3);
    assert_eq!(small.counts.len(), 362);
    let mut large = NormalScores::default();
    large.reserve(5000);
    large.of_count(1);
    assert_eq!(large.counts.len(), 26);
    assert!(large.scores.len() * size_of::<f64>() <= CACHE_BYTES);
    let full = large.of_count(5000).to_vec();
    let few = large.of_count(8).to_vec();
    assert_eq!(large.counts[8], 8);
    assert_eq!(large.of_count(5000), full);
    assert_eq!(large.counts[8], 5000);
    assert_eq!(large.of_count(8), few);
    assert_eq!(few, NormalScores::default().of_count(8));
    assert_eq!(full, NormalScores::default().of_count(5000));
}
