//! Tests for 3x3 median filter.

use crate::stacking::star_detection::median_filter::*;
use crate::testing::test_rng::TestRng;

#[test]
fn single_hot_pixel() {
    // 5x5 image with a hot pixel in center
    let mut pixels = Buffer2::new_filled(5, 5, 0.1f32);
    pixels[(2, 2)] = 1.0; // Center pixel

    let mut output = Buffer2::new_default(5, 5);
    median_filter_3x3(&pixels, &mut output);

    // Hot pixel should be replaced with median of neighbors (0.1)
    assert!(
        (output[(2, 2)] - 0.1).abs() < 1e-6,
        "Hot pixel should be filtered to 0.1, got {}",
        output[(2, 2)]
    );
}

#[test]
fn gradient_image() {
    // 10x10 gradient: pixel(x,y) = (x+y)/20.0
    let width = 10;
    let height = 10;
    let data: Vec<f32> = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x + y) as f32 / 20.0))
        .collect();
    let pixels = Buffer2::new(width, height, data);

    let mut output = Buffer2::new_default(width, height);
    median_filter_3x3(&pixels, &mut output);

    // Interior pixel (3,3): 3x3 neighborhood values = (x+y)/20 for
    // x in 2..=4, y in 2..=4:
    //   (2+2)/20=0.20, (3+2)/20=0.25, (4+2)/20=0.30,
    //   (2+3)/20=0.25, (3+3)/20=0.30, (4+3)/20=0.35,
    //   (2+4)/20=0.30, (3+4)/20=0.35, (4+4)/20=0.40
    // Sorted: [0.20, 0.25, 0.25, 0.30, 0.30, 0.30, 0.35, 0.35, 0.40]
    // Median = 0.30
    assert!(
        (output[(3, 3)] - 0.30).abs() < 1e-6,
        "Interior (3,3) median should be 0.30, got {}",
        output[(3, 3)]
    );

    // Interior pixel (5,5): neighborhood x in 4..=6, y in 4..=6:
    //   (4+4)/20=0.40, (5+4)/20=0.45, (6+4)/20=0.50,
    //   (4+5)/20=0.45, (5+5)/20=0.50, (6+5)/20=0.55,
    //   (4+6)/20=0.50, (5+6)/20=0.55, (6+6)/20=0.60
    // Sorted: [0.40, 0.45, 0.45, 0.50, 0.50, 0.50, 0.55, 0.55, 0.60]
    // Median = 0.50
    assert!(
        (output[(5, 5)] - 0.50).abs() < 1e-6,
        "Interior (5,5) median should be 0.50, got {}",
        output[(5, 5)]
    );
}

#[test]
fn small_image_2x2() {
    let pixels = Buffer2::new(2, 2, vec![0.1, 0.2, 0.3, 0.4]);
    let mut output = Buffer2::new_default(2, 2);
    median_filter_3x3(&pixels, &mut output);

    // Image too small for 3×3 filter, should return exact copy
    assert_eq!(output[(0, 0)], 0.1);
    assert_eq!(output[(1, 0)], 0.2);
    assert_eq!(output[(0, 1)], 0.3);
    assert_eq!(output[(1, 1)], 0.4);
}

#[test]
fn small_image_1x1() {
    let pixels = Buffer2::new(1, 1, vec![0.5]);
    let mut output = Buffer2::new_default(1, 1);
    median_filter_3x3(&pixels, &mut output);

    assert_eq!(output.len(), 1);
    assert!((output[0] - 0.5).abs() < 1e-6);
}

#[test]
fn filters_a_3x3_image() {
    // Exactly 3x3 - each pixel has different neighborhood size
    #[rustfmt::skip]
    let pixels = Buffer2::new(3, 3, vec![
        0.1, 0.2, 0.3,
        0.4, 0.5, 0.6,
        0.7, 0.8, 0.9,
    ]);

    let mut output = Buffer2::new_default(3, 3);
    median_filter_3x3(&pixels, &mut output);

    // Center pixel has full 9-element neighborhood
    // Median of [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9] = 0.5
    assert!(
        (output[(1, 1)] - 0.5).abs() < 1e-6,
        "Center median should be 0.5, got {}",
        output[(1, 1)]
    );
}

