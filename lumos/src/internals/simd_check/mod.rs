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
use simd::IsaTier;

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
        // NaN and both infinities beside ordinary values: a kernel that takes one in any lane
        // decides it as its reference does, in that lane alone.
        name: "non-finite",
        fill: |w, s| {
            (0..w)
                .map(|i| match (i + s) % 5 {
                    0 => f32::NAN,
                    1 => 0.4,
                    2 => f32::INFINITY,
                    3 => f32::NEG_INFINITY,
                    _ => 0.8,
                })
                .collect()
        },
    },
    DataShape {
        // Both zeros and subnormals of both signs, where a min, a max or a flush to zero shows
        // which operand it keeps.
        name: "zeros-and-subnormals",
        fill: |w, s| {
            (0..w)
                .map(|i| match (i + s) % 5 {
                    0 => 0.0,
                    1 => -0.0,
                    2 => 1e-40,
                    3 => -1e-40,
                    _ => 0.6,
                })
                .collect()
        },
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
}

impl ScalarSimd {
    pub(crate) const fn new(scalar: Vec<f32>, simd: Vec<f32>) -> Self {
        Self { scalar, simd }
    }

    /// Element `i` is the same bits on both sides, or NaN on both: IEEE 754 leaves a NaN's sign and
    /// payload to the hardware, and `simd` specifies its ops to the bit save those.
    fn agree(&self, i: usize) -> bool {
        let (s, v) = (self.scalar[i], self.simd[i]);
        s.to_bits() == v.to_bits() || (s.is_nan() && v.is_nan())
    }
}

/// Run `kernels` on every tier this CPU has (see [`IsaTier::supported`]), over every shape in
/// [`DATA_SHAPES`] at every width in `widths`, asserting that each tier computes its scalar
/// reference's bits (see [`ScalarSimd`]).
///
/// `kernels` is handed one tier, a shape and a width, and returns the scalar and the vector
/// output; whatever else the kernel needs, it closes over.
pub(crate) fn assert_simd_matches_scalar(
    widths: &[usize],
    kernels: impl Fn(IsaTier, &DataShape, usize) -> ScalarSimd,
) {
    for tier in IsaTier::supported() {
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
                        out.agree(i),
                        "{tier} {} w={width} [{i}]: scalar {:?} vs simd {:?}",
                        shape.name,
                        out.scalar[i],
                        out.simd[i]
                    );
                }
            }
        }
    }
}

mod tests {
    use std::cell::Cell;

    use crate::internals::simd_check::{
        DATA_SHAPES, SWEEP_WIDTHS, ScalarSimd, assert_simd_matches_scalar,
    };
    use simd::IsaTier;

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
    #[should_panic(expected = "Portable uniform w=4 [0]: scalar 1.0 vs simd 2.0")]
    fn a_disagreeing_kernel_pair_fails() {
        assert_simd_matches_scalar(&[4], |_, _, width| {
            ScalarSimd::new(vec![1.0; width], vec![2.0; width])
        });
    }

    /// Bits decide: the two zeros and two values an ulp apart differ, and any two NaNs agree.
    #[test]
    fn outputs_agree_bit_for_bit_and_nan_for_nan() {
        let pairs = [
            (0.0, 0.0, true),
            (0.0, -0.0, false),
            (1.0, 1.0f32.next_up(), false),
            (f32::NAN, -f32::NAN, true),
            (f32::NAN, f32::INFINITY, false),
        ];
        for (scalar, simd, agree) in pairs {
            assert_eq!(
                ScalarSimd::new(vec![scalar], vec![simd]).agree(0),
                agree,
                "{scalar:?} vs {simd:?}"
            );
        }
    }

    /// Each supported tier runs once per shape and width: over widths 3 and 8, twice per shape.
    #[test]
    fn every_tier_runs_every_shape_at_every_width() {
        let calls = Cell::new(0);
        assert_simd_matches_scalar(&[3, 8], |_, shape, width| {
            calls.set(calls.get() + 1);
            let row = shape.row(width, 0);
            ScalarSimd::new(row.clone(), row)
        });
        assert_eq!(
            calls.get(),
            IsaTier::supported().count() * DATA_SHAPES.len() * 2
        );
    }
}
