//! Checks that a stage's decisions do not depend on the scale or the zero point of its data.
//!
//! Every decode path lands on `[0, 1]`, but what one unit is worth differs by orders of magnitude:
//! 16-bit data in a 32-bit integer FITS arrives `2¹⁶` times smaller than the same data in a 16-bit
//! one. A stage that compares a spread with a fixed number decides differently on the two. The
//! check runs a stage under several affine maps of one input and compares each output with the
//! identity output, mapped the way the stage promises to map it.

use std::fmt::Debug;

/// The data grid every [`Affine::CASES`] map is exact on: multiples of `2⁻¹⁹` below 4 in
/// magnitude. Each case's product and sum then land on a value the result's own precision holds.
const GRID: f32 = 524_288.0;
const GRID_LIMIT: f32 = 4.0;

/// `x ↦ scale·x + offset`, exact on data that [`Affine::quantize`] produced.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Affine {
    pub(crate) scale: f32,
    pub(crate) offset: f32,
}

impl Affine {
    pub(crate) const IDENTITY: Self = Self {
        scale: 1.0,
        offset: 0.0,
    };

    /// The maps a stage is checked under. Each scale is a power of two, so the product is exact.
    pub(crate) const CASES: [Self; 5] = [
        // A 16-bit frame normalized a second time.
        Self {
            scale: 1.0 / 65_536.0,
            offset: 0.0,
        },
        // 16-bit data in a 32-bit integer FITS, normalized by 2³² − 1.
        Self {
            scale: 1.0 / 65_536.0 / 65_536.0,
            offset: 0.0,
        },
        // Data left in ADU.
        Self {
            scale: 256.0,
            offset: 0.0,
        },
        // A pedestal, such as a black level the decoder kept.
        Self {
            scale: 1.0,
            offset: 0.5,
        },
        // Both: a small scale and a pedestal far above the noise.
        Self {
            scale: 1.0 / 65_536.0,
            offset: 1.0 / 4096.0,
        },
    ];

    /// Round `x` to the grid every case is exact on.
    ///
    /// # Panics
    /// When `|x|` reaches the grid limit, where a case could round.
    pub(crate) fn quantize(x: f32) -> f32 {
        assert!(
            x.abs() < GRID_LIMIT,
            "{x} is outside the exact grid of ±{GRID_LIMIT}"
        );
        (x * GRID).round() / GRID
    }

    /// Map one sample.
    ///
    /// # Panics
    /// When the result rounds, which means `x` did not come from [`Self::quantize`].
    pub(crate) fn apply(self, x: f32) -> f32 {
        let y = x * self.scale + self.offset;
        assert_eq!(
            f64::from(y),
            f64::from(x) * f64::from(self.scale) + f64::from(self.offset),
            "{self:?} rounds {x}"
        );
        y
    }

    pub(crate) fn apply_all(self, values: &[f32]) -> Vec<f32> {
        values.iter().map(|&x| self.apply(x)).collect()
    }

    /// Run `stage` under the identity and under every case, and return each case whose output
    /// differs from `expected(identity output, case)`.
    pub(crate) fn mismatches<T: PartialEq + Debug>(
        stage: impl Fn(Self) -> T,
        expected: impl Fn(&T, Self) -> T,
    ) -> Vec<Mismatch<T>> {
        let identity = stage(Self::IDENTITY);
        Self::CASES
            .into_iter()
            .filter_map(|case| {
                let expected = expected(&identity, case);
                let actual = stage(case);
                (actual != expected).then_some(Mismatch {
                    case,
                    expected,
                    actual,
                })
            })
            .collect()
    }

    /// Assert that `stage` maps every case's input to `expected(identity output, case)`. Pass
    /// `|output, _| output.clone()` for a decision that must not change at all.
    #[track_caller]
    pub(crate) fn assert_equivariant<T: PartialEq + Debug>(
        stage: impl Fn(Self) -> T,
        expected: impl Fn(&T, Self) -> T,
    ) {
        let mismatches = Self::mismatches(stage, expected);
        assert!(
            mismatches.is_empty(),
            "the stage depends on the data's scale or zero point: {mismatches:#?}"
        );
    }
}

/// One case where a stage's output was not the mapped identity output.
#[derive(Debug, PartialEq)]
pub(crate) struct Mismatch<T> {
    pub(crate) case: Affine,
    pub(crate) expected: T,
    pub(crate) actual: T,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The grid holds every case exactly, at its edges too: the largest magnitude below the limit,
    /// and the finest step.
    #[test]
    fn every_case_is_exact_on_the_grid() {
        let step = 1.0 / GRID;
        for x in [
            0.0,
            step,
            -step,
            1.0 + step,
            GRID_LIMIT - step,
            -(GRID_LIMIT - step),
        ] {
            for case in Affine::CASES {
                case.apply(Affine::quantize(x));
            }
        }
    }

    /// A value off the grid is caught rather than compared after rounding: `0.25 + 2⁻²⁵` is an f32,
    /// but under the pedestal case it becomes `0.75 + 2⁻²⁵`, finer than the `2⁻²⁴` step of an f32
    /// near 0.75.
    #[test]
    #[should_panic(expected = "rounds")]
    fn an_off_grid_value_is_refused() {
        Affine::CASES[3].apply(0.25 + 1.0 / 33_554_432.0);
    }

    /// The median is equivariant, so it passes. A stage that tests its input against a fixed
    /// threshold is not, so it fails, under the cases that move its input across the threshold.
    #[test]
    fn the_check_passes_an_equivariant_stage_and_catches_a_fixed_threshold() {
        let samples: Vec<f32> = [0.25, -0.5, 1.0, 0.75, 0.0]
            .into_iter()
            .map(Affine::quantize)
            .collect();
        let median = |case: Affine| {
            let mut mapped = case.apply_all(&samples);
            mapped.sort_unstable_by(f32::total_cmp);
            mapped[2]
        };
        Affine::assert_equivariant(median, |&value, case| case.apply(value));

        let above_fixed = |case: Affine| case.apply(samples[2]) > 0.5;
        let failing: Vec<Affine> = Affine::mismatches(above_fixed, |&above, _| above)
            .into_iter()
            .map(|mismatch| mismatch.case)
            .collect();
        assert_eq!(
            failing,
            [Affine::CASES[0], Affine::CASES[1], Affine::CASES[4]]
        );
    }
}
