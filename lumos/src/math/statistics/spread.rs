//! [`Spread`]: a robust location and scale, and the floor below which a measured scale says
//! nothing.

use crate::math::statistics::MAD_TO_SIGMA;

/// The median of a sorted window and its MAD in Gaussian units, unbiased at every count.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Spread {
    pub(crate) centre: f32,
    pub(crate) sigma: f32,
}

/// `b_n` for n = 2 ..= 20, so that `b_n · 1.4826 · MAD` is unbiased for n Gaussian samples. The
/// MAD here takes the mean of the two middle values at an even count, for the median and for the
/// median of the deviations, and Croux & Rousseeuw's (1992) table does not fit that form at even n
/// (n = 6: 1.200 against 1.1895). Measured by `internals/reference/mad_consistency.py` from 10⁷
/// trials per count: the standard error is below 3·10⁻⁴ at n = 3 and falls with n.
const MAD_CONSISTENCY: [f32; 19] = [
    1.1955, 1.4869, 1.3604, 1.2170, 1.1895, 1.1378, 1.1274, 1.1011, 1.0958, 1.0799, 1.0765, 1.0661,
    1.0638, 1.0563, 1.0548, 1.0491, 1.0478, 1.0435, 1.0425,
];

/// The spacing of `f32` at 0.
const SMALLEST_STEP: f32 = f32::from_bits(1);

impl Spread {
    /// Measure an ascending window. One sample has no spread.
    pub(crate) fn of_sorted(sorted: &[f32]) -> Self {
        debug_assert!(!sorted.is_empty());
        debug_assert!(sorted.is_sorted());
        let centre = Self::median_of_sorted(sorted);
        let sigma = if sorted.len() < 2 {
            0.0
        } else {
            let scale = MAD_TO_SIGMA * f64::from(Self::consistency(sorted.len()));
            (f64::from(mad_of_sorted(sorted, centre)) * scale) as f32
        };
        Self { centre, sigma }
    }

    /// The median of an ascending window: the middle sample, or the mean of the two middle ones.
    /// The upper-middle sample alone sits high by up to half the gap between the two, which makes a
    /// symmetric band about it clip the low side harder.
    pub(crate) fn median_of_sorted(sorted: &[f32]) -> f32 {
        let n = sorted.len();
        debug_assert!(n > 0);
        if n % 2 == 1 {
            sorted[n / 2]
        } else {
            f32::midpoint(sorted[n / 2 - 1], sorted[n / 2])
        }
    }

    /// The small-sample factor `b_n` for `n ≥ 2` samples. Above the table, Croux & Rousseeuw's
    /// `n / (n − 0.8)`, which the same simulation puts within 0.1% of the measured factor from
    /// n = 20 on.
    pub(crate) fn consistency(n: usize) -> f32 {
        debug_assert!(n >= 2);
        if let Some(&factor) = MAD_CONSISTENCY.get(n - 2) {
            factor
        } else {
            let n = n as f64;
            (n / (n - 0.8)) as f32
        }
    }

    /// The smallest spread data at `centre` can show: one `f32` step there, `|centre|·ε` within a
    /// factor of two, and the smallest subnormal at 0. A smaller σ is not measurable whatever span
    /// the decoder divided by, which is what makes the floor scale-free where a fixed
    /// `f32::EPSILON` declares every 32-bit integer FITS flat.
    pub(crate) const fn resolution(centre: f32) -> f32 {
        (centre.abs() * f32::EPSILON).max(SMALLEST_STEP)
    }

    /// The σ to scale a band by: the measured one, raised to `background` and to the resolution at
    /// the centre. Always positive.
    ///
    /// `background` is the noise the frames were measured to have away from this window. A
    /// per-window MAD cannot see it when more than half the samples tie, as integer data with
    /// little noise does, and an outlier cannot raise it. With no background, a band about a tied
    /// majority holds the tied samples alone.
    pub(crate) const fn floored(self, background: f32) -> f32 {
        self.sigma
            .max(background)
            .max(Self::resolution(self.centre))
    }
}

