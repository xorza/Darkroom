//! Defect map for masking known sensor defects during star detection.

/// Map of known sensor defects (hot pixels, dead pixels, bad columns).
///
/// Used to mask out defective pixels before star detection to prevent
/// false detections and improve centroid accuracy.
///
/// The boolean mask is pre-computed at construction time for efficient lookup.
#[derive(Debug, Clone)]
pub struct DefectMap {
    width: usize,
    height: usize,
    /// Pre-computed boolean mask where `true` means the pixel is defective.
    mask: Vec<bool>,
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
        let mut mask = vec![false; width * height];

        for &(x, y) in hot_pixels {
            if x < width && y < height {
                mask[y * width + x] = true;
            }
        }

        for &(x, y) in dead_pixels {
            if x < width && y < height {
                mask[y * width + x] = true;
            }
        }

        for &col in bad_columns {
            if col < width {
                for y in 0..height {
                    mask[y * width + col] = true;
                }
            }
        }

        for &row in bad_rows {
            if row < height {
                for x in 0..width {
                    mask[row * width + x] = true;
                }
            }
        }

        Self {
            width,
            height,
            mask,
        }
    }

    /// Check if a pixel is marked as defective.
    #[inline]
    pub fn is_defective(&self, x: usize, y: usize) -> bool {
        if x < self.width && y < self.height {
            self.mask[y * self.width + x]
        } else {
            false
        }
    }

    /// Check if the defect map has no defective pixels.
    pub fn is_empty(&self) -> bool {
        !self.mask.contains(&true)
    }

    /// Get the pre-computed boolean mask.
    /// Returns a slice where `true` means the pixel is defective.
    #[inline]
    pub fn mask(&self) -> &[bool] {
        &self.mask
    }

    /// Get dimensions of the defect map.
    pub fn dimensions(&self) -> (usize, usize) {
        (self.width, self.height)
    }
}

/// Apply defect mask by replacing defective pixels with local median (in place).
///
/// This prevents hot pixels and other defects from being detected as stars
/// or affecting centroid computation.
pub(crate) fn apply_defect_mask(
    pixels: &mut [f32],
    width: usize,
    height: usize,
    defect_map: &DefectMap,
) {
    let mask = defect_map.mask();

    // Safe to apply immediately: we only read from non-defective neighbors
    // and only write to defective pixels, so there's no read-write conflict
    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;
            if mask[idx] {
                pixels[idx] = local_median_excluding_defects(pixels, width, height, x, y, mask);
            }
        }
    }
}

