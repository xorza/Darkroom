//! One shape table and one sweep for cross-checking a vector kernel against its scalar reference.
//!
//! The inputs live here — uniform rows, a ramp, a spike, negatives, alternating values — rather
//! than as one `#[test]` per shape per module with an identical body: that shape makes a new case
//! an edit in every module, which in practice leaves each module covering a different subset.
//!
//! The kernels themselves are too varied to share a call signature: the median filter takes three
//! rows, convolution takes a kernel, the background interpolator writes two outputs, and resample
//! takes a transform. So the caller keeps its own call and this owns the inputs, the width sweep,
//! the comparison, and the walk over every tier the host has.

pub(crate) mod data_shape;

use crate::internals::simd_check::data_shape::DataShape;
use crate::internals::test_rng::TestRng;
use crate::simd::tier::Tier;

/// The inputs every SIMD cross-check runs over. Adding one here covers every kernel at once,
/// which is the point — the per-module copies could not do that.
pub(crate) const DATA_SHAPES: &[DataShape] = &[
    DataShape {
        name: "uniform",
        fill: |w, s| vec![0.5 + s as f32 * 0.1; w],
    },
    DataShape {
        name: "ascending",
        fill: |w, s| (0..w).map(|i| (i + s) as f32 * 0.01).collect(),
    },
    DataShape {
        name: "descending",
        fill: |w, s| (0..w).map(|i| (w - i + s) as f32 * 0.01).collect(),
    },
    DataShape {
        name: "alternating",
        fill: |w, s| {
            (0..w)
                .map(|i| if (i + s) % 2 == 0 { 0.0 } else { 1.0 })
                .collect()
        },
    },
    DataShape {
        // A lone spike is what separates a correct median/clamp from one that averages.
        name: "outlier",
        fill: |w, s| {
            let mut row = vec![0.2f32; w];
            row[(w / 2 + s) % w] = 900.0;
            row
        },
    },
    DataShape {
        // Exact halves: a rounding step that breaks ties another way than the scalar one shows
        // here, and the values leave [0, 1] on both sides.
        name: "half-steps",
        fill: |w, s| (0..w).map(|i| ((i + s) % 9) as f32 * 0.5 - 1.5).collect(),
    },
    DataShape {
        // Lanes far outside a unit domain beside lanes inside it, so a clamp or a range guard
        // that acts per vector rather than per lane shows.
        name: "out-of-range",
        fill: |w, s| {
            (0..w)
                .map(|i| match (i + s) % 3 {
                    0 => -50.0,
                    1 => 0.25,
                    _ => 75.0,
                })
                .collect()
        },
    },
    DataShape {
        name: "negative",
        fill: |w, s| (0..w).map(|i| -((i + s) as f32) * 0.05 - 0.1).collect(),
    },
    DataShape {
        // Large magnitudes expose an absolute-tolerance comparison that should be relative.
        name: "large",
        fill: |w, s| (0..w).map(|i| ((i + s) as f32).mul_add(1e4, 1e6)).collect(),
    },
    DataShape {
        name: "pseudo-random",
        fill: |w, s| {
            let mut rng = TestRng::new(0x2545_F491_4F6C_DD1D ^ s as u64);
            (0..w).map(|_| rng.next_f32()).collect()
        },
    },
];

/// Widths spanning every lane boundary a kernel switches on: below a vector, exactly one,
/// one-past, and several multiples plus an odd tail.
pub(crate) const SWEEP_WIDTHS: &[usize] = &[3, 4, 5, 7, 8, 9, 11, 15, 16, 17, 31, 32, 33, 64, 100];

/// What one run of a kernel pair produced, for the harness to compare.
#[derive(Debug)]
pub(crate) struct ScalarSimd {
    scalar: Vec<f32>,
    simd: Vec<f32>,
    /// Per element, what the error is measured against; empty for each element's own size.
    magnitude: Vec<f32>,
}

impl ScalarSimd {
    /// Outputs whose error is measured against each element's own size, floored at 1.
    pub(crate) fn new(scalar: Vec<f32>, simd: Vec<f32>) -> ScalarSimd {
        ScalarSimd {
            scalar,
            simd,
            magnitude: Vec::new(),
        }
    }

    /// Outputs that are sums, with `magnitude[i]` the sum of the absolute terms of element `i`.
    ///
    /// Two summation orders of one sum differ by at most `2·γₖ·Σ|tᵢ|`, with `γₖ ≈ k·u` for k
    /// terms (Higham, *Accuracy and Stability of Numerical Algorithms*, §3.1), however far the
    /// terms cancel. Measured against `Σ|tᵢ|` rather than against the result, the tolerance can be
    /// that bound.
    pub(crate) fn of_sums(scalar: Vec<f32>, simd: Vec<f32>, magnitude: Vec<f32>) -> ScalarSimd {
        assert_eq!(magnitude.len(), scalar.len(), "one magnitude per element");
        ScalarSimd {
            scalar,
            simd,
            magnitude,
        }
    }

