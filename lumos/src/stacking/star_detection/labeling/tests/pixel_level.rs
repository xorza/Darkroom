use crate::stacking::star_detection::labeling::tests::*;

#[test]
fn exact_labels_simple_grid() {
    // 4x4 grid with known layout:
    // ##..
    // ##..
    // ..##
    // ..##
    // Should produce 2 components
    let mut mask = vec![false; 16];
    mask[0] = true;
    mask[1] = true;
    mask[4] = true;
    mask[5] = true;
    mask[10] = true;
    mask[11] = true;
    mask[14] = true;
    mask[15] = true;

    let bit_mask = BitBuffer2::from_slice(4, 4, &mask);
    let label_map = label_map_from_mask_with_connectivity(&bit_mask, Connectivity::Four);

    assert_eq!(label_map.num_labels(), 2);

    // First component (top-left 2x2)
    let label_a = label_map[0];
    assert!(label_a > 0);
    assert_eq!(label_map[1], label_a);
    assert_eq!(label_map[4], label_a);
    assert_eq!(label_map[5], label_a);

    // Second component (bottom-right 2x2)
    let label_b = label_map[10];
    assert!(label_b > 0);
    assert_ne!(label_b, label_a);
    assert_eq!(label_map[11], label_b);
    assert_eq!(label_map[14], label_b);
    assert_eq!(label_map[15], label_b);

    // Background
    assert_eq!(label_map[2], 0);
    assert_eq!(label_map[3], 0);
    assert_eq!(label_map[8], 0);
    assert_eq!(label_map[9], 0);
}

#[test]
fn exact_labels_diagonal_8conn() {
    // 3x3 diagonal:
    // #..
    // .#.
    // ..#
    let mut mask = vec![false; 9];
    mask[0] = true;
    mask[4] = true;
    mask[8] = true;

    let bit_mask = BitBuffer2::from_slice(3, 3, &mask);

    // 4-connectivity: 3 separate labels
    let label_map_4 = label_map_from_mask_with_connectivity(&bit_mask, Connectivity::Four);
    assert_eq!(label_map_4.num_labels(), 3);
    assert_ne!(label_map_4[0], label_map_4[4]);
    assert_ne!(label_map_4[4], label_map_4[8]);
    assert_ne!(label_map_4[0], label_map_4[8]);

    // 8-connectivity: all same label
    let label_map_8 = label_map_from_mask_with_connectivity(&bit_mask, Connectivity::Eight);
    assert_eq!(label_map_8.num_labels(), 1);
    let label = label_map_8[0];
    assert_eq!(label_map_8[4], label);
    assert_eq!(label_map_8[8], label);
}

#[test]
fn exact_labels_u_shape() {
    // U-shape that requires union-find merging:
    // #.#
    // #.#
    let mut mask = vec![false; 9];
    mask[0] = true;
    mask[2] = true;
    mask[3] = true;
    mask[5] = true;
    mask[6] = true;
    mask[7] = true;
    mask[8] = true;

    let bit_mask = BitBuffer2::from_slice(3, 3, &mask);
    let label_map = label_map_from_mask_with_connectivity(&bit_mask, Connectivity::Four);

    assert_eq!(label_map.num_labels(), 1);

    // All foreground pixels should have the same label
    let label = label_map[0];
    assert!(label > 0);
    for &idx in &[0, 2, 3, 5, 6, 7, 8] {
        assert_eq!(
            label_map[idx], label,
            "Pixel {} should have label {}",
            idx, label
        );
    }

    // Background pixels
    assert_eq!(label_map[1], 0);
    assert_eq!(label_map[4], 0);
}

#[test]
fn exact_labels_cross_pattern() {
    // Cross pattern:
    // .#.
    // .#.
    let mut mask = vec![false; 9];
    mask[1] = true;
    mask[3] = true;
    mask[4] = true;
    mask[5] = true;
    mask[7] = true;

    let bit_mask = BitBuffer2::from_slice(3, 3, &mask);
    let label_map = label_map_from_mask_with_connectivity(&bit_mask, Connectivity::Four);

    assert_eq!(label_map.num_labels(), 1);

    let label = label_map[1];
    assert!(label > 0);
    for &idx in &[1, 3, 4, 5, 7] {
        assert_eq!(label_map[idx], label);
    }

    // Corners are background
    for &idx in &[0, 2, 6, 8] {
        assert_eq!(label_map[idx], 0);
    }
}

#[test]
fn labels_deterministic() {
    // Same input should always produce same output
    let mut mask = vec![false; 100];
    for i in [5, 15, 25, 35, 45, 50, 51, 52, 60, 70, 80, 90] {
        mask[i] = true;
    }

    let bit_mask = BitBuffer2::from_slice(10, 10, &mask);

    let label_map_1 = label_map_from_mask_with_connectivity(&bit_mask, Connectivity::Four);
    let label_map_2 = label_map_from_mask_with_connectivity(&bit_mask, Connectivity::Four);

    assert_eq!(label_map_1.num_labels(), label_map_2.num_labels());
    assert_eq!(label_map_1.labels(), label_map_2.labels());
}
