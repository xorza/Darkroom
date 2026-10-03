use crate::internals::simd_check::{SWEEP_WIDTHS, ScalarSimd, assert_simd_matches_scalar};
use crate::star_detection::median_filter::simd::MedianRow;
use crate::star_detection::median_filter::simd::internals::{
    median_filter_row_scalar, median9_scalar,
};

/// Every tier, over every shape and width, against the scalar reference. Min/max networks compute
/// no new values, so they agree exactly. The borders carry no 3x3 window, so only the interior is
/// comparable. Widths 3 through 9 take the zero-padded vector, the rest the overlapping one.
#[test]
fn median_filter_row_matches_scalar() {
    let widths: Vec<usize> = (3..10).chain(SWEEP_WIDTHS.iter().copied()).collect();
    assert_simd_matches_scalar(&widths, 0.0, |tier, shape, width| {
        let above = shape.row(width, 0);
        let curr = shape.row(width, 1);
        let below = shape.row(width, 2);
        let mut scalar = vec![0.0f32; width];
        let mut simd = vec![0.0f32; width];
        median_filter_row_scalar(&above, &curr, &below, &mut scalar);
        tier.run(MedianRow {
            above: &above,
            curr: &curr,
            below: &below,
            output: &mut simd,
        });
        let interior = 1..width - 1;
        ScalarSimd::new(scalar[interior.clone()].to_vec(), simd[interior].to_vec())
    });
}

#[test]
fn scalar_row_matches_a_hand_taken_neighbourhood_median() {
    let width = 16;
    let row_above: Vec<f32> = (0..width).map(|i| (i % 10) as f32 * 0.1).collect();
    let row_curr: Vec<f32> = (0..width).map(|i| ((i + 3) % 10) as f32 * 0.1).collect();
    let row_below: Vec<f32> = (0..width).map(|i| ((i + 7) % 10) as f32 * 0.1).collect();
    let mut output = vec![0.0f32; width];

    median_filter_row_scalar(&row_above, &row_curr, &row_below, &mut output);

    let x = 5;
    let mut values = [
        row_above[x - 1],
        row_above[x],
        row_above[x + 1],
        row_curr[x - 1],
        row_curr[x],
        row_curr[x + 1],
        row_below[x - 1],
        row_below[x],
        row_below[x + 1],
    ];
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let expected = values[4];

    assert!(
        (output[x] - expected).abs() < 1e-6,
        "Scalar median mismatch at x={}: got {}, expected {}",
        x,
        output[x],
        expected
    );
}

#[test]
fn median9_scalar_known_values() {
    // Sorted: 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9
    // Median should be 0.5 (index 4)
    let result = median9_scalar([0.5, 0.1, 0.9, 0.2, 0.8, 0.3, 0.7, 0.4, 0.6]);
    assert!((result - 0.5).abs() < 1e-6, "Expected 0.5, got {result}");
}

#[test]
fn median9_scalar_all_same() {
    let result = median9_scalar([0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5]);
    assert!((result - 0.5).abs() < 1e-6, "Expected 0.5, got {result}");
}

#[test]
fn median9_scalar_various_orderings() {
    let expected_median = 0.5;

    let orderings = [
        [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9], // sorted
        [0.9, 0.8, 0.7, 0.6, 0.5, 0.4, 0.3, 0.2, 0.1], // reverse sorted
        [0.5, 0.1, 0.9, 0.2, 0.8, 0.3, 0.7, 0.4, 0.6], // mixed
        [0.1, 0.9, 0.2, 0.8, 0.3, 0.7, 0.4, 0.6, 0.5], // mixed 2
        [0.9, 0.1, 0.8, 0.2, 0.7, 0.3, 0.6, 0.4, 0.5], // mixed 3
    ];

    for (idx, order) in orderings.iter().enumerate() {
        let result = median9_scalar(*order);
        assert!(
            (result - expected_median).abs() < 1e-6,
            "Ordering {idx}: expected {expected_median}, got {result}"
        );
    }
}

#[test]
fn median9_scalar_with_duplicates() {
    let result = median9_scalar([0.5, 0.5, 0.5, 0.1, 0.1, 0.9, 0.9, 0.3, 0.7]);
    // Sorted: 0.1, 0.1, 0.3, 0.5, 0.5, 0.5, 0.7, 0.9, 0.9 -> median is 0.5
    assert!(
        (result - 0.5).abs() < 1e-6,
        "Duplicates test: expected 0.5, got {result}"
    );
}

#[test]
fn median9_scalar_extreme_values() {
    let result = median9_scalar([f32::MIN, f32::MAX, 0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    // Sorted: MIN, 0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, MAX -> median is 3.0
    assert!(
        (result - 3.0).abs() < 1e-6,
        "Extreme values test: expected 3.0, got {result}"
    );
}
