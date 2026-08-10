use crate::stacking::star_detection::median_filter::simd::*;
use crate::testing::prelude::*;

/// Every shape and width against the scalar reference, replacing eleven near-identical tests
/// that each covered one shape. The borders carry no 3x3 window, so only the interior is
/// comparable; width 3 leaves exactly one interior column, which is the minimum-width case.
#[test]
fn median_filter_row_simd_matches_scalar() {
    assert_simd_matches_scalar(SWEEP_WIDTHS, 1e-5, |shape, width| {
        let above = shape.row(width, 0);
        let curr = shape.row(width, 1);
        let below = shape.row(width, 2);
        let mut scalar = vec![0.0f32; width];
        let mut simd = vec![0.0f32; width];
        median_filter_row_scalar(&above, &curr, &below, &mut scalar, width);
        median_filter_row_simd(&above, &curr, &below, &mut simd, width);
        let interior = 1..width - 1;
        ScalarSimd {
            scalar: scalar[interior.clone()].to_vec(),
            simd: simd[interior].to_vec(),
        }
    });
}

#[test]
fn scalar_row_matches_a_hand_taken_neighbourhood_median() {
    let width = 16;
    let row_above: Vec<f32> = (0..width).map(|i| (i % 10) as f32 * 0.1).collect();
    let row_curr: Vec<f32> = (0..width).map(|i| ((i + 3) % 10) as f32 * 0.1).collect();
    let row_below: Vec<f32> = (0..width).map(|i| ((i + 7) % 10) as f32 * 0.1).collect();
    let mut output = vec![0.0f32; width];

    median_filter_row_scalar(&row_above, &row_curr, &row_below, &mut output, width);

    // Verify a specific pixel manually
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
    let result = median9_scalar(0.5, 0.1, 0.9, 0.2, 0.8, 0.3, 0.7, 0.4, 0.6);
    assert!((result - 0.5).abs() < 1e-6, "Expected 0.5, got {}", result);
}

#[test]
fn median9_scalar_all_same() {
    let result = median9_scalar(0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5);
    assert!((result - 0.5).abs() < 1e-6, "Expected 0.5, got {}", result);
}

#[test]
fn median9_scalar_various_orderings() {
    // Test the median9 function with various orderings of the same set of values
    let expected_median = 0.5;

    // Test with values in different orders
    let orderings = [
        [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9], // sorted
        [0.9, 0.8, 0.7, 0.6, 0.5, 0.4, 0.3, 0.2, 0.1], // reverse sorted
        [0.5, 0.1, 0.9, 0.2, 0.8, 0.3, 0.7, 0.4, 0.6], // mixed
        [0.1, 0.9, 0.2, 0.8, 0.3, 0.7, 0.4, 0.6, 0.5], // mixed 2
        [0.9, 0.1, 0.8, 0.2, 0.7, 0.3, 0.6, 0.4, 0.5], // mixed 3
    ];

    for (idx, order) in orderings.iter().enumerate() {
        let result = median9_scalar(
            order[0], order[1], order[2], order[3], order[4], order[5], order[6], order[7],
            order[8],
        );
        assert!(
            (result - expected_median).abs() < 1e-6,
            "Ordering {}: expected {}, got {}",
            idx,
            expected_median,
            result
        );
    }
}

#[test]
fn median9_scalar_with_duplicates() {
    // Test median with duplicate values
    let result = median9_scalar(0.5, 0.5, 0.5, 0.1, 0.1, 0.9, 0.9, 0.3, 0.7);
    // Sorted: 0.1, 0.1, 0.3, 0.5, 0.5, 0.5, 0.7, 0.9, 0.9 -> median is 0.5
    assert!(
        (result - 0.5).abs() < 1e-6,
        "Duplicates test: expected 0.5, got {}",
        result
    );
}

#[test]
fn median9_scalar_extreme_values() {
    // Test with extreme values
    let result = median9_scalar(f32::MIN, f32::MAX, 0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0);
    // Sorted: MIN, 0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, MAX -> median is 3.0
    assert!(
        (result - 3.0).abs() < 1e-6,
        "Extreme values test: expected 3.0, got {}",
        result
    );
}