#[test]
fn corner_pixels() {
    // 4x4 image
    #[rustfmt::skip]
    let pixels = Buffer2::new(4, 4, vec![
        0.1, 0.2, 0.3, 0.4,
        0.5, 0.6, 0.7, 0.8,
        0.9, 1.0, 1.1, 1.2,
        1.3, 1.4, 1.5, 1.6,
    ]);

    let mut output = Buffer2::new_default(4, 4);
    median_filter_3x3(&pixels, &mut output);

    // Top-left corner has 4 neighbors: [0.1, 0.2, 0.5, 0.6]
    // Median of 4 = average of middle two = (0.2 + 0.5) / 2 = 0.35
    assert!(
        (output[(0, 0)] - 0.35).abs() < 1e-6,
        "Top-left corner median should be 0.35, got {}",
        output[(0, 0)]
    );
}

#[test]
fn edge_pixels() {
    // 4x4 image
    #[rustfmt::skip]
    let pixels = Buffer2::new(4, 4, vec![
        0.1, 0.2, 0.3, 0.4,
        0.5, 0.6, 0.7, 0.8,
        0.9, 1.0, 1.1, 1.2,
        1.3, 1.4, 1.5, 1.6,
    ]);

    let mut output = Buffer2::new_default(4, 4);
    median_filter_3x3(&pixels, &mut output);

    // Top edge (1,0) has 6 neighbors: [0.1, 0.2, 0.3, 0.5, 0.6, 0.7]
    // Sorted: [0.1, 0.2, 0.3, 0.5, 0.6, 0.7]
    // Median of 6 = average of middle two = (0.3 + 0.5) / 2 = 0.4
    assert!(
        (output[(1, 0)] - 0.4).abs() < 1e-6,
        "Top edge median should be 0.4, got {}",
        output[(1, 0)]
    );
}

#[test]
fn salt_and_pepper_noise() {
    // Image with salt and pepper noise
    let mut pixels = Buffer2::new_filled(10, 10, 0.5f32);
    // Add noise
    pixels[23] = 0.0; // pepper
    pixels[45] = 1.0; // salt
    pixels[67] = 0.0; // pepper
    pixels[89] = 1.0; // salt

    let mut output = Buffer2::new_default(10, 10);
    median_filter_3x3(&pixels, &mut output);

    // All noisy pixels should be close to 0.5 after filtering
    assert!(
        (output[23] - 0.5).abs() < 0.1,
        "Pepper noise should be filtered"
    );
    assert!(
        (output[45] - 0.5).abs() < 0.1,
        "Salt noise should be filtered"
    );
    assert!(
        (output[67] - 0.5).abs() < 0.1,
        "Pepper noise should be filtered"
    );
    assert!(
        (output[89] - 0.5).abs() < 0.1,
        "Salt noise should be filtered"
    );
}

#[test]
fn median4_averages_the_two_middle_values() {
    let mut v = [0.4, 0.1, 0.3, 0.2];
    assert_eq!(median4(&mut v), f32::midpoint(0.2, 0.3));
}

/// Every order of six distinct values — 720 of them — and every pattern of two values: the
/// network must sort all, not just the orders a hand-picked case happens to take.
#[test]
fn median_networks_sort_every_order() {
    fn orders(n: usize) -> Vec<Vec<f32>> {
        let mut all: Vec<Vec<f32>> = vec![Vec::new()];
        for _ in 0..n {
            let mut longer = Vec::new();
            for prefix in &all {
                for v in (0..n).map(|v| v as f32).filter(|v| !prefix.contains(v)) {
                    let mut next = prefix.clone();
                    next.push(v);
                    longer.push(next);
                }
            }
            all = longer;
        }
        all
    }
    for order in orders(6) {
        assert_eq!(median6(&mut order.clone()), 2.5, "{order:?}");
    }
    for order in orders(4) {
        assert_eq!(median4(&mut order.clone()), 1.5, "{order:?}");
    }
    for bits in 0..64u32 {
        let mut v: Vec<f32> = (0..6).map(|i| ((bits >> i) & 1) as f32).collect();
        let mut sorted = v.clone();
        sorted.sort_by(f32::total_cmp);
        assert_eq!(
            median6(&mut v),
            f32::midpoint(sorted[2], sorted[3]),
            "{bits:06b}"
        );
    }
}

