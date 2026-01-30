//! Simple deblending using local maxima detection.
//!
//! This is a fast deblending algorithm that works by:
//! 1. Finding all local maxima in a connected component
//! 2. Filtering by prominence (peak must be significant fraction of primary)
//! 3. Filtering by separation (peaks must be sufficiently far apart)
//! 4. Assigning pixels to nearest peak using Voronoi partitioning
//!
//! Uses `ArrayVec` for small fixed-capacity collections to avoid heap allocations
//! in the common case (most components have ≤8 peaks).

use arrayvec::ArrayVec;

use super::DeblendConfig;
use crate::common::Buffer2;
use crate::star_detection::detection::LabelMap;

// ============================================================================
// Constants
// ============================================================================

/// Maximum number of peaks/candidates per component.
/// Components with more peaks than this will have excess peaks ignored.
pub const MAX_PEAKS: usize = 8;

// ============================================================================
// Types
// ============================================================================

/// A pixel with its coordinates and value.
#[derive(Debug, Clone, Copy)]
pub struct Pixel {
    pub x: usize,
    pub y: usize,
    pub value: f32,
}

/// Data for a connected component (allocation-free).
///
/// Instead of storing pixel coordinates, we store the component label
/// and iterate over the bounding box on-demand, checking the labels buffer.
#[derive(Debug, Clone, Copy)]
pub struct ComponentData {
    pub x_min: usize,
    pub x_max: usize,
    pub y_min: usize,
    pub y_max: usize,
    /// Component label in the labels buffer.
    pub label: u32,
    /// Number of pixels in the component (pre-computed).
    pub area: usize,
}

/// Result of deblending a single connected component.
#[derive(Debug)]
pub struct DeblendedCandidate {
    pub x_min: usize,
    pub x_max: usize,
    pub y_min: usize,
    pub y_max: usize,
    pub peak_x: usize,
    pub peak_y: usize,
    pub peak_value: f32,
    pub area: usize,
}

/// Per-peak bounding box and area data (internal).
#[derive(Debug, Copy, Clone)]
struct PeakData {
    x_min: usize,
    x_max: usize,
    y_min: usize,
    y_max: usize,
    area: usize,
}

impl Default for PeakData {
    fn default() -> Self {
        Self {
            x_min: usize::MAX,
            x_max: 0,
            y_min: usize::MAX,
            y_max: 0,
            area: 0,
        }
    }
}

// ============================================================================
// ComponentData implementation
// ============================================================================