/// The MAD about `centre` of an ascending window, without a scratch buffer: the deviations form two
/// ascending runs, the samples below `centre` read backwards and the rest read forwards, so a merge
/// reaches them in order up to the middle rank. An even count takes the mean of the two middle
/// deviations, as [`Spread::median_of_sorted`] does for the samples.
fn mad_of_sorted(sorted: &[f32], centre: f32) -> f32 {
    let n = sorted.len();
    let split = sorted.partition_point(|&v| v < centre);
    let mut below = split;
    let mut above = split;
    let mut previous = 0.0f32;
    let mut deviation = 0.0f32;
    for _ in 0..=n / 2 {
        previous = deviation;
        let low = (below > 0).then(|| centre - sorted[below - 1]);
        let high = (above < n).then(|| sorted[above] - centre);
        deviation = match (low, high) {
            (Some(low), Some(high)) if low <= high => {
                below -= 1;
                low
            }
            (Some(low), None) => {
                below -= 1;
                low
            }
            (_, Some(high)) => {
                above += 1;
                high
            }
            (None, None) => unreachable!("the merge stops at the middle rank of a non-empty run"),
        };
    }
    if n.is_multiple_of(2) {
        f32::midpoint(previous, deviation)
    } else {
        deviation
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internals::invariance::Affine;
    use crate::internals::prelude::TestRng;
    use crate::math::statistics::median_mut;

    /// The centre and the MAD are medians at both parities. [1, 3, 7, 9]: centre (3 + 7)/2 = 5,
    /// deviations ranked [2, 2, 4, 4], MAD (2 + 4)/2 = 3. [1, 3, 7]: centre 3, deviations ranked
    /// [0, 2, 4], MAD 2. The merge agrees with sorting the deviations on odd and even counts, a
    /// centre at either end, ties and a heavy outlier.
    #[test]
    fn the_centre_and_the_mad_are_medians_at_both_parities() {
        assert_eq!(Spread::median_of_sorted(&[1.0, 3.0, 7.0, 9.0]), 5.0);
        assert_eq!(mad_of_sorted(&[1.0, 3.0, 7.0, 9.0], 5.0), 3.0);
        assert_eq!(Spread::median_of_sorted(&[1.0, 3.0, 7.0]), 3.0);
        assert_eq!(mad_of_sorted(&[1.0, 3.0, 7.0], 3.0), 2.0);

        let cases: &[&[f32]] = &[
            &[1.0, 2.0, 3.0, 4.0, 100.0],
            &[1.0, 2.0, 3.0, 4.0],
            &[-5.0, 0.0, 0.0, 0.0, 1.0, 2.0],
            &[10.0, 10.0, 10.0],
            &[0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7],
        ];
        for sorted in cases {
            for centre in [
                Spread::median_of_sorted(sorted),
                sorted[0],
                sorted[sorted.len() - 1],
            ] {
                let mut deviations: Vec<f32> = sorted.iter().map(|v| (v - centre).abs()).collect();
                deviations.sort_unstable_by(f32::total_cmp);
                let expected = Spread::median_of_sorted(&deviations);
                assert_eq!(
                    mad_of_sorted(sorted, centre),
                    expected,
                    "{sorted:?} about {centre}"
                );
            }
        }
    }

    /// σ is 1.4826 · MAD · `b_n`, in f64 and rounded once. [1, 3, 7, 9] has MAD 3 and n = 4, so
    /// σ = 3 · 1.482602 · 1.3604. Above the table, n = 21 gives 21/20.2. One sample has no spread.
    #[test]
    fn sigma_carries_the_small_sample_factor() {
        let four = Spread::of_sorted(&[1.0, 3.0, 7.0, 9.0]);
        assert_eq!(four.centre, 5.0);
        assert_eq!(
            four.sigma,
            (3.0 * MAD_TO_SIGMA * f64::from(1.3604f32)) as f32
        );
        assert_eq!(Spread::consistency(21), (21.0f64 / 20.2) as f32);
        assert_eq!(Spread::consistency(20), 1.0425);
        assert_eq!(
            Spread::of_sorted(&[4.0]),
            Spread {
                centre: 4.0,
                sigma: 0.0
            }
        );
    }

    /// The factors make σ unbiased: over 20000 windows of 5 and of 6 Gaussian samples the mean σ
    /// is 1. The standard deviation of σ is 0.58 at n = 5 and 0.47 at n = 6 (simulated), so the
    /// mean of 20000 has a standard error of 0.0041 at most; four of them is the tolerance. Without
    /// the factor the mean would be 1/1.217 = 0.82 and 1/1.19 = 0.84.
    #[test]
    fn sigma_is_unbiased_for_gaussian_windows() {
        let mut rng = TestRng::new(5);
        for n in [5usize, 6] {
            let mut total = 0.0f64;
            let mut window = vec![0.0f32; n];
            for _ in 0..20_000 {
                for sample in &mut window {
                    *sample = rng.next_gaussian_f32();
                }
                window.sort_unstable_by(f32::total_cmp);
                total += f64::from(Spread::of_sorted(&window).sigma);
            }
            let mean = total / 20_000.0;
            assert!((mean - 1.0).abs() < 0.0165, "n = {n}: mean σ {mean}");
        }
    }

    /// The floor takes the largest of the measured σ, the background and one `f32` step at the
    /// centre. At centre 2 the step is 2·2⁻²³ = 2⁻²²; at 0 it is the smallest subnormal, 2⁻¹⁴⁹.
    #[test]
    fn the_floor_takes_the_largest_of_its_three_terms() {
        let spread = |sigma| Spread { centre: 2.0, sigma };
        assert_eq!(spread(0.5).floored(0.25), 0.5);
        assert_eq!(spread(0.125).floored(0.25), 0.25);
        assert_eq!(spread(0.0).floored(0.0), 2.0f32.powi(-22));
        assert_eq!(
            Spread {
                centre: 0.0,
                sigma: 0.0
            }
            .floored(0.0),
            f32::from_bits(1)
        );
        assert_eq!(f64::from(f32::from_bits(1)), 2.0f64.powi(-149));
    }

    /// The spread maps with the data: σ scales with the data and ignores an offset, the centre maps
    /// as a sample does, under every exact map of the invariance harness.
    #[test]
    fn the_spread_is_affine_equivariant() {
        let window: Vec<f32> = [-0.5, -0.125, 0.0, 0.25, 0.375, 1.0, 1.5]
            .into_iter()
            .map(Affine::quantize)
            .collect();
        Affine::assert_equivariant(
            |case| {
                let spread = Spread::of_sorted(&case.apply_all(&window));
                (spread.centre, spread.sigma)
            },
            |&(centre, sigma), case| (case.apply(centre), sigma * case.scale),
        );
    }

    /// [−1, 2, 3, 5, 7]: the median 3 is the quickselect median, the deviations rank
    /// [0, 1, 2, 4, 4], so the MAD is 2 and σ = 2 · 1.4826 · `b_5`.
    #[test]
    fn an_odd_window_takes_its_middle_sample() {
        let mut values = vec![3.0f32, -1.0, 7.0, 2.0, 5.0];
        let mut sorted = values.clone();
        sorted.sort_unstable_by(f32::total_cmp);
        let spread = Spread::of_sorted(&sorted);
        assert_eq!(spread.centre, median_mut(&mut values));
        assert_eq!(
            spread.sigma,
            (2.0 * MAD_TO_SIGMA * f64::from(Spread::consistency(5))) as f32
        );
    }
}
