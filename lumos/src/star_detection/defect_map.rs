//! Defect map for masking known sensor defects during star detection.

use std::collections::HashSet;

use crate::common::Buffer2;

/// Map of known sensor defects (hot pixels, dead pixels, bad columns).
///
/// Used to mask out defective pixels before star detection to prevent
/// false detections and improve centroid accuracy.
///
/// Stores defective pixel indices for memory efficiency with sparse defect maps.
#[derive(Debug, Clone)]
pub struct DefectMap {
    width: usize,
    height: usize,
    /// Sorted list of defective pixel indices (y * width + x).
    defective_indices: Vec<usize>,
}

impl DefectMap {
    /// Create a defect map from lists of defective pixels, columns, and rows.
    pub fn new(
        width: usize,
        height: usize,
        hot_pixels: &[(usize, usize)],
        dead_pixels: &[(usize, usize)],
        bad_columns: &[usize],
        bad_rows: &[usize],
    ) -> Self {
        let mut indices_set = HashSet::new();

        for &(x, y) in hot_pixels {
            if x < width && y < height {
                indices_set.insert(y * width + x);
            }
        }

        for &(x, y) in dead_pixels {
            if x < width && y < height {
                indices_set.insert(y * width + x);
            }
        }

        for &col in bad_columns {
            if col < width {
                for y in 0..height {
                    indices_set.insert(y * width + col);
                }
            }
        }

        for &row in bad_rows {
            if row < height {
                for x in 0..width {
                    indices_set.insert(row * width + x);
                }
            }
        }

        let mut defective_indices: Vec<usize> = indices_set.into_iter().collect();
        defective_indices.sort_unstable();

        Self {
            width,
            height,
            defective_indices,
        }
    }

    /// Check if the defect map has no defective pixels.
    pub fn is_empty(&self) -> bool {
        self.defective_indices.is_empty()
    }

    /// Get the list of defective pixel indices.
    #[inline]
    pub fn defective_indices(&self) -> &[usize] {
        &self.defective_indices
    }

