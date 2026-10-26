//! Tests for SIMD convolution implementations.

use common::Buffer2;

use crate::internals::simd_check::data_shape::DataShape;
use crate::internals::simd_check::{SWEEP_WIDTHS, ScalarSimd, assert_simd_matches_scalar};
use crate::star_detection::convolution::internals::convolve_cols;
use crate::star_detection::convolution::simd::{
    Convolve2dRow, ConvolveColsRow, ConvolveRow, Kernel2d, RowRing, mirror_idx,
};
use common::Size2us;
use simd::Kernel;

/// An asymmetric kernel, so a tap applied in mirrored order shows.
fn asymmetric_kernel(radius: usize) -> Vec<f32> {
    (0..=2 * radius).map(|i| (i as f32 + 1.0) * 0.05).collect()
}

/// An image of `size` whose row `y` is the shape's row for seed `y`.
fn shape_image(shape: &DataShape, size: Size2us) -> Buffer2<f32> {
    let pixels = (0..size.height)
        .flat_map(|y| shape.row(size.width, y))
        .collect();
    Buffer2::new(size, pixels)
}

#[test]
fn convolve_row_scalar_identity() {
    let input = vec![1.0, 2.0, 3.0, 4.0, 5.0];
    let kernel = vec![0.0, 1.0, 0.0];
    let mut output = vec![0.0; 5];

    ConvolveRow {
        input: &input,
        output: &mut output,
        kernel: &kernel,
    }
    .run_scalar();

    // 0·a + 1·b + 0·c is b exactly.
    assert_eq!(output, input);
}

#[test]
fn convolve_row_scalar_average() {
    let input = vec![0.0, 0.0, 3.0, 0.0, 0.0];
    let kernel = vec![1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0];
    let mut output = vec![0.0; 5];

    ConvolveRow {
        input: &input,
        output: &mut output,
        kernel: &kernel,
    }
    .run_scalar();

    // One nonzero term each: 3 · fl(1/3) rounds back to exactly 1.
    assert_eq!(output, [0.0, 1.0, 1.0, 1.0, 0.0]);
}

/// Every tier, at every radius through eight, over every shape at the sweep widths and at every
/// width across the alignments where the last vector lands on the mirrored edge. A row needs more
/// than `2r` samples for the mirror to stay inside it.
///
/// Every lane accumulates the taps in the scalar order with an unfused multiply then add, so each
/// is the scalar result bit for bit: the tolerance is zero.
#[test]
fn convolve_row_matches_scalar() {
    for radius in 1..=8 {
        let kernel = asymmetric_kernel(radius);
        let widths: Vec<usize> = SWEEP_WIDTHS
            .iter()
            .copied()
            .chain(2 * radius + 1..2 * radius + 48)
            .filter(|&width| width > 2 * radius)
            .collect();
        assert_simd_matches_scalar(&widths, 0.0, |tier, shape, width| {
            let input = shape.row(width, 0);
            let mut scalar = vec![0.0f32; width];
            let mut simd = vec![0.0f32; width];
            ConvolveRow {
                input: &input,
                output: &mut scalar,
                kernel: &kernel,
            }
            .run_scalar();
            tier.run(ConvolveRow {
                input: &input,
                output: &mut simd,
                kernel: &kernel,
            });
            ScalarSimd::new(scalar, simd)
        });
    }
}

/// The impulse response is the kernel *reversed*, which is what pins the tap order.
///
/// `ConvolveRow` correlates rather than convolves — `convolve_pixel_scalar` sums
/// `input[x + k - radius] · kernel[k]`, so an impulse at `p` puts `kernel[2r - t]` at column
/// `p - r + t`. That is the usual image-processing convention and production only ever passes
/// symmetric Gaussians, so it makes no difference there. It is why the kernel here is asymmetric:
/// a palindrome could not tell the two tap orders apart, and the parity sweep only says the
/// implementations agree. One tap meets the impulse, so each output is that weight exactly.
#[test]
fn convolve_row_impulse_response_is_the_reversed_kernel() {
    let width = 64;
    let radius = 2;
    let kernel = [0.1f32, 0.2, 0.4, 0.3, 0.05];
    let centre = width / 2;
    let mut input = vec![0.0f32; width];
    input[centre] = 1.0;

    // Both implementations, because an interior impulse takes the vector path — checking only
    // `ConvolveRow` would leave the scalar reference's own tap order unpinned, and the parity
    // sweep cannot tell which of the two is right.
    let mut simd = vec![0.0f32; width];
    let mut scalar = vec![0.0f32; width];
    ConvolveRow {
        input: &input,
        output: &mut simd,
        kernel: &kernel,
    }
    .dispatch();
    ConvolveRow {
        input: &input,
        output: &mut scalar,
        kernel: &kernel,
    }
    .run_scalar();

    for (label, output) in [("simd", &simd), ("scalar", &scalar)] {
        for tap in 0..kernel.len() {
            let col = centre - radius + tap;
            assert_eq!(
                output[col],
                kernel[2 * radius - tap],
                "{label}: column {col}"
            );
        }
        // Everything outside the kernel's reach stays zero.
        for (col, &value) in output.iter().enumerate() {
            if col < centre - radius || col > centre + radius {
                assert_eq!(value, 0.0, "{label}: column {col}");
            }
        }
    }
}

