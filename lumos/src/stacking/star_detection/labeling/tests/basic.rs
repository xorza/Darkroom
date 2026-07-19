use crate::stacking::star_detection::labeling::tests::*;

#[test]
fn empty_mask() {
    let mask = BitBuffer2::from_slice(4, 4, &[false; 16]);
    let label_map = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);

    assert_eq!(label_map.num_labels(), 0);
    assert!(label_map.labels().iter().all(|&l| l == 0));
}

#[test]
fn single_pixel() {
    // 4x4 mask with single pixel at (1, 1)
    let mut mask_data = vec![false; 16];
    mask_data[1 * 4 + 1] = true;
    let mask = BitBuffer2::from_slice(4, 4, &mask_data);

    let label_map = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);

    assert_eq!(label_map.num_labels(), 1);
    assert_eq!(label_map[1 * 4 + 1], 1);
    assert_eq!(label_map.labels().iter().filter(|&&l| l == 1).count(), 1);
}

#[test]
fn horizontal_line() {
    // 5x3 mask with horizontal line in middle row
    // .....
    // .....
    let mut mask_data = vec![false; 15];
    mask_data[1 * 5 + 0] = true;
    mask_data[1 * 5 + 1] = true;
    mask_data[1 * 5 + 2] = true;
    let mask = BitBuffer2::from_slice(5, 3, &mask_data);

    let label_map = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);

    assert_eq!(label_map.num_labels(), 1);
    // All three pixels should have the same label
    let label = label_map[1 * 5 + 0];
    assert!(label > 0);
    assert_eq!(label_map[1 * 5 + 1], label);
    assert_eq!(label_map[1 * 5 + 2], label);
}

#[test]
fn vertical_line() {
    // 3x5 mask with vertical line in middle column
    let mut mask_data = vec![false; 15];
    mask_data[0 * 3 + 1] = true;
    mask_data[1 * 3 + 1] = true;
    mask_data[2 * 3 + 1] = true;
    mask_data[3 * 3 + 1] = true;
    let mask = BitBuffer2::from_slice(3, 5, &mask_data);

    let label_map = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);

    assert_eq!(label_map.num_labels(), 1);
    let label = label_map[0 * 3 + 1];
    assert!(label > 0);
    assert_eq!(label_map[1 * 3 + 1], label);
    assert_eq!(label_map[2 * 3 + 1], label);
    assert_eq!(label_map[3 * 3 + 1], label);
}

#[test]
fn two_separate_regions() {
    // 6x3 mask with two separate single pixels
    // #....#
    // ......
    // ......
    let mut mask_data = vec![false; 18];
    mask_data[0] = true; // (0, 0)
    mask_data[5] = true; // (5, 0)
    let mask = BitBuffer2::from_slice(6, 3, &mask_data);

    let label_map = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);

    assert_eq!(label_map.num_labels(), 2);
    assert!(label_map[0] > 0);
    assert!(label_map[5] > 0);
    assert_ne!(label_map[0], label_map[5]);
}

#[test]
fn l_shape() {
    // 4x4 mask with L-shape
    // #...
    // #...
    // ##..
    // ....
    let mut mask_data = vec![false; 16];
    mask_data[0 * 4 + 0] = true;
    mask_data[1 * 4 + 0] = true;
    mask_data[2 * 4 + 0] = true;
    mask_data[2 * 4 + 1] = true;
    let mask = BitBuffer2::from_slice(4, 4, &mask_data);

    let label_map = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);

    assert_eq!(label_map.num_labels(), 1);
    let label = label_map[0];
    assert!(label > 0);
    assert_eq!(label_map[1 * 4 + 0], label);
    assert_eq!(label_map[2 * 4 + 0], label);
    assert_eq!(label_map[2 * 4 + 1], label);
}

