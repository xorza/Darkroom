//! Fine structure detection for cosmic ray identification.
//!
//! The fine structure is the difference between the original and median-filtered image.
//! It captures small-scale structure including cosmic rays, but also real fine features
//! like stellar cores.

/// Compute median of a small array (in-place partial sort).
#[inline]
pub fn median_of_n(values: &mut [f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    values[values.len() / 2]
}

/// Compute fine structure image using 3x3 median filter.
///
/// The fine structure is the difference between the original and
/// median-filtered image. It captures small-scale structure including
/// cosmic rays, but also real fine features like stellar cores.
pub fn compute_fine_structure(pixels: &[f32], width: usize, height: usize) -> Vec<f32> {
    let mut fine_structure = vec![0.0f32; pixels.len()];

    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;

            // Collect 3x3 neighborhood
            let mut neighbors = [0.0f32; 9];
            let mut count = 0;

            for dy in 0..3 {
                for dx in 0..3 {
                    let ny = y as isize + dy as isize - 1;
                    let nx = x as isize + dx as isize - 1;

                    if ny >= 0 && ny < height as isize && nx >= 0 && nx < width as isize {
                        neighbors[count] = pixels[ny as usize * width + nx as usize];
                        count += 1;
                    }
                }
            }

            // Compute median
            let median = median_of_n(&mut neighbors[..count]);

            // Fine structure = original - median (positive values = sharp features)
            fine_structure[idx] = (pixels[idx] - median).max(0.0);
        }
    }

    fine_structure
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_median_of_n() {
        let mut values = [1.0, 5.0, 3.0, 2.0, 4.0];
        assert!((median_of_n(&mut values) - 3.0).abs() < 1e-6);

        let mut values2 = [1.0, 2.0, 3.0, 4.0];
        assert!((median_of_n(&mut values2) - 3.0).abs() < 1e-6);

        let mut values3 = [42.0];
        assert!((median_of_n(&mut values3) - 42.0).abs() < 1e-6);
    }

    #[test]
    fn test_fine_structure_flat() {
        let pixels = vec![0.5f32; 25];
        let fine = compute_fine_structure(&pixels, 5, 5);

        // Flat image has no fine structure
        for &v in &fine {
            assert!(v.abs() < 1e-6);
        }
    }

    #[test]
    fn test_fine_structure_peak() {
        let mut pixels = vec![0.0f32; 25];
        pixels[2 * 5 + 2] = 1.0;

        let fine = compute_fine_structure(&pixels, 5, 5);

        // Peak should have positive fine structure
        assert!(fine[2 * 5 + 2] > 0.5);
    }
}