#[test]
fn mirror_idx_in_bounds() {
    for len in [5, 10, 100] {
        for i in 0..len {
            assert_eq!(mirror_idx(i as isize, len), i);
        }
    }
}

#[test]
fn mirror_idx_negative() {
    // Negative indices should reflect: -1 -> 1, -2 -> 2, etc.
    let len = 10;
    assert_eq!(mirror_idx(-1, len), 1);
    assert_eq!(mirror_idx(-2, len), 2);
    assert_eq!(mirror_idx(-3, len), 3);
}

#[test]
fn mirror_idx_overflow() {
    // Indices >= len should reflect: len -> len-2, len+1 -> len-3, etc.
    let len = 10;
    assert_eq!(mirror_idx(10, len), 8); // 2*10-2-10 = 8
    assert_eq!(mirror_idx(11, len), 7); // 2*10-2-11 = 7
    assert_eq!(mirror_idx(12, len), 6); // 2*10-2-12 = 6
}

#[test]
fn mirror_index_far_out_of_bounds() {
    // Past one reflection the index reflects again, with period 2(len − 1) = 8 for len 5:
    // −5 → 3, −10 → 2 (phase 6, 8 − 6), 20 → 4 (phase 4), 100 → 4 (phase 4). For len 3, period 4,
    // −5 → 1 and 5 → 1, where one reflection and a clamp gave 2 and 0. Length 1 is always 0.
    let len = 5;
    assert_eq!(mirror_idx(-5, len), 3);
    assert_eq!(mirror_idx(-10, len), 2);
    assert_eq!(mirror_idx(20, len), 4);
    assert_eq!(mirror_idx(100, len), 4);
    assert_eq!(mirror_idx(-5, 3), 1);
    assert_eq!(mirror_idx(5, 3), 1);
    assert_eq!(mirror_idx(7, 1), 0);
    for i in -20..30 {
        let idx = mirror_idx(i, len);
        assert!(idx < len, "mirror_index({i}, {len})");
        // Mirroring is symmetric about 0 and periodic.
        assert_eq!(idx, mirror_idx(-i, len), "{i}");
        assert_eq!(idx, mirror_idx(i + 8, len), "{i}");
    }
}

#[test]
fn convolve_cols_uniform_input() {
    // 42 times weights summing to 1 within their own rounding: five products and five sums,
    // 10ε relative.
    let width = 32;
    let height = 32;
    let input = Buffer2::new_filled((width, height), 42.0f32);
    let kernel = vec![0.1, 0.2, 0.4, 0.2, 0.1];

    let mut output = Buffer2::new_default((width, height));
    convolve_cols(&input, &mut output, &kernel);

    for (i, &v) in output.pixels().iter().enumerate() {
        assert!(
            (v - 42.0).abs() <= 42.0 * 10.0 * f32::EPSILON,
            "Uniform input should stay uniform at {i}: {v}"
        );
    }
}

#[test]
fn convolve_cols_impulse_response() {
    // As in the row case: an impulse at (4, 8) puts `kernel[2r − t]` at row `8 − r + t` of its
    // column, exactly, and nothing anywhere else. The kernel is asymmetric so the tap order shows.
    let width = 8;
    let height = 16;
    let mut input = Buffer2::new_filled((width, height), 0.0f32);
    input[(4, 8)] = 1.0;
    let kernel = [0.1f32, 0.2, 0.4, 0.3, 0.05];
    let radius = kernel.len() / 2;

    let mut output = Buffer2::new_default((width, height));
    convolve_cols(&input, &mut output, &kernel);

    for y in 0..height {
        for x in 0..width {
            let expected = if x == 4 && (8 - radius..=8 + radius).contains(&y) {
                kernel[2 * radius - (y + radius - 8)]
            } else {
                0.0
            };
            assert_eq!(output[(x, y)], expected, "({x}, {y})");
        }
    }
}

