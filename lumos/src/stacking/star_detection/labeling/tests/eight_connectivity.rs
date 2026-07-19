use crate::stacking::star_detection::labeling::tests::*;

#[test]
fn diagonal_connected() {
    // 3x3 mask with diagonal pixels
    // #..
    // .#.
    // ..#
    // With 4-connectivity: 3 separate components
    // With 8-connectivity: 1 component
    let mut mask_data = vec![false; 9];
    mask_data[0 * 3 + 0] = true;
    mask_data[1 * 3 + 1] = true;
    mask_data[2 * 3 + 2] = true;
    let mask = BitBuffer2::from_slice(3, 3, &mask_data);

    // 4-connectivity: diagonals are separate
    let label_map_4 = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);
    assert_eq!(label_map_4.num_labels(), 3);

    // 8-connectivity: diagonals are connected
    let label_map_8 = label_map_from_mask_with_connectivity(&mask, Connectivity::Eight);
    assert_eq!(label_map_8.num_labels(), 1);
}

#[test]
fn anti_diagonal_connected() {
    // Anti-diagonal
    // ..#
    // .#.
    // #..
    let mut mask_data = vec![false; 9];
    mask_data[0 * 3 + 2] = true;
    mask_data[1 * 3 + 1] = true;
    mask_data[2 * 3 + 0] = true;
    let mask = BitBuffer2::from_slice(3, 3, &mask_data);

    // 4-connectivity: diagonals are separate
    let label_map_4 = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);
    assert_eq!(label_map_4.num_labels(), 3);

    // 8-connectivity: diagonals are connected
    let label_map_8 = label_map_from_mask_with_connectivity(&mask, Connectivity::Eight);
    assert_eq!(label_map_8.num_labels(), 1);
}

#[test]
fn checkerboard_8conn() {
    // 4x4 checkerboard - all connected with 8-connectivity
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

    // 4-connectivity: each pixel is isolated
    let label_map_4 = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);
    assert_eq!(label_map_4.num_labels(), 8);

    // 8-connectivity: all are connected diagonally
    let label_map_8 = label_map_from_mask_with_connectivity(&mask, Connectivity::Eight);
    assert_eq!(label_map_8.num_labels(), 1);
}

#[test]
fn adjacent_runs_diagonal() {
    // Test diagonal adjacency between runs
    // ....###
    // With 4-conn: 2 components (runs don't overlap)
    // With 8-conn: 1 component (runs touch at diagonal: run1.end=3, run2.start=4)
    let width = 7;
    let height = 2;
    let mut mask_data = vec![false; width * height];

    // Row 0: x=0-3
    for x in 0..3 {
        mask_data[0 * width + x] = true;
    }
    // Row 1: x=3-6 (diagonal touch at x=3)
    for x in 3..6 {
        mask_data[1 * width + x] = true;
    }

    let mask = BitBuffer2::from_slice(width, height, &mask_data);

    // 4-connectivity: no overlap (run1 ends at 3, run2 starts at 3)
    // Actually, run1=[0,3), run2=[3,6), so they share x=3? No, run1.end=3 exclusive
    // So run1 covers x=0,1,2 and run2 covers x=3,4,5 - no vertical overlap
    let label_map_4 = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);
    assert_eq!(label_map_4.num_labels(), 2);

    // 8-connectivity: diagonal touch (x=2 in row0 touches x=3 in row1)
    let label_map_8 = label_map_from_mask_with_connectivity(&mask, Connectivity::Eight);
    assert_eq!(label_map_8.num_labels(), 1);
}

#[test]
fn l_shape_diagonal_gap() {
    // L-shape with diagonal gap
    // ##....
    // ..##..
    // ....##
    // With 4-conn: 3 components
    // With 8-conn: 1 component (diagonal chain)
    let width = 6;
    let height = 3;
    let mut mask_data = vec![false; width * height];

    for x in 0..2 {
        mask_data[0 * width + x] = true;
    }
    for x in 2..4 {
        mask_data[1 * width + x] = true;
    }
    for x in 4..6 {
        mask_data[2 * width + x] = true;
    }

    let mask = BitBuffer2::from_slice(width, height, &mask_data);

    let label_map_4 = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);
    assert_eq!(label_map_4.num_labels(), 3);

    let label_map_8 = label_map_from_mask_with_connectivity(&mask, Connectivity::Eight);
    assert_eq!(label_map_8.num_labels(), 1);
}

#[test]
fn parallel_strip_boundary_diagonal() {
    // Test diagonal connectivity at strip boundaries (large image)
    let width = 400;
    let height = 300;
    let mut mask_data = vec![false; width * height];

    // Create a diagonal line that crosses strip boundaries
    // With 64-row strips, boundaries are at y=64, 128, 192
    for i in 0..150 {
        let x = 100 + i;
        let y = 50 + i; // Crosses y=64 boundary diagonally
        if x < width && y < height {
            mask_data[y * width + x] = true;
        }
    }

    let mask = BitBuffer2::from_slice(width, height, &mask_data);

    // 4-connectivity: each pixel is separate
    let label_map_4 = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);
    assert_eq!(label_map_4.num_labels(), 150);

    // 8-connectivity: all pixels form one diagonal line
    let label_map_8 = label_map_from_mask_with_connectivity(&mask, Connectivity::Eight);
    assert_eq!(label_map_8.num_labels(), 1);
}

#[test]
fn corner_touch_only() {
    // Two runs that only touch at corner
    // #.
    // .#
    let mut mask_data = vec![false; 4];
    mask_data[0] = true; // (0,0)
    mask_data[3] = true; // (1,1)
    let mask = BitBuffer2::from_slice(2, 2, &mask_data);

    // 4-conn: separate
    let label_map_4 = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);
    assert_eq!(label_map_4.num_labels(), 2);

    // 8-conn: connected
    let label_map_8 = label_map_from_mask_with_connectivity(&mask, Connectivity::Eight);
    assert_eq!(label_map_8.num_labels(), 1);
}

#[test]
fn horizontal_still_connected() {
    // Verify horizontal connectivity still works with 8-conn
    let mut mask_data = vec![false; 15];
    mask_data[1 * 5 + 0] = true;
    mask_data[1 * 5 + 1] = true;
    mask_data[1 * 5 + 2] = true;
    let mask = BitBuffer2::from_slice(5, 3, &mask_data);

    let label_map_8 = label_map_from_mask_with_connectivity(&mask, Connectivity::Eight);
    assert_eq!(label_map_8.num_labels(), 1);
}

#[test]
fn vertical_still_connected() {
    // Verify vertical connectivity still works with 8-conn
    let mut mask_data = vec![false; 15];
    mask_data[0 * 3 + 1] = true;
    mask_data[1 * 3 + 1] = true;
    mask_data[2 * 3 + 1] = true;
    let mask = BitBuffer2::from_slice(3, 5, &mask_data);

    let label_map_8 = label_map_from_mask_with_connectivity(&mask, Connectivity::Eight);
    assert_eq!(label_map_8.num_labels(), 1);
}
