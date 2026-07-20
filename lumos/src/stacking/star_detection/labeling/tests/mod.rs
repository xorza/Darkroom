//! Tests for connected component labeling.

// Allow identity operations like `y * width + x` for clarity in 2D indexing
#![allow(clippy::identity_op, clippy::erasing_op)]

use crate::bit_buffer2::BitBuffer2;
use crate::stacking::star_detection::config::Connectivity;
use crate::stacking::star_detection::labeling::test_utils::label_map_from_mask_with_connectivity;

/// Simple flood-fill reference implementation for ground truth comparison.
/// This is intentionally naive and slow but obviously correct.
fn reference_ccl_4conn(mask: &[bool], width: usize, height: usize) -> (Vec<u32>, usize) {
    let mut labels = vec![0u32; width * height];
    let mut current_label = 0u32;

    for start_y in 0..height {
        for start_x in 0..width {
            let start_idx = start_y * width + start_x;
            if !mask[start_idx] || labels[start_idx] != 0 {
                continue;
            }

            // New component - flood fill
            current_label += 1;
            let mut stack = vec![(start_x, start_y)];

            while let Some((x, y)) = stack.pop() {
                let idx = y * width + x;
                if labels[idx] != 0 || !mask[idx] {
                    continue;
                }
                labels[idx] = current_label;

                // 4-connectivity neighbors
                if x > 0 {
                    stack.push((x - 1, y));
                }
                if x + 1 < width {
                    stack.push((x + 1, y));
                }
                if y > 0 {
                    stack.push((x, y - 1));
                }
                if y + 1 < height {
                    stack.push((x, y + 1));
                }
            }
        }
    }

    (labels, current_label as usize)
}

/// Simple flood-fill reference implementation for 8-connectivity.
fn reference_ccl_8conn(mask: &[bool], width: usize, height: usize) -> (Vec<u32>, usize) {
    let mut labels = vec![0u32; width * height];
    let mut current_label = 0u32;

    for start_y in 0..height {
        for start_x in 0..width {
            let start_idx = start_y * width + start_x;
            if !mask[start_idx] || labels[start_idx] != 0 {
                continue;
            }

            // New component - flood fill
            current_label += 1;
            let mut stack = vec![(start_x, start_y)];

            while let Some((x, y)) = stack.pop() {
                let idx = y * width + x;
                if labels[idx] != 0 || !mask[idx] {
                    continue;
                }
                labels[idx] = current_label;

                // 8-connectivity neighbors
                for dy in -1i32..=1 {
                    for dx in -1i32..=1 {
                        if dx == 0 && dy == 0 {
                            continue;
                        }
                        let nx = x as i32 + dx;
                        let ny = y as i32 + dy;
                        if nx >= 0 && nx < width as i32 && ny >= 0 && ny < height as i32 {
                            stack.push((nx as usize, ny as usize));
                        }
                    }
                }
            }
        }
    }

    (labels, current_label as usize)
}