/// Every tier's column pass against the scalar reference, on images whose rows are the sweep's
/// shapes: the whole output, so the mirrored top and bottom rows are compared with the interior.
/// Bit for bit, as for rows.
#[test]
fn convolve_cols_matches_scalar() {
    for radius in [1, 2, 3, 5] {
        let kernel = asymmetric_kernel(radius);
        let height = 2 * radius + 6;
        assert_simd_matches_scalar(SWEEP_WIDTHS, 0.0, |tier, shape, width| {
            let size = Size2us::new(width, height);
            let input = shape_image(shape, size);
            let mut scalar = vec![0.0f32; width * height];
            let mut simd = vec![0.0f32; width * height];
            for y in 0..height {
                let row = y * width..(y + 1) * width;
                ConvolveColsRow {
                    input: RowRing::plane(&input),
                    out_row: &mut scalar[row.clone()],
                    y,
                    kernel: &kernel,
                }
                .run_scalar();
                tier.run(ConvolveColsRow {
                    input: RowRing::plane(&input),
                    out_row: &mut simd[row],
                    y,
                    kernel: &kernel,
                });
            }
            ScalarSimd::new(scalar, simd)
        });
    }
}

/// Every tier's 2D row against the scalar reference, for each odd kernel size through nine, on the
/// two mirrored rows at each edge and one interior row. Bit for bit, as for rows; the weights rise
/// across the kernel, so no reflection of it is itself. Widths below a vector take the gathered
/// partial vector.
#[test]
fn convolve_2d_row_matches_scalar() {
    for ksize in [3, 5, 7, 9] {
        let radius = ksize / 2;
        let weights: Vec<f32> = (0..ksize * ksize)
            .map(|i| (i as f32 + 1.0) * 0.01)
            .collect();
        let kernel = Kernel2d::new(&weights, ksize);
        let height = 2 * radius + 6;
        let widths: Vec<usize> = (1..8).chain(SWEEP_WIDTHS.iter().copied()).collect();
        assert_simd_matches_scalar(&widths, 0.0, |tier, shape, width| {
            let size = Size2us::new(width, height);
            let input = shape_image(shape, size);
            let mut scalar = Vec::new();
            let mut simd = Vec::new();
            for y in [0, 1, height / 2, height - 2, height - 1] {
                let mut scalar_row = vec![0.0f32; width];
                let mut simd_row = vec![0.0f32; width];
                Convolve2dRow {
                    input: RowRing::plane(&input),
                    output_row: &mut scalar_row,
                    y,
                    kernel,
                }
                .run_scalar();
                tier.run(Convolve2dRow {
                    input: RowRing::plane(&input),
                    output_row: &mut simd_row,
                    y,
                    kernel,
                });
                scalar.extend(scalar_row);
                simd.extend(simd_row);
            }
            ScalarSimd::new(scalar, simd)
        });
    }
}

#[test]
fn convolve_2d_row_uniform() {
    // 42 times 25 weights of 1/25: 25 products and 25 sums, 50ε relative.
    let width = 16;
    let height = 16;
    let input = Buffer2::new_filled((width, height), 42.0f32);
    let weights = vec![1.0 / 25.0; 25];
    let kernel = Kernel2d::new(&weights, 5);

    for y in 0..height {
        let mut output = vec![0.0f32; width];
        Convolve2dRow {
            input: RowRing::plane(&input),
            output_row: &mut output,
            y,
            kernel,
        }
        .dispatch();
        for (x, &v) in output.iter().enumerate() {
            assert!(
                (v - 42.0).abs() <= 42.0 * 50.0 * f32::EPSILON,
                "row {y} x {x}: {v}"
            );
        }
    }
}

#[test]
fn convolve_2d_row_impulse_response() {
    // An impulse at (8, 8) through a 3×3 kernel of distinct weights: row `8 + dy` carries the
    // kernel's row `1 − dy` reversed, exactly — both axes correlate, as `ConvolveRow` does.
    let width = 16;
    let height = 16;
    let mut input = Buffer2::new_default((width, height));
    input[8 * width + 8] = 1.0;
    let weights: Vec<f32> = (1..=9).map(|w| w as f32 * 0.1).collect();
    let kernel = Kernel2d::new(&weights, 3);

    for y in 0..height {
        let mut output = vec![0.0f32; width];
        Convolve2dRow {
            input: RowRing::plane(&input),
            output_row: &mut output,
            y,
            kernel,
        }
        .dispatch();
        for (x, &value) in output.iter().enumerate() {
            let expected = if (7..=9).contains(&y) && (7..=9).contains(&x) {
                kernel.at(9 - y, 9 - x)
            } else {
                0.0
            };
            assert_eq!(value, expected, "({x}, {y})");
        }
    }
}