    /// Get dimensions of the defect map.
    pub fn dimensions(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    /// Apply defect mask by replacing defective pixels with local median.
    ///
    /// Copies input to output, replacing defective pixels with the local median
    /// of their non-defective neighbors. This prevents hot pixels and other
    /// defects from being detected as stars or affecting centroid computation.
    pub(crate) fn apply(&self, input: &Buffer2<f32>, output: &mut Buffer2<f32>) {
        assert_eq!(input.len(), output.len());

        // Copy input to output first
        output.copy_from(input);

        if self.defective_indices.is_empty() {
            return;
        }

        // Replace defective pixels with local median
        for &idx in &self.defective_indices {
            let x = idx % input.width();
            let y = idx / input.width();
            output[idx] = local_median_excluding_defects(input, x, y, &self.defective_indices);
        }
    }
}

/// Compute local median of 3x3 neighborhood, excluding defective pixels.
fn local_median_excluding_defects(
    pixels: &Buffer2<f32>,
    cx: usize,
    cy: usize,
    defective_indices: &[usize],
) -> f32 {
    let mut values = [0.0f32; 9];
    let mut count = 0;

    for dy in -1i32..=1 {
        for dx in -1i32..=1 {
            let nx = cx as i32 + dx;
            let ny = cy as i32 + dy;

            if nx >= 0 && nx < pixels.width() as i32 && ny >= 0 && ny < pixels.height() as i32 {
                let nidx = ny as usize * pixels.width() + nx as usize;
                if defective_indices.binary_search(&nidx).is_err() {
                    values[count] = pixels[nidx];
                    count += 1;
                }
            }
        }
    }

    if count == 0 {
        // All neighbors are defective, use the pixel value itself
        pixels[cy * pixels.width() + cx]
    } else {
        let slice = &mut values[..count];
        slice.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        slice[count / 2]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test helper: check if (x, y) is defective
    fn is_defective(map: &DefectMap, x: usize, y: usize) -> bool {
        let (width, height) = map.dimensions();
        if x < width && y < height {
            let idx = y * width + x;
            map.defective_indices().binary_search(&idx).is_ok()
        } else {
            false
        }
    }

    // =============================================================================
    // DefectMap Construction Tests
    // =============================================================================

    #[test]
    fn test_defect_map_empty() {
        let map = DefectMap::new(10, 10, &[], &[], &[], &[]);
        assert!(map.is_empty());
        assert_eq!(map.dimensions(), (10, 10));
    }

    #[test]
    fn test_defect_map_hot_pixels() {
        let hot_pixels = vec![(2, 3), (5, 5)];
        let map = DefectMap::new(10, 10, &hot_pixels, &[], &[], &[]);

        assert!(!map.is_empty());
        assert!(is_defective(&map, 2, 3));
        assert!(is_defective(&map, 5, 5));
        assert!(!is_defective(&map, 0, 0));
        assert!(!is_defective(&map, 2, 2));
    }

    #[test]
    fn test_defect_map_dead_pixels() {
        let dead_pixels = vec![(1, 1), (8, 8)];
        let map = DefectMap::new(10, 10, &[], &dead_pixels, &[], &[]);

        assert!(is_defective(&map, 1, 1));
        assert!(is_defective(&map, 8, 8));
        assert!(!is_defective(&map, 0, 0));
    }

    #[test]
    fn test_defect_map_bad_columns() {
        let bad_columns = vec![3];
        let map = DefectMap::new(10, 10, &[], &[], &bad_columns, &[]);

        // Entire column 3 should be defective
        for y in 0..10 {
            assert!(
                is_defective(&map, 3, y),
                "Column 3, row {} should be defective",
                y
            );
        }
        // Other columns should not be defective
        assert!(!is_defective(&map, 2, 5));
        assert!(!is_defective(&map, 4, 5));
    }

    #[test]
    fn test_defect_map_bad_rows() {
        let bad_rows = vec![7];
        let map = DefectMap::new(10, 10, &[], &[], &[], &bad_rows);

        // Entire row 7 should be defective
        for x in 0..10 {
            assert!(
                is_defective(&map, x, 7),
                "Row 7, col {} should be defective",
                x
            );
        }
        // Other rows should not be defective
        assert!(!is_defective(&map, 5, 6));
        assert!(!is_defective(&map, 5, 8));
    }

    #[test]
    fn test_defect_map_combined() {
        let hot_pixels = vec![(0, 0)];
        let dead_pixels = vec![(9, 9)];
        let bad_columns = vec![5];
        let bad_rows = vec![2];
        let map = DefectMap::new(10, 10, &hot_pixels, &dead_pixels, &bad_columns, &bad_rows);

        assert!(is_defective(&map, 0, 0)); // Hot pixel
        assert!(is_defective(&map, 9, 9)); // Dead pixel
        assert!(is_defective(&map, 5, 0)); // Bad column
        assert!(is_defective(&map, 5, 9)); // Bad column
        assert!(is_defective(&map, 0, 2)); // Bad row
        assert!(is_defective(&map, 9, 2)); // Bad row
        assert!(is_defective(&map, 5, 2)); // Intersection of bad column and row
        assert!(!is_defective(&map, 1, 1)); // Normal pixel
    }

    #[test]
    fn test_defect_map_out_of_bounds_ignored() {
        let hot_pixels = vec![(100, 100), (5, 5)]; // First is out of bounds
        let map = DefectMap::new(10, 10, &hot_pixels, &[], &[], &[]);

        assert!(is_defective(&map, 5, 5));
        assert!(!is_defective(&map, 9, 9)); // No crash, just not defective
    }

    #[test]
    fn test_defect_map_is_defective_out_of_bounds() {
        let map = DefectMap::new(10, 10, &[], &[], &[], &[]);

        // Out of bounds queries should return false, not panic
        assert!(!is_defective(&map, 10, 5));
        assert!(!is_defective(&map, 5, 10));
        assert!(!is_defective(&map, 100, 100));
    }

    #[test]
    fn test_defect_map_indices_access() {
        let hot_pixels = vec![(1, 2), (3, 0)];
        let map = DefectMap::new(5, 5, &hot_pixels, &[], &[], &[]);

        let indices = map.defective_indices();
        assert_eq!(indices.len(), 2);
        // Indices should be sorted
        assert_eq!(indices[0], 3); // (3,0) = 0*5+3 = 3
        assert_eq!(indices[1], 11); // (1,2) = 2*5+1 = 11
    }

    // =============================================================================
    // apply_defect_mask Tests
    // =============================================================================

    #[test]
    fn test_apply_defect_mask_single_hot_pixel() {
        // 5x5 image with uniform value 1.0, one hot pixel at (2,2) with value 10.0
        let mut pixels = vec![1.0f32; 25];
        pixels[2 * 5 + 2] = 10.0; // Hot pixel at (2,2)

        let map = DefectMap::new(5, 5, &[(2, 2)], &[], &[], &[]);
        let input = Buffer2::new(5, 5, pixels);
        let mut output = Buffer2::new_default(5, 5);
        map.apply(&input, &mut output);

        // Hot pixel should be replaced with median of neighbors (all 1.0)
        assert!((output[2 * 5 + 2] - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_apply_defect_mask_corner_pixel() {
        // 5x5 image, hot pixel at corner (0,0)
        let mut pixels = vec![1.0f32; 25];
        pixels[0] = 10.0; // Hot pixel at (0,0)

        let map = DefectMap::new(5, 5, &[(0, 0)], &[], &[], &[]);
        let input = Buffer2::new(5, 5, pixels);
        let mut output = Buffer2::new_default(5, 5);
        map.apply(&input, &mut output);

        // Corner has only 3 neighbors, median should still be 1.0
        assert!((output[0] - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_apply_defect_mask_edge_pixel() {
        // 5x5 image, hot pixel at edge (2,0)
        let mut pixels = vec![1.0f32; 25];
        pixels[2] = 10.0; // Hot pixel at (2,0)

        let map = DefectMap::new(5, 5, &[(2, 0)], &[], &[], &[]);
        let input = Buffer2::new(5, 5, pixels);
        let mut output = Buffer2::new_default(5, 5);
        map.apply(&input, &mut output);

        // Edge has 5 neighbors, median should be 1.0
        assert!((output[2] - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_apply_defect_mask_multiple_hot_pixels() {
        // 5x5 image with two non-adjacent hot pixels
        let mut pixels = vec![1.0f32; 25];
        pixels[6] = 10.0; // (1,1) = 1*5+1 = 6
        pixels[18] = 20.0; // (3,3) = 3*5+3 = 18

        let map = DefectMap::new(5, 5, &[(1, 1), (3, 3)], &[], &[], &[]);
        let input = Buffer2::new(5, 5, pixels);
        let mut output = Buffer2::new_default(5, 5);
        map.apply(&input, &mut output);

        assert!((output[6] - 1.0).abs() < 0.01);
        assert!((output[18] - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_apply_defect_mask_adjacent_hot_pixels() {
        // Two adjacent hot pixels - each should use median excluding the other
        let mut pixels = vec![1.0f32; 25];
        pixels[2 * 5 + 2] = 10.0; // (2,2)
        pixels[2 * 5 + 3] = 15.0; // (3,2)

        let map = DefectMap::new(5, 5, &[(2, 2), (3, 2)], &[], &[], &[]);
        let input = Buffer2::new(5, 5, pixels);
        let mut output = Buffer2::new_default(5, 5);
        map.apply(&input, &mut output);

        // Both should be replaced with median of their non-defective neighbors
        assert!((output[2 * 5 + 2] - 1.0).abs() < 0.01);
        assert!((output[2 * 5 + 3] - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_apply_defect_mask_gradient_neighborhood() {
        // Test that median is correctly computed with varying neighbor values
        let pixels = vec![
            1.0, 2.0, 3.0, 4.0, 5.0, // row 0
            2.0, 3.0, 4.0, 5.0, 6.0, // row 1
            3.0, 4.0, 99.0, 6.0, 7.0, // row 2 - hot pixel at (2,2)
            4.0, 5.0, 6.0, 7.0, 8.0, // row 3
            5.0, 6.0, 7.0, 8.0, 9.0, // row 4
        ];

        let map = DefectMap::new(5, 5, &[(2, 2)], &[], &[], &[]);
        let input = Buffer2::new(5, 5, pixels);
        let mut output = Buffer2::new_default(5, 5);
        map.apply(&input, &mut output);

        // Neighbors of (2,2): 3,4,5, 4,6, 5,6,7 -> sorted: 3,4,4,5,5,6,6,7 -> median = 5
        assert!((output[2 * 5 + 2] - 5.0).abs() < 0.01);
    }

    #[test]
    fn test_apply_defect_mask_empty_map() {
        let pixels = vec![1.0, 2.0, 3.0, 4.0];

        let map = DefectMap::new(2, 2, &[], &[], &[], &[]);
        let input = Buffer2::new(2, 2, pixels.clone());
        let mut output = Buffer2::new_default(2, 2);
        map.apply(&input, &mut output);

        // No changes when map is empty
        assert_eq!(output, Buffer2::new(2, 2, pixels));
    }

    #[test]
    fn test_apply_defect_mask_all_neighbors_defective() {
        // 3x3 image where center and all neighbors are defective
        let pixels = vec![
            10.0, 10.0, 10.0, // row 0
            10.0, 50.0, 10.0, // row 1 - center at (1,1)
            10.0, 10.0, 10.0, // row 2
        ];

        // Mark all pixels as defective
        let all_pixels: Vec<(usize, usize)> =
            (0..3).flat_map(|y| (0..3).map(move |x| (x, y))).collect();
        let map = DefectMap::new(3, 3, &all_pixels, &[], &[], &[]);
        let input = Buffer2::new(3, 3, pixels);
        let mut output = Buffer2::new_default(3, 3);
        map.apply(&input, &mut output);

        // When all neighbors are defective, pixel keeps its original value
        // (1,1) = 1*3+1 = 4
        assert!((output[4] - 50.0).abs() < 0.01);
    }

    #[test]
    fn test_apply_defect_mask_bad_column() {
        // 5x5 image with bad column 2
        let mut pixels = vec![1.0f32; 25];
        for y in 0..5 {
            pixels[y * 5 + 2] = 10.0 + y as f32;
        }

        let map = DefectMap::new(5, 5, &[], &[], &[2], &[]);
        let input = Buffer2::new(5, 5, pixels);
        let mut output = Buffer2::new_default(5, 5);
        map.apply(&input, &mut output);

        // All pixels in column 2 should be replaced
        for y in 0..5 {
            assert!(
                (output[y * 5 + 2] - 1.0).abs() < 0.01,
                "Column 2, row {} should be ~1.0",
                y
            );
        }
    }
}