#[test]
fn median6_averages_the_two_middle_values() {
    let mut v = [0.6, 0.1, 0.5, 0.2, 0.4, 0.3];
    assert_eq!(median6(&mut v), f32::midpoint(0.3, 0.4));
}

#[test]
#[should_panic(expected = "pixels length must equal width * height")]
fn wrong_pixel_count() {
    let pixels = Buffer2::new(20, 10, vec![0.5f32; 100]); // Expects 200 pixels
    let mut output = Buffer2::new_default(20, 10);
    median_filter_3x3(&pixels, &mut output);
}

#[test]
fn bayer_pattern_removal() {
    // Alternating row brightness: even rows = 0.4, odd rows = 0.6
    let width = 10;
    let height = 10;
    let data: Vec<f32> = (0..height)
        .flat_map(|y| {
            let base = if y % 2 == 0 { 0.4 } else { 0.6 };
            (0..width).map(move |_| base)
        })
        .collect();
    let pixels = Buffer2::new(width, height, data);

    let mut output = Buffer2::new_default(width, height);
    median_filter_3x3(&pixels, &mut output);

    // Interior pixel at (5,3) — odd row (y=3, value=0.6).
    // 3×3 neighborhood rows: y=2 (0.4), y=3 (0.6), y=4 (0.4)
    // 9 values: [0.4, 0.4, 0.4, 0.6, 0.6, 0.6, 0.4, 0.4, 0.4]
    // Sorted: [0.4]*6, [0.6]*3 → median = 0.4
    assert!(
        (output[(5, 3)] - 0.4).abs() < 1e-6,
        "Interior odd-row pixel should be 0.4, got {}",
        output[(5, 3)]
    );

    // Interior pixel at (5,4) — even row (y=4, value=0.4).
    // 3×3 neighborhood rows: y=3 (0.6), y=4 (0.4), y=5 (0.6)
    // 9 values: [0.6, 0.6, 0.6, 0.4, 0.4, 0.4, 0.6, 0.6, 0.6]
    // Sorted: [0.4]*3, [0.6]*6 → median = 0.6
    assert!(
        (output[(5, 4)] - 0.6).abs() < 1e-6,
        "Interior even-row pixel should be 0.6, got {}",
        output[(5, 4)]
    );
}

#[test]
fn median_at_side_edges() {
    #[rustfmt::skip]
    let pixels = vec![
        0.1, 0.2, 0.3, 0.4, 0.5,
        0.6, 0.7, 0.8, 0.9, 1.0,
        1.1, 1.2, 1.3, 1.4, 1.5,
        1.6, 1.7, 1.8, 1.9, 2.0,
        2.1, 2.2, 2.3, 2.4, 2.5,
    ];

    let left = median_at_left_edge(&pixels, 5, 1);
    assert!(
        (left - 0.65).abs() < 1e-6,
        "left edge median should be 0.65, got {left}"
    );
    let right = median_at_right_edge(&pixels, 5, 1);
    assert!(
        (right - 0.95).abs() < 1e-6,
        "right edge median should be 0.95, got {right}"
    );
}

