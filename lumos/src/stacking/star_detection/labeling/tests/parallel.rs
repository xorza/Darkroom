use crate::stacking::star_detection::labeling::tests::*;

#[test]
fn large_image_parallel_path() {
    // Large image to trigger parallel code path (>100k pixels)
    let width = 400;
    let height = 300;
    let mut mask_data = vec![false; width * height];

    // Create several separate components
    // Component 1: top-left corner
    for y in 10..20 {
        for x in 10..20 {
            mask_data[y * width + x] = true;
        }
    }
    // Component 2: bottom-right corner
    for y in 250..260 {
        for x in 350..360 {
            mask_data[y * width + x] = true;
        }
    }
    // Component 3: center
    for y in 145..155 {
        for x in 195..205 {
            mask_data[y * width + x] = true;
        }
    }

    let mask = BitBuffer2::from_slice(width, height, &mask_data);
    let label_map = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);

    assert_eq!(label_map.num_labels(), 3);

    // Verify each component has consistent labels
    let label1 = label_map[10 * width + 10];
    let label2 = label_map[250 * width + 350];
    let label3 = label_map[145 * width + 195];

    assert!(label1 > 0);
    assert!(label2 > 0);
    assert!(label3 > 0);
    assert_ne!(label1, label2);
    assert_ne!(label1, label3);
    assert_ne!(label2, label3);

    // Check all pixels in component 1
    for y in 10..20 {
        for x in 10..20 {
            assert_eq!(label_map[y * width + x], label1);
        }
    }
}

#[test]
fn dense_mask_does_not_overflow_atomic_uf() {
    // Regression: the parallel union-find capacity scales with the foreground count, not a
    // fixed 5%-of-pixels heuristic. A grid of isolated pixels is one component each — 25% of
    // pixels here — which previously overflowed the atomic union-find (cap = pixels/20) and
    // panicked. With cap = count_ones() it labels cleanly.
    let width = 400;
    let height = 300; // 120_000 px > PARALLEL_CCL_THRESHOLD → parallel path
    let mut mask_data = vec![false; width * height];
    let mut expected = 0;
    for y in (0..height).step_by(2) {
        for x in (0..width).step_by(2) {
            mask_data[y * width + x] = true;
            expected += 1;
        }
    }

    let mask = BitBuffer2::from_slice(width, height, &mask_data);
    let label_map = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);

    // Every isolated pixel is its own component (4-connectivity, 1-px gaps).
    assert_eq!(label_map.num_labels(), expected); // 200 * 150 = 30_000, far above pixels/20
}

#[test]
fn strip_boundary_vertical_line() {
    // Vertical line spanning multiple strips (tests boundary merging)
    let width = 400;
    let height = 300;
    let mut mask_data = vec![false; width * height];

    // Vertical line from y=0 to y=299 at x=200
    for y in 0..height {
        mask_data[y * width + 200] = true;
    }

    let mask = BitBuffer2::from_slice(width, height, &mask_data);
    let label_map = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);

    // Should be single component despite crossing strip boundaries
    assert_eq!(label_map.num_labels(), 1);

    let label = label_map[200]; // First pixel
    for y in 0..height {
        assert_eq!(
            label_map[y * width + 200],
            label,
            "Pixel at y={} should have same label",
            y
        );
    }
}

#[test]
fn strip_boundary_diagonal() {
    // Diagonal line that crosses strip boundaries but should NOT connect
    // (4-connectivity means diagonal pixels are separate)
    let width = 400;
    let height = 300;
    let mut mask_data = vec![false; width * height];

    // Diagonal from (0,0) to (299,299) - one pixel per row
    for i in 0..height.min(width) {
        mask_data[i * width + i] = true;
    }

    let mask = BitBuffer2::from_slice(width, height, &mask_data);
    let label_map = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);

    // Each diagonal pixel should be separate (4-connectivity)
    assert_eq!(label_map.num_labels(), height.min(width));
}

#[test]
fn strip_boundary_u_shape_large() {
    // Large U-shape that spans strip boundaries and requires merging
    let width = 400;
    let height = 300;
    let mut mask_data = vec![false; width * height];

    // Left vertical bar from y=50 to y=250
    for y in 50..250 {
        mask_data[y * width + 100] = true;
    }
    // Right vertical bar from y=50 to y=250
    for y in 50..250 {
        mask_data[y * width + 300] = true;
    }
    // Bottom horizontal bar connecting them at y=249
    for x in 100..=300 {
        mask_data[249 * width + x] = true;
    }

    let mask = BitBuffer2::from_slice(width, height, &mask_data);
    let label_map = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);

    // All should be one component
    assert_eq!(label_map.num_labels(), 1);

    let label = label_map[50 * width + 100];
    // Check some pixels from each part
    assert_eq!(label_map[100 * width + 100], label); // left bar
    assert_eq!(label_map[100 * width + 300], label); // right bar
    assert_eq!(label_map[249 * width + 200], label); // bottom bar
}

