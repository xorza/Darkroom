//! Simple deblending using local maxima detection.
//!
//! This is a fast deblending algorithm that works by:
//! 1. Finding all local maxima in a connected component
//! 2. Filtering by prominence (peak must be significant fraction of primary)
//! 3. Filtering by separation (peaks must be sufficiently far apart)
//! 4. Assigning pixels to nearest peak using Voronoi partitioning

use super::DeblendConfig;

/// A pixel with its coordinates and value.
#[derive(Debug, Clone, Copy)]
pub struct Pixel {
    pub x: usize,
    pub y: usize,
    pub value: f32,
}

/// Temporary data for a connected component.
#[derive(Debug)]
pub struct ComponentData {
    pub x_min: usize,
    pub x_max: usize,
    pub y_min: usize,
    pub y_max: usize,
    pub pixels: Vec<Pixel>,
}

/// Result of deblending a single connected component.
#[derive(Debug)]
pub struct DeblendedCandidate {
    /// Bounding box min X.
    pub x_min: usize,
    /// Bounding box max X.
    pub x_max: usize,
    /// Bounding box min Y.
    pub y_min: usize,
    /// Bounding box max Y.
    pub y_max: usize,
    /// Peak pixel X coordinate.
    pub peak_x: usize,
    /// Peak pixel Y coordinate.
    pub peak_y: usize,
    /// Peak pixel value.
    pub peak_value: f32,
    /// Number of pixels in the region.
    pub area: usize,
}