#[test]
fn diagonal_not_connected() {
    // 3x3 mask with diagonal pixels (4-connectivity means they're separate)
    // #..
    // .#.
    // ..#
    let mut mask_data = vec![false; 9];
    mask_data[0 * 3 + 0] = true;
    mask_data[1 * 3 + 1] = true;
    mask_data[2 * 3 + 2] = true;
    let mask = BitBuffer2::from_slice(3, 3, &mask_data);

    let label_map = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);

    // With 4-connectivity, diagonal pixels are NOT connected
    assert_eq!(label_map.num_labels(), 3);
}

#[test]
fn u_shape_union_find() {
    // This tests the union-find when labels need to be merged
    // 5x3 mask forming a U shape:
    // #...#
    // #...#
    let mut mask_data = vec![false; 15];
    // Left column
    mask_data[0 * 5 + 0] = true;
    mask_data[1 * 5 + 0] = true;
    mask_data[2 * 5 + 0] = true;
    // Right column
    mask_data[0 * 5 + 4] = true;
    mask_data[1 * 5 + 4] = true;
    mask_data[2 * 5 + 4] = true;
    // Bottom row connecting them
    mask_data[2 * 5 + 1] = true;
    mask_data[2 * 5 + 2] = true;
    mask_data[2 * 5 + 3] = true;
    let mask = BitBuffer2::from_slice(5, 3, &mask_data);

    let label_map = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);

    // All pixels should be in one component due to union-find
    assert_eq!(label_map.num_labels(), 1);
    let label = label_map[0];
    assert!(label > 0);
    for i in 0..15 {
        if mask_data[i] {
            assert_eq!(
                label_map[i], label,
                "All U-shape pixels should have same label"
            );
        }
    }
}

#[test]
fn checkerboard() {
    // 4x4 checkerboard pattern - no pixels should be connected
    // #.#.
    // .#.#
    // #.#.
    // .#.#
    let mut mask_data = vec![false; 16];
    for y in 0..4 {
        for x in 0..4 {
            if (x + y) % 2 == 0 {
                mask_data[y * 4 + x] = true;
            }
        }
    }
    let mask = BitBuffer2::from_slice(4, 4, &mask_data);

    let label_map = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);

    // Each pixel is isolated (4-connectivity)
    assert_eq!(label_map.num_labels(), 8);
}

#[test]
fn filled_rectangle() {
    // 3x3 all true
    let mask = BitBuffer2::from_slice(3, 3, &[true; 9]);
    let label_map = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);

    assert_eq!(label_map.num_labels(), 1);
    assert!(label_map.labels().iter().all(|&l| l == 1));
}

#[test]
fn labels_are_sequential() {
    // 6x1 with three separate pixels
    // #.#.#.
    let mut mask_data = vec![false; 6];
    mask_data[0] = true;
    mask_data[2] = true;
    mask_data[4] = true;
    let mask = BitBuffer2::from_slice(6, 1, &mask_data);

    let label_map = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);

    assert_eq!(label_map.num_labels(), 3);
    // Labels should be 1, 2, 3 (sequential)
    assert_eq!(label_map[0], 1);
    assert_eq!(label_map[2], 2);
    assert_eq!(label_map[4], 3);
}

#[test]
fn zero_dimensions() {
    // Zero width
    let mask = BitBuffer2::from_slice(0, 10, &[]);
    let label_map = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);
    assert_eq!(label_map.num_labels(), 0);

    // Zero height
    let mask = BitBuffer2::from_slice(10, 0, &[]);
    let label_map = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);
    assert_eq!(label_map.num_labels(), 0);
}

#[test]
fn edge_touching_components() {
    // Components touching all four edges
    let width = 10;
    let height = 10;
    let mut mask_data = vec![false; width * height];

    // Top edge component
    mask_data[0] = true;
    mask_data[1] = true;
    // Bottom edge component
    mask_data[9 * width + 8] = true;
    mask_data[9 * width + 9] = true;
    // Left edge component
    mask_data[5 * width] = true;
    // Right edge component
    mask_data[3 * width + 9] = true;

    let mask = BitBuffer2::from_slice(width, height, &mask_data);
    let label_map = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);

    assert_eq!(label_map.num_labels(), 4);
}