/// Compute local median of 3x3 neighborhood, excluding defective pixels.
fn local_median_excluding_defects(
    pixels: &[f32],
    width: usize,
    height: usize,
    cx: usize,
    cy: usize,
    defect_mask: &[bool],
) -> f32 {
    let mut values = [0.0f32; 9];
    let mut count = 0;

    for dy in -1i32..=1 {
        for dx in -1i32..=1 {
            let nx = cx as i32 + dx;
            let ny = cy as i32 + dy;

            if nx >= 0 && nx < width as i32 && ny >= 0 && ny < height as i32 {
                let nidx = ny as usize * width + nx as usize;
                if !defect_mask[nidx] {
                    values[count] = pixels[nidx];
                    count += 1;
                }
            }
        }
    }

    if count == 0 {
        // All neighbors are defective, use the pixel value itself
        pixels[cy * width + cx]
    } else {
        let slice = &mut values[..count];
        slice.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        slice[count / 2]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(map.is_defective(2, 3));
        assert!(map.is_defective(5, 5));
        assert!(!map.is_defective(0, 0));
        assert!(!map.is_defective(2, 2));
    }

    #[test]
    fn test_defect_map_dead_pixels() {
        let dead_pixels = vec![(1, 1), (8, 8)];
        let map = DefectMap::new(10, 10, &[], &dead_pixels, &[], &[]);

        assert!(map.is_defective(1, 1));
        assert!(map.is_defective(8, 8));
        assert!(!map.is_defective(0, 0));
    }

    #[test]
    fn test_defect_map_bad_columns() {
        let bad_columns = vec![3];
        let map = DefectMap::new(10, 10, &[], &[], &bad_columns, &[]);

        // Entire column 3 should be defective
        for y in 0..10 {
            assert!(
                map.is_defective(3, y),
                "Column 3, row {} should be defective",
                y
            );
        }
        // Other columns should not be defective
        assert!(!map.is_defective(2, 5));
        assert!(!map.is_defective(4, 5));
    }

    #[test]
    fn test_defect_map_bad_rows() {
        let bad_rows = vec![7];
        let map = DefectMap::new(10, 10, &[], &[], &[], &bad_rows);

        // Entire row 7 should be defective
        for x in 0..10 {
            assert!(
                map.is_defective(x, 7),
                "Row 7, col {} should be defective",
                x
            );
        }
        // Other rows should not be defective
        assert!(!map.is_defective(5, 6));
        assert!(!map.is_defective(5, 8));
    }

    #[test]
    fn test_defect_map_combined() {
        let hot_pixels = vec![(0, 0)];
        let dead_pixels = vec![(9, 9)];
        let bad_columns = vec![5];
        let bad_rows = vec![2];
        let map = DefectMap::new(10, 10, &hot_pixels, &dead_pixels, &bad_columns, &bad_rows);

        assert!(map.is_defective(0, 0)); // Hot pixel
        assert!(map.is_defective(9, 9)); // Dead pixel
        assert!(map.is_defective(5, 0)); // Bad column
        assert!(map.is_defective(5, 9)); // Bad column
        assert!(map.is_defective(0, 2)); // Bad row
        assert!(map.is_defective(9, 2)); // Bad row
        assert!(map.is_defective(5, 2)); // Intersection of bad column and row
        assert!(!map.is_defective(1, 1)); // Normal pixel
    }

    #[test]
    fn test_defect_map_out_of_bounds_ignored() {
        let hot_pixels = vec![(100, 100), (5, 5)]; // First is out of bounds
        let map = DefectMap::new(10, 10, &hot_pixels, &[], &[], &[]);

        assert!(map.is_defective(5, 5));
        assert!(!map.is_defective(9, 9)); // No crash, just not defective
    }

    #[test]
    fn test_defect_map_is_defective_out_of_bounds() {
        let map = DefectMap::new(10, 10, &[], &[], &[], &[]);

        // Out of bounds queries should return false, not panic
        assert!(!map.is_defective(10, 5));
        assert!(!map.is_defective(5, 10));
        assert!(!map.is_defective(100, 100));
    }

    #[test]
    fn test_defect_map_mask_access() {
        let hot_pixels = vec![(1, 2)];
        let map = DefectMap::new(5, 5, &hot_pixels, &[], &[], &[]);

        let mask = map.mask();
        assert_eq!(mask.len(), 25);
        assert!(mask[2 * 5 + 1]); // y=2, x=1
        assert!(!mask[0]);
    }

    // =============================================================================
    // apply_defect_mask Tests
    // =============================================================================

    #[test]
    fn test_apply_defect_mask_single_hot_pixel() {
        // 5x5 image with uniform value 1.0, one hot pixel at (2,2) with value 10.0
        let mut pixels = vec![1.0f32; 25];
        pixels[2 * 5 + 2] = 10.0; // Hot pixel

        let map = DefectMap::new(5, 5, &[(2, 2)], &[], &[], &[]);
        apply_defect_mask(&mut pixels, 5, 5, &map);

        // Hot pixel should be replaced with median of neighbors (all 1.0)
        assert!((pixels[2 * 5 + 2] - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_apply_defect_mask_corner_pixel() {
        // 5x5 image, hot pixel at corner (0,0)
        let mut pixels = vec![1.0f32; 25];
        pixels[0] = 10.0; // Hot pixel at (0,0)

        let map = DefectMap::new(5, 5, &[(0, 0)], &[], &[], &[]);
        apply_defect_mask(&mut pixels, 5, 5, &map);

        // Corner has only 3 neighbors, median should still be 1.0
        assert!((pixels[0] - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_apply_defect_mask_edge_pixel() {
        // 5x5 image, hot pixel at edge (2,0)
        let mut pixels = vec![1.0f32; 25];
        pixels[2] = 10.0; // Hot pixel at (2,0)

        let map = DefectMap::new(5, 5, &[(2, 0)], &[], &[], &[]);
        apply_defect_mask(&mut pixels, 5, 5, &map);

        // Edge has 5 neighbors, median should be 1.0
        assert!((pixels[2] - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_apply_defect_mask_multiple_hot_pixels() {
        // 5x5 image with two non-adjacent hot pixels
        let mut pixels = vec![1.0f32; 25];
        pixels[6] = 10.0; // (1,1) = 1*5+1 = 6
        pixels[18] = 20.0; // (3,3) = 3*5+3 = 18

        let map = DefectMap::new(5, 5, &[(1, 1), (3, 3)], &[], &[], &[]);
        apply_defect_mask(&mut pixels, 5, 5, &map);

        assert!((pixels[6] - 1.0).abs() < 0.01);
        assert!((pixels[18] - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_apply_defect_mask_adjacent_hot_pixels() {
        // Two adjacent hot pixels - each should use median excluding the other
        let mut pixels = vec![1.0f32; 25];
        pixels[2 * 5 + 2] = 10.0; // (2,2)
        pixels[2 * 5 + 3] = 15.0; // (3,2)

        let map = DefectMap::new(5, 5, &[(2, 2), (3, 2)], &[], &[], &[]);
        apply_defect_mask(&mut pixels, 5, 5, &map);

        // Both should be replaced with median of their non-defective neighbors
        assert!((pixels[2 * 5 + 2] - 1.0).abs() < 0.01);
        assert!((pixels[2 * 5 + 3] - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_apply_defect_mask_gradient_neighborhood() {
        // Test that median is correctly computed with varying neighbor values
        let mut pixels = vec![
            1.0, 2.0, 3.0, 4.0, 5.0, // row 0
            2.0, 3.0, 4.0, 5.0, 6.0, // row 1
            3.0, 4.0, 99.0, 6.0, 7.0, // row 2 - hot pixel at (2,2)
            4.0, 5.0, 6.0, 7.0, 8.0, // row 3
            5.0, 6.0, 7.0, 8.0, 9.0, // row 4
        ];

        let map = DefectMap::new(5, 5, &[(2, 2)], &[], &[], &[]);
        apply_defect_mask(&mut pixels, 5, 5, &map);

        // Neighbors of (2,2): 3,4,5, 4,6, 5,6,7 -> sorted: 3,4,4,5,5,6,6,7 -> median = 5
        assert!((pixels[2 * 5 + 2] - 5.0).abs() < 0.01);
    }

    #[test]
    fn test_apply_defect_mask_empty_map() {
        let mut pixels = vec![1.0, 2.0, 3.0, 4.0];
        let original = pixels.clone();

        let map = DefectMap::new(2, 2, &[], &[], &[], &[]);
        apply_defect_mask(&mut pixels, 2, 2, &map);

        // No changes when map is empty
        assert_eq!(pixels, original);
    }

    #[test]
    fn test_apply_defect_mask_all_neighbors_defective() {
        // 3x3 image where center and all neighbors are defective
        let mut pixels = vec![
            10.0, 10.0, 10.0, // row 0
            10.0, 50.0, 10.0, // row 1 - center at (1,1)
            10.0, 10.0, 10.0, // row 2
        ];

        // Mark all pixels as defective
        let all_pixels: Vec<(usize, usize)> =
            (0..3).flat_map(|y| (0..3).map(move |x| (x, y))).collect();
        let map = DefectMap::new(3, 3, &all_pixels, &[], &[], &[]);
        apply_defect_mask(&mut pixels, 3, 3, &map);

        // When all neighbors are defective, pixel keeps its original value
        // (1,1) = 1*3+1 = 4
        assert!((pixels[4] - 50.0).abs() < 0.01);
    }

    #[test]
    fn test_apply_defect_mask_bad_column() {
        // 5x5 image with bad column 2
        let mut pixels = vec![1.0f32; 25];
        for y in 0..5 {
            pixels[y * 5 + 2] = 10.0 + y as f32;
        }

        let map = DefectMap::new(5, 5, &[], &[], &[2], &[]);
        apply_defect_mask(&mut pixels, 5, 5, &map);

        // All pixels in column 2 should be replaced
        for y in 0..5 {
            assert!(
                (pixels[y * 5 + 2] - 1.0).abs() < 0.01,
                "Column 2, row {} should be ~1.0",
                y
            );
        }
    }
}