/// Verify CCL invariants on any label map.
fn verify_ccl_invariants(
    mask: &[bool],
    labels: &[u32],
    width: usize,
    height: usize,
    connectivity: Connectivity,
) {
    // Invariant 1: Background pixels have label 0
    for (i, (&m, &l)) in mask.iter().zip(labels.iter()).enumerate() {
        if !m {
            assert_eq!(
                l, 0,
                "Background pixel at index {} has non-zero label {}",
                i, l
            );
        }
    }

    // Invariant 2: Foreground pixels have non-zero labels
    for (i, (&m, &l)) in mask.iter().zip(labels.iter()).enumerate() {
        if m {
            assert!(l > 0, "Foreground pixel at index {} has zero label", i);
        }
    }

    // Invariant 3: Connected pixels have the same label
    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;
            if !mask[idx] {
                continue;
            }
            let label = labels[idx];

            // Check all neighbors based on connectivity
            let neighbors: Vec<(i32, i32)> = match connectivity {
                Connectivity::Four => vec![(-1, 0), (1, 0), (0, -1), (0, 1)],
                Connectivity::Eight => vec![
                    (-1, -1),
                    (0, -1),
                    (1, -1),
                    (-1, 0),
                    (1, 0),
                    (-1, 1),
                    (0, 1),
                    (1, 1),
                ],
            };

            for (dx, dy) in neighbors {
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                if nx >= 0 && nx < width as i32 && ny >= 0 && ny < height as i32 {
                    let nidx = ny as usize * width + nx as usize;
                    if mask[nidx] {
                        assert_eq!(
                            labels[nidx], label,
                            "Connected pixels at ({},{}) and ({},{}) have different labels: {} vs {}",
                            x, y, nx, ny, label, labels[nidx]
                        );
                    }
                }
            }
        }
    }

    // Invariant 4: Labels are sequential starting from 1
    let max_label = *labels.iter().max().unwrap_or(&0);
    if max_label > 0 {
        let mut label_present = vec![false; max_label as usize + 1];
        for &l in labels {
            if l > 0 {
                label_present[l as usize] = true;
            }
        }
        for l in 1..=max_label {
            assert!(
                label_present[l as usize],
                "Label {} is missing from sequential range 1..{}",
                l, max_label
            );
        }
    }
}

/// Compare our implementation against reference flood-fill.
fn compare_with_reference(mask_data: &[bool], width: usize, height: usize) {
    let mask = BitBuffer2::from_slice(width, height, mask_data);

    // Test 4-connectivity
    let label_map_4 = label_map_from_mask_with_connectivity(&mask, Connectivity::Four);
    let (ref_labels_4, ref_count_4) = reference_ccl_4conn(mask_data, width, height);

    assert_eq!(
        label_map_4.num_labels(),
        ref_count_4,
        "4-conn: Component count mismatch: got {}, expected {}",
        label_map_4.num_labels(),
        ref_count_4
    );

    verify_ccl_invariants(
        mask_data,
        label_map_4.labels(),
        width,
        height,
        Connectivity::Four,
    );

    // Verify same grouping (labels may differ but grouping must match)
    verify_same_grouping(label_map_4.labels(), &ref_labels_4, width * height);

    // Test 8-connectivity
    let label_map_8 = label_map_from_mask_with_connectivity(&mask, Connectivity::Eight);
    let (ref_labels_8, ref_count_8) = reference_ccl_8conn(mask_data, width, height);

    assert_eq!(
        label_map_8.num_labels(),
        ref_count_8,
        "8-conn: Component count mismatch: got {}, expected {}",
        label_map_8.num_labels(),
        ref_count_8
    );

    verify_ccl_invariants(
        mask_data,
        label_map_8.labels(),
        width,
        height,
        Connectivity::Eight,
    );
    verify_same_grouping(label_map_8.labels(), &ref_labels_8, width * height);
}

/// Verify two labelings have the same grouping (same pixels grouped together).
fn verify_same_grouping(labels_a: &[u32], labels_b: &[u32], len: usize) {
    // Two labelings are equivalent if: for all i,j:
    // labels_a[i] == labels_a[j] <=> labels_b[i] == labels_b[j]
    // We check this by verifying that mapping from a->b is consistent

    use std::collections::HashMap;
    let mut a_to_b: HashMap<u32, u32> = HashMap::new();

    for i in 0..len {
        let la = labels_a[i];
        let lb = labels_b[i];

        if la == 0 && lb == 0 {
            continue; // Both background
        }

        if la == 0 || lb == 0 {
            panic!(
                "Pixel {} has label {} in A but {} in B (one is background)",
                i, la, lb
            );
        }

        match a_to_b.get(&la) {
            Some(&expected_b) => {
                assert_eq!(
                    lb, expected_b,
                    "Inconsistent grouping: label {} in A maps to both {} and {} in B",
                    la, expected_b, lb
                );
            }
            None => {
                a_to_b.insert(la, lb);
            }
        }
    }
}

mod basic;
mod eight_connectivity;
mod ground_truth;
mod parallel;
mod pixel_level;
mod property_based;
mod rle;
mod word_boundary;