    /// Outputs whose error is measured purely relative to the scalar value, with no absolute
    /// floor: a zero must stay exactly zero.
    pub(crate) fn relative(scalar: Vec<f32>, simd: Vec<f32>) -> ScalarSimd {
        let magnitude = scalar.iter().map(|value| value.abs()).collect();
        ScalarSimd::of_sums(scalar, simd, magnitude)
    }

    fn agree(&self, i: usize, tol: f64) -> bool {
        let (s, v) = (f64::from(self.scalar[i]), f64::from(self.simd[i]));
        match self.magnitude.get(i) {
            Some(&magnitude) => s == v || (s - v).abs() <= tol * f64::from(magnitude),
            // Each element's own size, floored at 1: near zero a relative bound leaves no room.
            None => s == v || (s - v).abs() <= tol * s.abs().max(v.abs()).max(1.0),
        }
    }
}

/// Run `kernels` on every tier this CPU has (see [`Tier::supported`]), over every shape in
/// [`DATA_SHAPES`] at every width in `widths`, asserting that each tier agrees with the scalar
/// reference to `tol` (see [`ScalarSimd`]; `0.0` demands equality).
///
/// `kernels` is handed one tier, a shape and a width, and returns the scalar and the vector
/// output; whatever else the kernel needs, it closes over.
pub(crate) fn assert_simd_matches_scalar(
    widths: &[usize],
    tol: f32,
    kernels: impl Fn(Tier, &DataShape, usize) -> ScalarSimd,
) {
    let tol = f64::from(tol);
    for tier in Tier::supported() {
        for shape in DATA_SHAPES {
            for &width in widths {
                let out = kernels(tier, shape, width);
                assert_eq!(
                    out.scalar.len(),
                    out.simd.len(),
                    "{tier} {} w={width}: output lengths differ",
                    shape.name
                );
                for i in 0..out.scalar.len() {
                    assert!(
                        out.agree(i, tol),
                        "{tier} {} w={width} [{i}]: scalar {} vs simd {}",
                        shape.name,
                        out.scalar[i],
                        out.simd[i]
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use crate::internals::simd_check::{
        DATA_SHAPES, SWEEP_WIDTHS, ScalarSimd, assert_simd_matches_scalar,
    };
    use crate::simd::tier::Tier;

    #[test]
    fn every_shape_fills_the_requested_width_and_varies_with_seed() {
        for shape in DATA_SHAPES {
            for &width in SWEEP_WIDTHS {
                assert_eq!(shape.row(width, 0).len(), width, "{}", shape.name);
            }
            // Decorrelated rows are what the three-row kernels rely on.
            assert_ne!(
                shape.row(16, 0),
                shape.row(16, 1),
                "{} ignores its seed",
                shape.name
            );
        }
    }

    #[test]
    #[should_panic(expected = "Portable uniform w=4 [0]: scalar 1 vs simd 2")]
    fn a_disagreeing_kernel_pair_fails() {
        assert_simd_matches_scalar(&[4], 1e-6, |_, _, width| {
            ScalarSimd::new(vec![1.0; width], vec![2.0; width])
        });
    }

    /// Sums are held to `tol` of their absolute terms: a difference of 1 in a sum whose terms
    /// add to 10 in magnitude is exactly a tolerance of 0.1.
    #[test]
    fn sums_are_measured_against_their_absolute_terms() {
        let sums = || ScalarSimd::of_sums(vec![0.5], vec![1.5], vec![10.0]);
        assert!(sums().agree(0, 0.1));
        assert!(!sums().agree(0, 0.099));
        // Each element's own size would have refused it at 0.1.
        assert!(!ScalarSimd::new(vec![0.5], vec![1.5]).agree(0, 0.1));
    }

    /// Each supported tier runs once per shape and width: over widths 3 and 8, twice per shape.
    #[test]
    fn every_tier_runs_every_shape_at_every_width() {
        let calls = Cell::new(0);
        assert_simd_matches_scalar(&[3, 8], 0.0, |_, shape, width| {
            calls.set(calls.get() + 1);
            let row = shape.row(width, 0);
            ScalarSimd::new(row.clone(), row)
        });
        assert_eq!(
            calls.get(),
            Tier::supported().count() * DATA_SHAPES.len() * 2
        );
    }
}