#[test]
fn median_at_edge_truth_table() {
    #[derive(Debug)]
    struct EdgeMedianCase {
        pos: Vec2us,
        expected: f32,
    }

    #[rustfmt::skip]
    let pixels = vec![
        0.1, 0.2, 0.3, 0.4,
        0.5, 0.6, 0.7, 0.8,
        0.9, 1.0, 1.1, 1.2,
        1.3, 1.4, 1.5, 1.6,
    ];
    let cases = [
        EdgeMedianCase {
            pos: Vec2us::new(0, 0),
            expected: 0.35,
        },
        EdgeMedianCase {
            pos: Vec2us::new(3, 0),
            expected: 0.55,
        },
        EdgeMedianCase {
            pos: Vec2us::new(0, 3),
            expected: 1.15,
        },
        EdgeMedianCase {
            pos: Vec2us::new(3, 3),
            expected: 1.35,
        },
        EdgeMedianCase {
            pos: Vec2us::new(1, 0),
            expected: 0.4,
        },
        EdgeMedianCase {
            pos: Vec2us::new(1, 3),
            expected: 1.2,
        },
    ];

    for case in cases {
        let actual = median_at_edge(&pixels, Size2us::new(4, 4), case.pos);
        assert!((actual - case.expected).abs() < 1e-6, "{case:?}");
    }
}

#[test]
fn interior_row_replaces_a_hot_pixel_with_its_neighbourhood_median() {
    // 5x5 image with known values
    #[rustfmt::skip]
    let pixels = vec![
        0.1, 0.1, 0.1, 0.1, 0.1,
        0.1, 0.5, 0.5, 0.5, 0.1,
        0.1, 0.5, 1.0, 0.5, 0.1,  // Hot pixel in center
        0.1, 0.5, 0.5, 0.5, 0.1,
        0.1, 0.1, 0.1, 0.1, 0.1,
    ];

    let mut output_row = vec![0.0f32; 5];
    filter_interior_row(&pixels, 5, 2, &mut output_row);

    // Center pixel (x=2) should be filtered
    // Neighborhood: [0.5, 0.5, 0.5, 0.5, 1.0, 0.5, 0.5, 0.5, 0.5]
    // Median = 0.5
    assert!(
        (output_row[2] - 0.5).abs() < 1e-6,
        "Interior row center should be 0.5, got {}",
        output_row[2]
    );
}

#[test]
fn filter_edge_row_top() {
    // 5x5 image
    #[rustfmt::skip]
    let pixels = vec![
        0.1, 0.2, 0.3, 0.4, 0.5,
        0.6, 0.7, 0.8, 0.9, 1.0,
        1.1, 1.2, 1.3, 1.4, 1.5,
        1.6, 1.7, 1.8, 1.9, 2.0,
        2.1, 2.2, 2.3, 2.4, 2.5,
    ];

    let mut output_row = vec![0.0f32; 5];
    filter_edge_row(&pixels, Size2us::new(5, 5), 0, &mut output_row);

    // Check first pixel (corner)
    // Neighbors: [0.1, 0.2, 0.6, 0.7] -> median = (0.2 + 0.6) / 2 = 0.4
    assert!(
        (output_row[0] - 0.4).abs() < 1e-6,
        "Top-left should be 0.4, got {}",
        output_row[0]
    );
}

#[test]
fn filter_edge_row_bottom() {
    // 5x5 image
    #[rustfmt::skip]
    let pixels = vec![
        0.1, 0.2, 0.3, 0.4, 0.5,
        0.6, 0.7, 0.8, 0.9, 1.0,
        1.1, 1.2, 1.3, 1.4, 1.5,
        1.6, 1.7, 1.8, 1.9, 2.0,
        2.1, 2.2, 2.3, 2.4, 2.5,
    ];

    let mut output_row = vec![0.0f32; 5];
    filter_edge_row(&pixels, Size2us::new(5, 5), 4, &mut output_row);

    // Check last pixel (corner)
    // Neighbors: [1.9, 2.0, 2.4, 2.5] -> median = (2.0 + 2.4) / 2 = 2.2
    assert!(
        (output_row[4] - 2.2).abs() < 1e-6,
        "Bottom-right should be 2.2, got {}",
        output_row[4]
    );
}