impl ComponentData {
    /// Iterate over all pixels in this component.
    ///
    /// Scans the bounding box and yields pixels that match the component label.
    #[inline]
    pub fn iter_pixels<'a>(
        &'a self,
        pixels: &'a Buffer2<f32>,
        labels: &'a LabelMap,
    ) -> impl Iterator<Item = Pixel> + 'a {
        let width = pixels.width();
        (self.y_min..=self.y_max).flat_map(move |y| {
            (self.x_min..=self.x_max).filter_map(move |x| {
                let idx = y * width + x;
                if labels[idx] == self.label {
                    Some(Pixel {
                        x,
                        y,
                        value: pixels[idx],
                    })
                } else {
                    None
                }
            })
        })
    }

    /// Find the peak pixel (maximum value) in this component.
    #[inline]
    pub fn find_peak(&self, pixels: &Buffer2<f32>, labels: &LabelMap) -> Pixel {
        self.iter_pixels(pixels, labels)
            .max_by(|a, b| {
                a.value
                    .partial_cmp(&b.value)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or(Pixel {
                x: self.x_min,
                y: self.y_min,
                value: 0.0,
            })
    }
}

// ============================================================================
// Public API
// ============================================================================

/// Deblend a component using local maxima detection.
///
/// Combines `find_local_maxima` and `deblend_by_nearest_peak`.
/// Returns a single candidate if no deblending is needed, or multiple
/// candidates if the component contains multiple peaks.
///
/// Uses `ArrayVec` to avoid heap allocation.
pub fn deblend_local_maxima(
    data: &ComponentData,
    pixels: &Buffer2<f32>,
    labels: &LabelMap,
    config: &DeblendConfig,
) -> ArrayVec<DeblendedCandidate, MAX_PEAKS> {
    debug_assert_eq!(
        (pixels.width(), pixels.height()),
        (labels.width(), labels.height()),
        "pixels and labels must have same dimensions"
    );

    let peaks = find_local_maxima(data, pixels, labels, config);

    if peaks.len() <= 1 {
        // Single peak - create one candidate
        let peak = if peaks.is_empty() {
            data.find_peak(pixels, labels)
        } else {
            peaks[0]
        };

        let mut result = ArrayVec::new();
        result.push(DeblendedCandidate {
            x_min: data.x_min,
            x_max: data.x_max,
            y_min: data.y_min,
            y_max: data.y_max,
            peak_x: peak.x,
            peak_y: peak.y,
            peak_value: peak.value,
            area: data.area,
        });
        result
    } else {
        // Multiple peaks - deblend by assigning pixels to nearest peak
        deblend_by_nearest_peak(data, pixels, labels, &peaks)
    }
}

/// Find local maxima within a component for deblending.
///
/// A pixel is a local maximum if it's greater than all 8 neighbors.
/// Only returns peaks that are sufficiently separated and prominent.
/// Returns at most `MAX_PEAKS` peaks to avoid heap allocation.
pub fn find_local_maxima(
    data: &ComponentData,
    pixels: &Buffer2<f32>,
    labels: &LabelMap,
    config: &DeblendConfig,
) -> ArrayVec<Pixel, MAX_PEAKS> {
    debug_assert_eq!(
        (pixels.width(), pixels.height()),
        (labels.width(), labels.height()),
        "pixels and labels must have same dimensions"
    );

    let mut peaks: ArrayVec<Pixel, MAX_PEAKS> = ArrayVec::new();

    // Find global max using find_peak (single iteration)
    let global_peak = data.find_peak(pixels, labels);
    let min_peak_value = global_peak.value * config.min_prominence;
    let min_sep_sq = config.min_separation * config.min_separation;

    for pixel in data.iter_pixels(pixels, labels) {
        if pixel.value < min_peak_value {
            continue;
        }

        if !is_local_maximum(pixel, pixels) {
            continue;
        }

        add_or_replace_peak(&mut peaks, pixel, min_sep_sq);
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
/// creating separate candidates. Uses fixed-size arrays to avoid heap allocation.
pub fn deblend_by_nearest_peak(
    data: &ComponentData,
    pixels: &Buffer2<f32>,
    labels: &LabelMap,
    peaks: &[Pixel],
) -> ArrayVec<DeblendedCandidate, MAX_PEAKS> {
    debug_assert_eq!(
        (pixels.width(), pixels.height()),
        (labels.width(), labels.height()),
        "pixels and labels must have same dimensions"
    );

    let mut result: ArrayVec<DeblendedCandidate, MAX_PEAKS> = ArrayVec::new();

    if peaks.is_empty() {
        return result;
    }

    let mut peak_data: [PeakData; MAX_PEAKS] = [PeakData::default(); MAX_PEAKS];
    let num_peaks = peaks.len().min(MAX_PEAKS);

    // Assign each pixel to nearest peak
    for pixel in data.iter_pixels(pixels, labels) {
        let nearest = find_nearest_peak(pixel, peaks, num_peaks);
        let pd = &mut peak_data[nearest];
        pd.x_min = pd.x_min.min(pixel.x);
        pd.x_max = pd.x_max.max(pixel.x);
        pd.y_min = pd.y_min.min(pixel.y);
        pd.y_max = pd.y_max.max(pixel.y);
        pd.area += 1;
    }

    // Build candidates
    for (peak, pd) in peaks.iter().take(num_peaks).zip(peak_data.iter()) {
        if pd.area > 0 {
            result.push(DeblendedCandidate {
                x_min: pd.x_min,
                x_max: pd.x_max,
                y_min: pd.y_min,
                y_max: pd.y_max,
                peak_x: peak.x,
                peak_y: peak.y,
                peak_value: peak.value,
                area: pd.area,
            });
        }
    }

    result
}

// ============================================================================
// Internal helpers
// ============================================================================

/// Check if a pixel is a local maximum (greater than all 8 neighbors).
/// Uses explicit neighbor checks instead of loops for better performance.
#[inline]
fn is_local_maximum(pixel: Pixel, pixels: &Buffer2<f32>) -> bool {
    let x = pixel.x;
    let y = pixel.y;
    let v = pixel.value;
    let width = pixels.width();
    let height = pixels.height();

    // Check all 8 neighbors explicitly (avoids loop overhead)
    (x == 0 || pixels[(x - 1, y)] < v)
        && (x + 1 >= width || pixels[(x + 1, y)] < v)
        && (y == 0 || pixels[(x, y - 1)] < v)
        && (y + 1 >= height || pixels[(x, y + 1)] < v)
        && (x == 0 || y == 0 || pixels[(x - 1, y - 1)] < v)
        && (x + 1 >= width || y == 0 || pixels[(x + 1, y - 1)] < v)
        && (x == 0 || y + 1 >= height || pixels[(x - 1, y + 1)] < v)
        && (x + 1 >= width || y + 1 >= height || pixels[(x + 1, y + 1)] < v)
}

/// Add a peak to the list, or replace an existing nearby peak if this one is brighter.
/// Uses squared Euclidean distance for consistency with `find_nearest_peak`.
#[inline]
fn add_or_replace_peak(peaks: &mut ArrayVec<Pixel, MAX_PEAKS>, pixel: Pixel, min_sep_sq: usize) {
    // Check separation from existing peaks using squared Euclidean distance
    let well_separated = peaks.iter().all(|peak| {
        let dx = (pixel.x as i32 - peak.x as i32).unsigned_abs() as usize;
        let dy = (pixel.y as i32 - peak.y as i32).unsigned_abs() as usize;
        dx * dx + dy * dy >= min_sep_sq
    });

    if well_separated {
        if !peaks.is_full() {
            peaks.push(pixel);
        }
    } else {
        // Replace nearby peak if this one is brighter
        for peak in peaks.iter_mut() {
            let dx = (pixel.x as i32 - peak.x as i32).unsigned_abs() as usize;
            let dy = (pixel.y as i32 - peak.y as i32).unsigned_abs() as usize;
            if dx * dx + dy * dy < min_sep_sq && pixel.value > peak.value {
                *peak = pixel;
                break;
            }
        }
    }
}

/// Find the index of the nearest peak to a pixel.
#[inline]
fn find_nearest_peak(pixel: Pixel, peaks: &[Pixel], num_peaks: usize) -> usize {
    let mut min_dist_sq = usize::MAX;
    let mut nearest = 0;

    for (i, peak) in peaks.iter().take(num_peaks).enumerate() {
        let dx = (pixel.x as i32 - peak.x as i32).unsigned_abs() as usize;
        let dy = (pixel.y as i32 - peak.y as i32).unsigned_abs() as usize;
        let dist_sq = dx * dx + dy * dy;

        if dist_sq < min_dist_sq {
            min_dist_sq = dist_sq;
            nearest = i;
        }
    }

    nearest
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a test image with Gaussian stars and return pixels, labels, and component data.
    fn make_test_component(
        width: usize,
        height: usize,
        stars: &[(usize, usize, f32, f32)], // (cx, cy, amplitude, sigma)
    ) -> (Buffer2<f32>, LabelMap, ComponentData) {
        let mut pixels = Buffer2::new_filled(width, height, 0.0f32);
        let mut labels = Buffer2::new_filled(width, height, 0u32);

        let mut x_min = usize::MAX;
        let mut x_max = 0;
        let mut y_min = usize::MAX;
        let mut y_max = 0;
        let mut area = 0;

        for (cx, cy, amplitude, sigma) in stars {
            let radius = (sigma * 4.0).ceil() as i32;

            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    let x = (*cx as i32 + dx) as usize;
                    let y = (*cy as i32 + dy) as usize;

                    if x < width && y < height {
                        let r2 = (dx * dx + dy * dy) as f32;
                        let value = amplitude * (-r2 / (2.0 * sigma * sigma)).exp();
                        if value > 0.001 {
                            pixels[(x, y)] += value;
                            if labels[(x, y)] == 0 {
                                labels[(x, y)] = 1;
                                x_min = x_min.min(x);
                                x_max = x_max.max(x);
                                y_min = y_min.min(y);
                                y_max = y_max.max(y);
                                area += 1;
                            }
                        }
                    }
                }
            }
        }

        let label_map = LabelMap::from_raw(labels, 1);
        let component = ComponentData {
            x_min,
            x_max,
            y_min,
            y_max,
            label: 1,
            area,
        };

        (pixels, label_map, component)
    }

    #[test]
    fn test_find_single_peak() {
        let (pixels, labels, data) = make_test_component(100, 100, &[(50, 50, 1.0, 3.0)]);

        let config = DeblendConfig::default();
        let peaks = find_local_maxima(&data, &pixels, &labels, &config);

        assert_eq!(peaks.len(), 1, "Should find exactly one peak");
        assert!(
            (peaks[0].x as i32 - 50).abs() <= 1,
            "Peak should be near center"
        );
    }

    #[test]
    fn test_find_two_peaks() {
        let (pixels, labels, data) =
            make_test_component(100, 100, &[(30, 50, 1.0, 2.5), (70, 50, 0.8, 2.5)]);

        let config = DeblendConfig {
            min_separation: 3,
            min_prominence: 0.3,
            ..Default::default()
        };
        let peaks = find_local_maxima(&data, &pixels, &labels, &config);

        assert_eq!(peaks.len(), 2, "Should find two peaks");
    }

    #[test]
    fn test_deblend_creates_separate_candidates() {
        let (pixels, labels, data) =
            make_test_component(100, 100, &[(30, 50, 1.0, 2.5), (70, 50, 0.8, 2.5)]);

        let config = DeblendConfig {
            min_separation: 3,
            min_prominence: 0.3,
            ..Default::default()
        };

        let candidates = deblend_local_maxima(&data, &pixels, &labels, &config);

        assert_eq!(candidates.len(), 2, "Should create two candidates");
        assert!(candidates[0].area > 0);
        assert!(candidates[1].area > 0);
    }

    #[test]
    fn test_iter_pixels_count() {
        let (pixels, labels, data) = make_test_component(100, 100, &[(50, 50, 1.0, 3.0)]);

        let iter_count = data.iter_pixels(&pixels, &labels).count();
        assert_eq!(
            iter_count, data.area,
            "iter_pixels should yield exactly area pixels"
        );
    }

    #[test]
    fn test_euclidean_separation() {
        // Two peaks at distance sqrt(18) ≈ 4.24 apart (diagonal)
        // With min_separation=5, they should be merged (5^2=25 > 18)
        // With min_separation=4, they should be separate (4^2=16 < 18)
        let (pixels, labels, data) =
            make_test_component(100, 100, &[(50, 50, 1.0, 1.5), (53, 53, 0.9, 1.5)]);

        let config_merge = DeblendConfig {
            min_separation: 5,
            min_prominence: 0.3,
            ..Default::default()
        };
        let peaks_merge = find_local_maxima(&data, &pixels, &labels, &config_merge);
        assert_eq!(peaks_merge.len(), 1, "Close peaks should merge");

        let config_separate = DeblendConfig {
            min_separation: 4,
            min_prominence: 0.3,
            ..Default::default()
        };
        let peaks_separate = find_local_maxima(&data, &pixels, &labels, &config_separate);
        assert_eq!(peaks_separate.len(), 2, "Distant peaks should separate");
    }

    #[test]
    fn test_prominence_filter() {
        // Bright primary peak and dim secondary that should be filtered
        let (pixels, labels, data) =
            make_test_component(100, 100, &[(30, 50, 1.0, 2.5), (70, 50, 0.2, 2.5)]);

        // With high prominence threshold, only bright peak survives
        let config_high = DeblendConfig {
            min_separation: 3,
            min_prominence: 0.5,
            ..Default::default()
        };
        let peaks = find_local_maxima(&data, &pixels, &labels, &config_high);
        assert_eq!(peaks.len(), 1, "Dim peak should be filtered by prominence");

        // With low prominence threshold, both peaks survive
        let config_low = DeblendConfig {
            min_separation: 3,
            min_prominence: 0.1,
            ..Default::default()
        };
        let peaks = find_local_maxima(&data, &pixels, &labels, &config_low);
        assert_eq!(peaks.len(), 2, "Both peaks should pass low prominence");
    }

    #[test]
    fn test_deblend_empty_peaks() {
        let (pixels, labels, data) = make_test_component(100, 100, &[(50, 50, 1.0, 3.0)]);
        let empty_peaks: &[Pixel] = &[];

        let candidates = deblend_by_nearest_peak(&data, &pixels, &labels, empty_peaks);
        assert!(
            candidates.is_empty(),
            "Empty peaks should return empty result"
        );
    }

    #[test]
    fn test_deblend_single_peak_returns_full_component() {
        let (pixels, labels, data) = make_test_component(100, 100, &[(50, 50, 1.0, 3.0)]);

        let config = DeblendConfig::default();
        let candidates = deblend_local_maxima(&data, &pixels, &labels, &config);

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].area, data.area);
        assert_eq!(candidates[0].x_min, data.x_min);
        assert_eq!(candidates[0].x_max, data.x_max);
    }

    #[test]
    fn test_peaks_sorted_by_brightness() {
        let (pixels, labels, data) = make_test_component(
            100,
            100,
            &[(30, 50, 0.5, 2.5), (50, 50, 1.0, 2.5), (70, 50, 0.7, 2.5)],
        );

        let config = DeblendConfig {
            min_separation: 3,
            min_prominence: 0.3,
            ..Default::default()
        };
        let peaks = find_local_maxima(&data, &pixels, &labels, &config);

        assert_eq!(peaks.len(), 3);
        assert!(
            peaks[0].value >= peaks[1].value && peaks[1].value >= peaks[2].value,
            "Peaks should be sorted by brightness descending"
        );
    }

    #[test]
    fn test_find_peak_returns_global_max() {
        let (pixels, labels, data) = make_test_component(
            100,
            100,
            &[(30, 50, 0.5, 2.5), (50, 50, 1.0, 2.5), (70, 50, 0.7, 2.5)],
        );

        let peak = data.find_peak(&pixels, &labels);
        assert!(
            (peak.x as i32 - 50).abs() <= 1 && (peak.y as i32 - 50).abs() <= 1,
            "find_peak should return the brightest star's position"
        );
        assert!(peak.value > 0.9, "Peak value should be close to 1.0");
    }

    #[test]
    fn test_deblend_area_conservation() {
        // Total area of deblended candidates should equal original component area
        let (pixels, labels, data) =
            make_test_component(100, 100, &[(30, 50, 1.0, 2.5), (70, 50, 0.8, 2.5)]);

        let config = DeblendConfig {
            min_separation: 3,
            min_prominence: 0.3,
            ..Default::default()
        };
        let candidates = deblend_local_maxima(&data, &pixels, &labels, &config);

        let total_area: usize = candidates.iter().map(|c| c.area).sum();
        assert_eq!(
            total_area, data.area,
            "Deblending should conserve total area"
        );
    }

    #[test]
    fn test_peak_replacement_when_brighter() {
        // Two very close peaks - brighter one should replace dimmer
        let (pixels, labels, data) =
            make_test_component(100, 100, &[(50, 50, 1.0, 1.5), (51, 50, 0.8, 1.5)]);

        let config = DeblendConfig {
            min_separation: 5, // Force them to be "too close"
            min_prominence: 0.3,
            ..Default::default()
        };
        let peaks = find_local_maxima(&data, &pixels, &labels, &config);

        assert_eq!(peaks.len(), 1, "Should merge to single peak");
        assert!(
            peaks[0].value > 0.9,
            "Merged peak should be the brighter one"
        );
    }

    #[test]
    fn test_is_local_maximum_edge_cases() {
        // Test local maximum detection at image boundaries
        let mut pixels = Buffer2::new_filled(10, 10, 0.0f32);

        // Corner pixel (0,0) as local max
        pixels[(0, 0)] = 1.0;
        pixels[(1, 0)] = 0.5;
        pixels[(0, 1)] = 0.5;
        pixels[(1, 1)] = 0.5;

        let corner_pixel = Pixel {
            x: 0,
            y: 0,
            value: 1.0,
        };
        assert!(
            is_local_maximum(corner_pixel, &pixels),
            "Corner pixel should be local max"
        );

        // Edge pixel
        pixels[(5, 0)] = 1.0;
        pixels[(4, 0)] = 0.5;
        pixels[(6, 0)] = 0.5;
        pixels[(4, 1)] = 0.5;
        pixels[(5, 1)] = 0.5;
        pixels[(6, 1)] = 0.5;

        let edge_pixel = Pixel {
            x: 5,
            y: 0,
            value: 1.0,
        };
        assert!(
            is_local_maximum(edge_pixel, &pixels),
            "Edge pixel should be local max"
        );
    }

    #[test]
    fn test_is_local_maximum_not_max() {
        let mut pixels = Buffer2::new_filled(10, 10, 0.0f32);

        // Center pixel with brighter neighbor
        pixels[(5, 5)] = 0.5;
        pixels[(6, 5)] = 1.0; // Brighter neighbor

        let pixel = Pixel {
            x: 5,
            y: 5,
            value: 0.5,
        };
        assert!(
            !is_local_maximum(pixel, &pixels),
            "Pixel with brighter neighbor is not local max"
        );
    }

    #[test]
    fn test_voronoi_partitioning() {
        // Create component with two well-separated peaks
        let (pixels, labels, data) =
            make_test_component(100, 100, &[(25, 50, 1.0, 3.0), (75, 50, 1.0, 3.0)]);

        let peaks = vec![
            Pixel {
                x: 25,
                y: 50,
                value: 1.0,
            },
            Pixel {
                x: 75,
                y: 50,
                value: 1.0,
            },
        ];

        let candidates = deblend_by_nearest_peak(&data, &pixels, &labels, &peaks);

        assert_eq!(candidates.len(), 2);
        // Each candidate should have its peak inside its bounding box
        for candidate in &candidates {
            assert!(candidate.peak_x >= candidate.x_min && candidate.peak_x <= candidate.x_max);
            assert!(candidate.peak_y >= candidate.y_min && candidate.peak_y <= candidate.y_max);
        }
    }

    #[test]
    fn test_many_peaks_limited_to_max() {
        // Create component with more peaks than MAX_PEAKS
        let stars: Vec<_> = (0..12)
            .map(|i| (10 + i * 8, 50usize, 1.0 - i as f32 * 0.05, 1.5f32))
            .collect();

        let (pixels, labels, data) = make_test_component(120, 100, &stars);

        let config = DeblendConfig {
            min_separation: 2,
            min_prominence: 0.1,
            ..Default::default()
        };
        let peaks = find_local_maxima(&data, &pixels, &labels, &config);

        assert!(
            peaks.len() <= MAX_PEAKS,
            "Should not exceed MAX_PEAKS ({}), got {}",
            MAX_PEAKS,
            peaks.len()
        );
    }
}
