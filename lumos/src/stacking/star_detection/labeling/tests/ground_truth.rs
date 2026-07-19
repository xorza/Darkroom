use crate::stacking::star_detection::labeling::tests::*;

#[test]
fn simple_shapes() {
    // Single pixel
    let mut mask = vec![false; 16];
    mask[5] = true;
    compare_with_reference(&mask, 4, 4);

    // Horizontal line
    let mut mask = vec![false; 20];
    for x in 2..7 {
        mask[1 * 5 + x % 5] = true;
    }
    compare_with_reference(&mask, 5, 4);

    // Vertical line
    let mut mask = vec![false; 20];
    for y in 0..4 {
        mask[y * 5 + 2] = true;
    }
    compare_with_reference(&mask, 5, 4);

    // L-shape
    let mut mask = vec![false; 16];
    mask[0] = true;
    mask[4] = true;
    mask[8] = true;
    mask[9] = true;
    compare_with_reference(&mask, 4, 4);
}

#[test]
fn multiple_components() {
    // Two separate blobs
    let mut mask = vec![false; 25];
    mask[0] = true;
    mask[1] = true;
    mask[5] = true;
    mask[23] = true;
    mask[24] = true;
    compare_with_reference(&mask, 5, 5);

    // Three diagonal pixels (separate in 4-conn, together in 8-conn)
    let mut mask = vec![false; 9];
    mask[0] = true;
    mask[4] = true;
    mask[8] = true;
    compare_with_reference(&mask, 3, 3);
}

#[test]
fn checkerboard_patterns() {
    // Small checkerboard
    let mask: Vec<bool> = (0..16)
        .map(|i| {
            let x = i % 4;
            let y = i / 4;
            (x + y) % 2 == 0
        })
        .collect();
    compare_with_reference(&mask, 4, 4);

    // Larger checkerboard
    let mask: Vec<bool> = (0..64)
        .map(|i| {
            let x = i % 8;
            let y = i / 8;
            (x + y) % 2 == 0
        })
        .collect();
    compare_with_reference(&mask, 8, 8);
}

#[test]
fn spiral_shape() {
    // Spiral pattern - tests complex connectivity
    let width = 9;
    let height = 9;
    let mut mask = vec![false; width * height];

    // Outer ring
    for x in 0..9 {
        mask[0 * width + x] = true;
    }
    for y in 1..9 {
        mask[y * width + 8] = true;
    }
    for x in 0..8 {
        mask[8 * width + x] = true;
    }
    for y in 2..8 {
        mask[y * width + 0] = true;
    }

    // Inner part
    for x in 2..7 {
        mask[2 * width + x] = true;
    }
    for y in 3..7 {
        mask[y * width + 6] = true;
    }
    for x in 2..6 {
        mask[6 * width + x] = true;
    }
    for y in 4..6 {
        mask[y * width + 2] = true;
    }
    mask[4 * width + 4] = true;

    compare_with_reference(&mask, width, height);
}

#[test]
fn concentric_squares() {
    // Concentric squares (nested loops)
    let width = 11;
    let height = 11;
    let mut mask = vec![false; width * height];

    // Outer square
    for x in 0..11 {
        mask[0 * width + x] = true;
        mask[10 * width + x] = true;
    }
    for y in 1..10 {
        mask[y * width + 0] = true;
        mask[y * width + 10] = true;
    }

    // Inner square (separate component)
    for x in 4..7 {
        mask[4 * width + x] = true;
        mask[6 * width + x] = true;
    }
    for y in 5..6 {
        mask[y * width + 4] = true;
        mask[y * width + 6] = true;
    }

    compare_with_reference(&mask, width, height);
}