#[test]
fn filters_the_interior_of_a_4x4_image() {
    // 4x4 image: pixel(x,y) = y*4 + x + 1
    #[rustfmt::skip]
    let pixels = Buffer2::new(4, 4, vec![
        1.0,  2.0,  3.0,  4.0,
        5.0,  6.0,  7.0,  8.0,
        9.0,  10.0, 11.0, 12.0,
        13.0, 14.0, 15.0, 16.0,
    ]);

    let mut output = Buffer2::new_default(4, 4);
    median_filter_3x3(&pixels, &mut output);

    // Interior (1,1): neighbors [1,2,3,5,6,7,9,10,11] → median = 6.0
    assert!(
        (output[(1, 1)] - 6.0).abs() < 1e-6,
        "Interior (1,1) should be 6.0, got {}",
        output[(1, 1)]
    );
    // Interior (2,2): neighbors [6,7,8,10,11,12,14,15,16] → median = 11.0
    assert!(
        (output[(2, 2)] - 11.0).abs() < 1e-6,
        "Interior (2,2) should be 11.0, got {}",
        output[(2, 2)]
    );
    // Interior (1,2): neighbors [5,6,7,9,10,11,13,14,15] → median = 10.0
    assert!(
        (output[(1, 2)] - 10.0).abs() < 1e-6,
        "Interior (1,2) should be 10.0, got {}",
        output[(1, 2)]
    );
    // Interior (2,1): neighbors [2,3,4,6,7,8,10,11,12] → median = 7.0
    assert!(
        (output[(2, 1)] - 7.0).abs() < 1e-6,
        "Interior (2,1) should be 7.0, got {}",
        output[(2, 1)]
    );
}

#[test]
fn median_with_duplicates() {
    // Test median functions with duplicate values
    let mut v4 = [0.3, 0.3, 0.7, 0.7];
    assert!((median4(&mut v4) - 0.5).abs() < 1e-6);

    let mut v6 = [0.1, 0.1, 0.5, 0.5, 0.9, 0.9];
    assert!((median6(&mut v6) - 0.5).abs() < 1e-6);
}

/// The median of the in-frame 3×3 neighbourhood by sorting it: 9 values inside, 6 on an edge, 4
/// at a corner, the two middle ones averaged when even.
fn reference_median(pixels: &Buffer2<f32>, x: usize, y: usize) -> f32 {
    let mut values: Vec<f32> = (y.saturating_sub(1)..(y + 2).min(pixels.height()))
        .flat_map(|ny| (x.saturating_sub(1)..(x + 2).min(pixels.width())).map(move |nx| (nx, ny)))
        .map(|(nx, ny)| pixels[(nx, ny)])
        .collect();
    values.sort_by(f32::total_cmp);
    let mid = values.len() / 2;
    if values.len() % 2 == 1 {
        values[mid]
    } else {
        f32::midpoint(values[mid - 1], values[mid])
    }
}

/// Every pixel equals the sorted reference, bit for bit, on random fields whose rows all differ —
/// so a row read from the wrong offset, at a SIMD remainder or where the parallel split cuts the
/// rows, changes the answer. The sizes cover the 3×3 minimum, odd widths past every lane count,
/// one-row-thick interiors both ways, and frames tall enough to split across threads.
#[test]
fn matches_a_sorted_reference_everywhere() {
    for (width, height) in [
        (3, 3),
        (4, 4),
        (5, 7),
        (17, 9),
        (33, 3),
        (3, 33),
        (100, 4),
        (4, 100),
        (130, 67),
        (1031, 37),
    ] {
        let mut rng = TestRng::new((width * 1000 + height) as u64);
        let pixels = Buffer2::new(
            width,
            height,
            (0..width * height).map(|_| rng.next_f32()).collect(),
        );
        let mut output = Buffer2::new_default(width, height);
        median_filter_3x3(&pixels, &mut output);
        for y in 0..height {
            for x in 0..width {
                assert_eq!(
                    output[(x, y)],
                    reference_median(&pixels, x, y),
                    "{width}×{height} at ({x}, {y})"
                );
            }
        }
    }
}
