use crate::stacking::star_detection::labeling::tests::*;

#[test]
fn long_horizontal_run() {
    // Long horizontal run that spans multiple 64-bit words
    let width = 200;
    let height = 5;
    let mut mask_data = vec![false; width * height];

    // Run from x=10 to x=190 (180 pixels, spans 3 words)
    for x in 10..190 {
        mask_data[2 * width + x] = true;
    }

    let mask = BitBuffer2::from_slice(width, height, &mask_data);
    let label_map = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);

    assert_eq!(label_map.num_labels(), 1);
    let label = label_map[2 * width + 10];
    for x in 10..190 {
        assert_eq!(
            label_map[2 * width + x],
            label,
            "All pixels in run should have same label"
        );
    }
}

#[test]
fn multiple_runs_per_row() {
    // Multiple separate runs in a single row
    let width = 100;
    let height = 3;
    let mut mask_data = vec![false; width * height];

    // Row 1: three separate runs
    // Run 1: x=5-15
    for x in 5..15 {
        mask_data[1 * width + x] = true;
    }
    // Run 2: x=30-40
    for x in 30..40 {
        mask_data[1 * width + x] = true;
    }
    // Run 3: x=60-70
    for x in 60..70 {
        mask_data[1 * width + x] = true;
    }

    let mask = BitBuffer2::from_slice(width, height, &mask_data);
    let label_map = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);

    assert_eq!(label_map.num_labels(), 3);

    // Each run should have distinct labels
    let label1 = label_map[1 * width + 5];
    let label2 = label_map[1 * width + 30];
    let label3 = label_map[1 * width + 60];

    assert!(label1 > 0);
    assert!(label2 > 0);
    assert!(label3 > 0);
    assert_ne!(label1, label2);
    assert_ne!(label1, label3);
    assert_ne!(label2, label3);
}

#[test]
fn runs_merging_vertically() {
    // Two runs in adjacent rows that overlap
    let width = 50;
    let height = 4;
    let mut mask_data = vec![false; width * height];

    // Row 1: x=10-30
    for x in 10..30 {
        mask_data[1 * width + x] = true;
    }
    // Row 2: x=20-40 (overlaps with row 1 at x=20-30)
    for x in 20..40 {
        mask_data[2 * width + x] = true;
    }

    let mask = BitBuffer2::from_slice(width, height, &mask_data);
    let label_map = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);

    // Should be single component due to vertical overlap
    assert_eq!(label_map.num_labels(), 1);

    let label = label_map[1 * width + 10];
    for x in 10..30 {
        assert_eq!(label_map[1 * width + x], label);
    }
    for x in 20..40 {
        assert_eq!(label_map[2 * width + x], label);
    }
}

#[test]
fn runs_not_merging_no_overlap() {
    // Two runs in adjacent rows that don't overlap
    let width = 50;
    let height = 4;
    let mut mask_data = vec![false; width * height];

    // Row 1: x=5-15
    for x in 5..15 {
        mask_data[1 * width + x] = true;
    }
    // Row 2: x=25-35 (no overlap with row 1)
    for x in 25..35 {
        mask_data[2 * width + x] = true;
    }

    let mask = BitBuffer2::from_slice(width, height, &mask_data);
    let label_map = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);

    // Should be two separate components
    assert_eq!(label_map.num_labels(), 2);
    assert_ne!(label_map[1 * width + 5], label_map[2 * width + 25]);
}

#[test]
fn complex_run_pattern() {
    // Complex pattern with multiple runs merging via intermediate rows
    // Row 0: ###........###
    // Row 1: ..###..###....
    // Row 2: ....####......
    let width = 14;
    let height = 3;
    let mut mask_data = vec![false; width * height];

    // Row 0
    for x in 0..3 {
        mask_data[0 * width + x] = true;
    }
    for x in 11..14 {
        mask_data[0 * width + x] = true;
    }
    // Row 1
    for x in 2..5 {
        mask_data[1 * width + x] = true;
    }
    for x in 7..10 {
        mask_data[1 * width + x] = true;
    }
    // Row 2
    for x in 4..8 {
        mask_data[2 * width + x] = true;
    }

    let mask = BitBuffer2::from_slice(width, height, &mask_data);
    let label_map = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);

    // Row 0 left connects to row 1 left (overlap at x=2)
    // Row 1 left connects to row 2 (overlap at x=4)
    // Row 1 right connects to row 2 (overlap at x=7)
    // Row 0 right is isolated (no overlap with row 1)
    assert_eq!(label_map.num_labels(), 2);

    // Check the connected component
    let connected_label = label_map[0 * width + 0];
    assert_eq!(label_map[1 * width + 2], connected_label);
    assert_eq!(label_map[2 * width + 4], connected_label);
    assert_eq!(label_map[1 * width + 7], connected_label);

    // Check the isolated component
    let isolated_label = label_map[0 * width + 11];
    assert_ne!(isolated_label, connected_label);
}

#[test]
fn all_ones_word() {
    // Test word that is all 1s (0xFFFFFFFFFFFFFFFF)
    // RLE should create one long run for this
    let width = 128; // 2 full words
    let height = 3;
    let mut mask_data = vec![false; width * height];

    // Fill entire middle row
    for x in 0..width {
        mask_data[1 * width + x] = true;
    }

    let mask = BitBuffer2::from_slice(width, height, &mask_data);
    let label_map = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);

    assert_eq!(label_map.num_labels(), 1);
    let label = label_map[1 * width + 0];
    for x in 0..width {
        assert_eq!(label_map[1 * width + x], label);
    }
}

#[test]
fn alternating_bits_in_word() {
    // Pattern: 101010... which creates many tiny runs
    let width = 64;
    let height = 3;
    let mut mask_data = vec![false; width * height];

    // Alternating pattern in middle row
    for x in (0..width).step_by(2) {
        mask_data[1 * width + x] = true;
    }

    let mask = BitBuffer2::from_slice(width, height, &mask_data);
    let label_map = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);

    // Each pixel is isolated (4-connectivity)
    assert_eq!(label_map.num_labels(), 32);
}

#[test]
fn strip_boundary_run_overlap() {
    // Test that runs at strip boundaries merge correctly
    // This specifically tests the RLE boundary merging logic
    let width = 400;
    let height = 300;
    let mut mask_data = vec![false; width * height];

    // Create a wide horizontal band that spans strip boundaries
    // With 64-row strips, boundaries are at y=64, 128, 192, etc.
    for y in 60..70 {
        // Spans boundary at y=64
        for x in 100..200 {
            mask_data[y * width + x] = true;
        }
    }

    let mask = BitBuffer2::from_slice(width, height, &mask_data);
    let label_map = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);

    assert_eq!(label_map.num_labels(), 1);

    let label = label_map[60 * width + 100];
    for y in 60..70 {
        for x in 100..200 {
            assert_eq!(
                label_map[y * width + x],
                label,
                "All pixels should have same label at ({}, {})",
                x,
                y
            );
        }
    }
}

#[test]
fn empty_rows_between_runs() {
    // Test runs separated by empty rows
    let width = 50;
    let height = 10;
    let mut mask_data = vec![false; width * height];

    // Run at row 2
    for x in 10..20 {
        mask_data[2 * width + x] = true;
    }
    // Run at row 7 (same x range but separated by empty rows)
    for x in 10..20 {
        mask_data[7 * width + x] = true;
    }

    let mask = BitBuffer2::from_slice(width, height, &mask_data);
    let label_map = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);

    // Should be two separate components
    assert_eq!(label_map.num_labels(), 2);
}
