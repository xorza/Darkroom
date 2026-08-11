//! One shape table and one sweep for cross-checking a SIMD kernel against its scalar reference.
//!
//! The inputs live here — uniform rows, a ramp, a spike, negatives, alternating values — rather
//! than as one `#[test]` per shape per module with an identical body: that shape makes a new case an
//! edit in every module, which in practice leaves each module covering a different subset.
//!
//! The kernels themselves are too varied to share a call signature: the median filter takes three
//! rows, convolution takes a kernel, the background interpolator writes two outputs, and resample
//! takes a transform. So the caller keeps its own call and this owns the inputs, the width sweep,
//! and the comparison.

use crate::testing::assertions::is_close;

/// A named way of filling a row. `seed` lets a caller draw several decorrelated rows of the same
/// shape, which is what the three-row kernels need.
pub(crate) struct DataShape {
    pub(crate) name: &'static str,
    fill: fn(usize, usize) -> Vec<f32>,
}

impl std::fmt::Debug for DataShape {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DataShape")
            .field("name", &self.name)
            .finish()
    }
}

impl DataShape {
    /// One row of `width` samples; `seed` shifts the pattern without changing its character.
    pub(crate) fn row(&self, width: usize, seed: usize) -> Vec<f32> {
        (self.fill)(width, seed)
    }
}

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
            let mut state = 0x2545_F491_4F6C_DD1Du64 ^ (s as u64).wrapping_mul(0x9E37_79B9);
            (0..w)
                .map(|_| {
                    state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                    (state >> 33) as f32 / (1u64 << 31) as f32
                })
                .collect()
        },
    },
];

/// Widths spanning every lane boundary the backends switch on: below a vector, exactly one,
/// one-past, and several multiples plus an odd tail.
pub(crate) const SWEEP_WIDTHS: &[usize] = &[3, 4, 5, 7, 8, 9, 11, 15, 16, 17, 31, 32, 33, 64, 100];

/// What one run of a kernel pair produced, for the harness to compare.
#[derive(Debug)]
pub(crate) struct ScalarSimd {
    pub(crate) scalar: Vec<f32>,
    pub(crate) simd: Vec<f32>,
}

/// Run `kernels` over every shape in [`DATA_SHAPES`] at every width in `widths`, asserting the
/// two outputs agree to `tol` (absolutely or relatively — see [`is_close`]).
///
/// `kernels` is handed a shape and a width and returns both outputs; whatever else the kernel
/// needs, it closes over.
pub(crate) fn assert_simd_matches_scalar(
    widths: &[usize],
    tol: f32,
    kernels: impl Fn(&DataShape, usize) -> ScalarSimd,
) {
    let tol = f64::from(tol);
    for shape in DATA_SHAPES {
        for &width in widths {
            let out = kernels(shape, width);
            assert_eq!(
                out.scalar.len(),
                out.simd.len(),
                "{} w={width}: output lengths differ",
                shape.name
            );
            for (i, (s, v)) in out.scalar.iter().zip(&out.simd).enumerate() {
                assert!(
                    is_close(f64::from(*s), f64::from(*v), tol),
                    "{} w={width} [{i}]: scalar {s} vs simd {v}",
                    shape.name
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::simd_check::{
        DATA_SHAPES, SWEEP_WIDTHS, ScalarSimd, assert_simd_matches_scalar,
    };

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
    #[should_panic(expected = "scalar 1 vs simd 2")]
    fn a_disagreeing_kernel_pair_fails() {
        assert_simd_matches_scalar(&[4], 1e-6, |_, width| ScalarSimd {
            scalar: vec![1.0; width],
            simd: vec![2.0; width],
        });
    }

    #[test]
    fn an_agreeing_kernel_pair_passes_every_shape() {
        assert_simd_matches_scalar(SWEEP_WIDTHS, 1e-6, |shape, width| {
            let row = shape.row(width, 0);
            ScalarSimd {
                scalar: row.clone(),
                simd: row,
            }
        });
    }
}