/// Find local maxima within a component for deblending.
///
/// A pixel is a local maximum if it's greater than all 8 neighbors.
/// Only returns peaks that are sufficiently separated and prominent.
pub fn find_local_maxima(
    data: &ComponentData,
    pixels: &[f32],
    width: usize,
    config: &DeblendConfig,
) -> Vec<Pixel> {
    let mut peaks: Vec<Pixel> = Vec::new();

    // Find global maximum first
    let global_max = data.pixels.iter().map(|p| p.value).fold(f32::MIN, f32::max);

    let min_peak_value = global_max * config.min_prominence;

    // Check each pixel for local maximum
    for &pixel in &data.pixels {
        if pixel.value < min_peak_value {
            continue;
        }

        // Check if this is a local maximum (greater than all 8 neighbors)
        let mut is_maximum = true;
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                if dx == 0 && dy == 0 {
                    continue;
                }

                let nx = pixel.x as i32 + dx;
                let ny = pixel.y as i32 + dy;

                if nx >= 0 && ny >= 0 {
                    let nx = nx as usize;
                    let ny = ny as usize;
                    let neighbor_idx = ny * width + nx;

                    if neighbor_idx < pixels.len() && pixels[neighbor_idx] >= pixel.value {
                        is_maximum = false;
                        break;
                    }
                }
            }
            if !is_maximum {
                break;
            }
        }

        if is_maximum {
            // Check separation from existing peaks
            let min_sep = config.min_separation;
            let well_separated = peaks.iter().all(|peak| {
                let dx = (pixel.x as i32 - peak.x as i32).unsigned_abs() as usize;
                let dy = (pixel.y as i32 - peak.y as i32).unsigned_abs() as usize;
                dx >= min_sep || dy >= min_sep
            });

            if well_separated {
                peaks.push(pixel);
            } else {
                // Keep the brighter peak
                for peak in &mut peaks {
                    let dx = (pixel.x as i32 - peak.x as i32).unsigned_abs() as usize;
                    let dy = (pixel.y as i32 - peak.y as i32).unsigned_abs() as usize;
                    if dx < min_sep && dy < min_sep && pixel.value > peak.value {
                        *peak = pixel;
                        break;
                    }
                }
            }
        }
    }

    // Sort by brightness (brightest first)
    peaks.sort_by(|a, b| {
        b.value
            .partial_cmp(&a.value)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    peaks
}

/// Deblend a component into multiple candidates based on peak positions.
///
/// Each pixel is assigned to the nearest peak (Voronoi partitioning),
/// creating separate candidates.
pub fn deblend_by_nearest_peak(data: &ComponentData, peaks: &[Pixel]) -> Vec<DeblendedCandidate> {
    if peaks.is_empty() {
        return Vec::new();
    }

    // Initialize per-peak data
    let mut peak_data: Vec<(usize, usize, usize, usize, usize)> = peaks
        .iter()
        .map(|_| (usize::MAX, 0, usize::MAX, 0, 0)) // (x_min, x_max, y_min, y_max, area)
        .collect();

    // Assign each pixel to nearest peak
    for pixel in &data.pixels {
        let mut min_dist_sq = usize::MAX;
        let mut nearest_peak = 0;

        for (i, peak) in peaks.iter().enumerate() {
            let dx = (pixel.x as i32 - peak.x as i32).unsigned_abs() as usize;
            let dy = (pixel.y as i32 - peak.y as i32).unsigned_abs() as usize;
            let dist_sq = dx * dx + dy * dy;

            if dist_sq < min_dist_sq {
                min_dist_sq = dist_sq;
                nearest_peak = i;
            }
        }

        let pd = &mut peak_data[nearest_peak];
        pd.0 = pd.0.min(pixel.x);
        pd.1 = pd.1.max(pixel.x);
        pd.2 = pd.2.min(pixel.y);
        pd.3 = pd.3.max(pixel.y);
        pd.4 += 1;
    }

    // Build candidates
    peaks
        .iter()
        .zip(peak_data.iter())
        .filter(|(_, pd)| pd.4 > 0) // Only include peaks with assigned pixels
        .map(
            |(peak, &(x_min, x_max, y_min, y_max, area))| DeblendedCandidate {
                x_min,
                x_max,
                y_min,
                y_max,
                peak_x: peak.x,
                peak_y: peak.y,
                peak_value: peak.value,
                area,
            },
        )
        .collect()
}

/// Deblend a component using local maxima detection.
///
/// This is a convenience function that combines `find_local_maxima` and
/// `deblend_by_nearest_peak`.
///
/// Returns a single candidate if no deblending is needed, or multiple
/// candidates if the component contains multiple peaks.
pub fn deblend_local_maxima(
    data: &ComponentData,
    pixels: &[f32],
    width: usize,
    config: &DeblendConfig,
) -> Vec<DeblendedCandidate> {
    let peaks = find_local_maxima(data, pixels, width, config);

    if peaks.len() <= 1 {
        // Single peak - create one candidate
        let peak = if peaks.is_empty() {
            // Fallback: use global maximum
            data.pixels
                .iter()
                .max_by(|a, b| {
                    a.value
                        .partial_cmp(&b.value)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .copied()
                .unwrap_or(Pixel {
                    x: data.x_min,
                    y: data.y_min,
                    value: 0.0,
                })
        } else {
            peaks[0]
        };

        vec![DeblendedCandidate {
            x_min: data.x_min,
            x_max: data.x_max,
            y_min: data.y_min,
            y_max: data.y_max,
            peak_x: peak.x,
            peak_y: peak.y,
            peak_value: peak.value,
            area: data.pixels.len(),
        }]
    } else {
        // Multiple peaks - deblend by assigning pixels to nearest peak
        deblend_by_nearest_peak(data, &peaks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_gaussian_star(cx: usize, cy: usize, amplitude: f32, sigma: f32) -> Vec<Pixel> {
        let mut pixels = Vec::new();
        let radius = (sigma * 4.0).ceil() as i32;

        for dy in -radius..=radius {
            for dx in -radius..=radius {
                let x = (cx as i32 + dx) as usize;
                let y = (cy as i32 + dy) as usize;
                let r2 = (dx * dx + dy * dy) as f32;
                let value = amplitude * (-r2 / (2.0 * sigma * sigma)).exp();
                if value > 0.001 {
                    pixels.push(Pixel { x, y, value });
                }
            }
        }

        pixels
    }

    #[test]
    fn test_find_single_peak() {
        let star = make_gaussian_star(50, 50, 1.0, 3.0);

        let data = ComponentData {
            x_min: star.iter().map(|p| p.x).min().unwrap(),
            x_max: star.iter().map(|p| p.x).max().unwrap(),
            y_min: star.iter().map(|p| p.y).min().unwrap(),
            y_max: star.iter().map(|p| p.y).max().unwrap(),
            pixels: star.clone(),
        };

        // Create full image
        let width = 100;
        let height = 100;
        let mut pixels = vec![0.0f32; width * height];
        for p in &star {
            if p.x < width && p.y < height {
                pixels[p.y * width + p.x] = p.value;
            }
        }

        let config = DeblendConfig::default();
        let peaks = find_local_maxima(&data, &pixels, width, &config);

        assert_eq!(peaks.len(), 1, "Should find exactly one peak");
        assert!(
            (peaks[0].x as i32 - 50).abs() <= 1,
            "Peak should be near center"
        );
    }

    #[test]
    fn test_find_two_peaks() {
        let width = 100;
        let height = 100;

        let star1 = make_gaussian_star(30, 50, 1.0, 2.5);
        let star2 = make_gaussian_star(70, 50, 0.8, 2.5);

        // Combine into one component
        let mut all_pixels: Vec<Pixel> = Vec::new();
        let mut image = vec![0.0f32; width * height];

        for p in star1.iter().chain(star2.iter()) {
            if p.x < width && p.y < height {
                image[p.y * width + p.x] += p.value;
                all_pixels.push(Pixel {
                    x: p.x,
                    y: p.y,
                    value: image[p.y * width + p.x],
                });
            }
        }

        // Deduplicate
        let mut seen = std::collections::HashMap::new();
        for p in all_pixels {
            seen.insert((p.x, p.y), p.value);
        }
        let component_pixels: Vec<_> = seen
            .into_iter()
            .map(|((x, y), value)| Pixel { x, y, value })
            .collect();

        let data = ComponentData {
            x_min: component_pixels.iter().map(|p| p.x).min().unwrap(),
            x_max: component_pixels.iter().map(|p| p.x).max().unwrap(),
            y_min: component_pixels.iter().map(|p| p.y).min().unwrap(),
            y_max: component_pixels.iter().map(|p| p.y).max().unwrap(),
            pixels: component_pixels,
        };

        let config = DeblendConfig {
            min_separation: 3,
            min_prominence: 0.3,
            ..Default::default()
        };
        let peaks = find_local_maxima(&data, &image, width, &config);

        assert_eq!(peaks.len(), 2, "Should find two peaks");
    }

    #[test]
    fn test_deblend_creates_separate_candidates() {
        let width = 100;
        let height = 100;

        let star1 = make_gaussian_star(30, 50, 1.0, 2.5);
        let star2 = make_gaussian_star(70, 50, 0.8, 2.5);

        let mut all_pixels: Vec<Pixel> = Vec::new();
        let mut image = vec![0.0f32; width * height];

        for p in star1.iter().chain(star2.iter()) {
            if p.x < width && p.y < height {
                image[p.y * width + p.x] += p.value;
                all_pixels.push(Pixel {
                    x: p.x,
                    y: p.y,
                    value: image[p.y * width + p.x],
                });
            }
        }

        let mut seen = std::collections::HashMap::new();
        for p in all_pixels {
            seen.insert((p.x, p.y), p.value);
        }
        let component_pixels: Vec<_> = seen
            .into_iter()
            .map(|((x, y), value)| Pixel { x, y, value })
            .collect();

        let data = ComponentData {
            x_min: component_pixels.iter().map(|p| p.x).min().unwrap(),
            x_max: component_pixels.iter().map(|p| p.x).max().unwrap(),
            y_min: component_pixels.iter().map(|p| p.y).min().unwrap(),
            y_max: component_pixels.iter().map(|p| p.y).max().unwrap(),
            pixels: component_pixels,
        };

        let config = DeblendConfig {
            min_separation: 3,
            min_prominence: 0.3,
            ..Default::default()
        };

        let candidates = deblend_local_maxima(&data, &image, width, &config);

        assert_eq!(candidates.len(), 2, "Should create two candidates");
        assert!(candidates[0].area > 0);
        assert!(candidates[1].area > 0);
    }
}
