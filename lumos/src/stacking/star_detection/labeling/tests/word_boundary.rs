use crate::stacking::star_detection::labeling::tests::*;

#[test]
fn boundary_64() {
    // Test component crossing 64-pixel word boundary
    let width = 70;
    let height = 3;
    let mut mask_data = vec![false; width * height];

    // Horizontal line crossing word boundary at x=63-66
    for x in 62..67 {
        mask_data[1 * width + x] = true;
    }
    let mask = BitBuffer2::from_slice(width, height, &mask_data);

    let label_map = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);

    assert_eq!(label_map.num_labels(), 1);
    let label = label_map[1 * width + 62];
    for x in 62..67 {
        assert_eq!(
            label_map[1 * width + x],
            label,
            "Pixel at x={} should have same label",
            x
        );
    }
}

#[test]
fn boundary_128() {
    // Test component crossing second word boundary at x=128
    let width = 140;
    let height = 3;
    let mut mask_data = vec![false; width * height];

    // Horizontal line crossing word boundary at x=126-130
    for x in 126..131 {
        mask_data[1 * width + x] = true;
    }
    let mask = BitBuffer2::from_slice(width, height, &mask_data);

    let label_map = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);

    assert_eq!(label_map.num_labels(), 1);
    let label = label_map[1 * width + 126];
    for x in 126..131 {
        assert_eq!(
            label_map[1 * width + x],
            label,
            "Pixel at x={} should have same label",
            x
        );
    }
}