#[test]
fn many_small_components() {
    // Many small isolated components (stress test for label allocation)
    let width = 400;
    let height = 300;
    let mut mask_data = vec![false; width * height];

    // Create grid of single pixels, spaced 10 apart
    let mut expected_count = 0;
    for y in (5..height).step_by(10) {
        for x in (5..width).step_by(10) {
            mask_data[y * width + x] = true;
            expected_count += 1;
        }
    }

    let mask = BitBuffer2::from_slice(width, height, &mask_data);
    let label_map = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);

    assert_eq!(label_map.num_labels(), expected_count);
}

#[test]
fn single_row_large() {
    // Single row, wide image (edge case for strip division)
    let width = 500;
    let height = 1;
    let mut mask_data = vec![false; width * height];

    // Three separate components
    mask_data[10..20].fill(true);
    mask_data[100..110].fill(true);
    mask_data[400..410].fill(true);

    let mask = BitBuffer2::from_slice(width, height, &mask_data);
    let label_map = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);

    assert_eq!(label_map.num_labels(), 3);
}

#[test]
fn single_column_large() {
    // Single column, tall image
    let width = 1;
    let height = 500;
    let mut mask_data = vec![false; width * height];

    // Three separate components
    mask_data[10..20].fill(true);
    mask_data[100..110].fill(true);
    mask_data[400..410].fill(true);

    let mask = BitBuffer2::from_slice(width, height, &mask_data);
    let label_map = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);

    assert_eq!(label_map.num_labels(), 3);
}

#[test]
fn component_at_strip_boundary_exact() {
    // Component exactly at strip boundary (64 rows per strip)
    let width = 400;
    let height = 300;
    let mut mask_data = vec![false; width * height];

    // Small square crossing y=64 boundary (rows 62-66)
    for y in 62..67 {
        for x in 100..105 {
            mask_data[y * width + x] = true;
        }
    }

    let mask = BitBuffer2::from_slice(width, height, &mask_data);
    let label_map = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);

    assert_eq!(label_map.num_labels(), 1);

    let label = label_map[62 * width + 100];
    for y in 62..67 {
        for x in 100..105 {
            assert_eq!(label_map[y * width + x], label);
        }
    }
}

#[test]
fn all_pixels_set_large() {
    // Large fully-filled image (worst case for union-find)
    let width = 200;
    let height = 200;
    let mask_data = vec![true; width * height];

    let mask = BitBuffer2::from_slice(width, height, &mask_data);
    let label_map = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);

    assert_eq!(label_map.num_labels(), 1);
    assert!(label_map.labels().iter().all(|&l| l == 1));
}

#[test]
fn compare_sequential_parallel() {
    // Compare results between sequential and parallel paths
    // by testing same pattern at different sizes
    let pattern_test = |width: usize, height: usize| {
        let mut mask_data = vec![false; width * height];

        // Create a pattern: horizontal lines every 10 rows
        for y in (5..height).step_by(10) {
            for x in 10..(width - 10) {
                mask_data[y * width + x] = true;
            }
        }

        let mask = BitBuffer2::from_slice(width, height, &mask_data);
        let label_map = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);

        // Count expected: one component per line
        let expected_lines = (height - 5) / 10 + if (height - 5) % 10 >= 1 { 1 } else { 0 };
        (label_map.num_labels(), expected_lines)
    };

    // Small (sequential path)
    let (small_labels, small_expected) = pattern_test(100, 100);
    // Large (parallel path)
    let (large_labels, large_expected) = pattern_test(400, 300);

    assert_eq!(small_labels, small_expected);
    assert_eq!(large_labels, large_expected);
}

#[test]
fn alternating_rows_large() {
    // Alternating rows pattern that stresses strip boundary merging
    let width = 400;
    let height = 300;
    let mut mask_data = vec![false; width * height];

    // Fill every other row completely
    for y in (0..height).step_by(2) {
        for x in 0..width {
            mask_data[y * width + x] = true;
        }
    }

    let mask = BitBuffer2::from_slice(width, height, &mask_data);
    let label_map = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);

    // Each row is a separate component (no vertical connectivity)
    let expected = height.div_ceil(2);
    assert_eq!(label_map.num_labels(), expected);
}

#[test]
fn sparse_large() {
    // Very sparse: few pixels spread across large image
    let width = 1000;
    let height = 1000;
    let mut mask_data = vec![false; width * height];

    // Only 4 isolated pixels in corners
    mask_data[10 * width + 10] = true;
    mask_data[10 * width + 990] = true;
    mask_data[990 * width + 10] = true;
    mask_data[990 * width + 990] = true;

    let mask = BitBuffer2::from_slice(width, height, &mask_data);
    let label_map = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);

    assert_eq!(label_map.num_labels(), 4);
}
