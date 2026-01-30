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
#[derive(Debug, Clone, Copy)]
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

    /// Find the global maximum pixel value in this component.
    #[inline]
    pub fn global_max(&self, pixels: &Buffer2<f32>, labels: &LabelMap) -> f32 {
        self.iter_pixels(pixels, labels)
            .map(|p| p.value)
            .fold(f32::MIN, f32::max)
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
    let mut peaks: ArrayVec<Pixel, MAX_PEAKS> = ArrayVec::new();
    let width = pixels.width();
    let height = pixels.height();

    let global_max = data.global_max(pixels, labels);
    let min_peak_value = global_max * config.min_prominence;

    for pixel in data.iter_pixels(pixels, labels) {
        if pixel.value < min_peak_value {
            continue;
        }

        if !is_local_maximum(pixel, pixels, width, height) {
            continue;
        }

        add_or_replace_peak(&mut peaks, pixel, config.min_separation);
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
#[inline]
fn is_local_maximum(pixel: Pixel, pixels: &Buffer2<f32>, width: usize, height: usize) -> bool {
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

                if nx < width && ny < height && pixels[(nx, ny)] >= pixel.value {
                    return false;
                }
            }
        }
    }
    true
}

/// Add a peak to the list, or replace an existing nearby peak if this one is brighter.
#[inline]
fn add_or_replace_peak(
    peaks: &mut ArrayVec<Pixel, MAX_PEAKS>,
    pixel: Pixel,
    min_separation: usize,
) {
    // Check separation from existing peaks
    let well_separated = peaks.iter().all(|peak| {
        let dx = (pixel.x as i32 - peak.x as i32).unsigned_abs() as usize;
        let dy = (pixel.y as i32 - peak.y as i32).unsigned_abs() as usize;
        dx >= min_separation || dy >= min_separation
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
            if dx < min_separation && dy < min_separation && pixel.value > peak.value {
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

    fn make_gaussian_image(
        width: usize,
        height: usize,
        stars: &[(usize, usize, f32, f32)], // (cx, cy, amplitude, sigma)
    ) -> (Buffer2<f32>, LabelMap) {
        let mut pixels = Buffer2::new_filled(width, height, 0.0f32);
        let mut labels = Buffer2::new_filled(width, height, 0u32);

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
                            labels[(x, y)] = 1;
                        }
                    }
                }
            }
        }

        let label_map = LabelMap::from_raw(labels, 1);
        (pixels, label_map)
    }

    fn compute_bbox(labels: &LabelMap, label: u32) -> (usize, usize, usize, usize, usize) {
        let mut x_min = usize::MAX;
        let mut x_max = 0;
        let mut y_min = usize::MAX;
        let mut y_max = 0;
        let mut area = 0;
        let width = labels.width();

        for (idx, &l) in labels.iter().enumerate() {
            if l == label {
                let x = idx % width;
                let y = idx / width;
                x_min = x_min.min(x);
                x_max = x_max.max(x);
                y_min = y_min.min(y);
                y_max = y_max.max(y);
                area += 1;
            }
        }

        (x_min, x_max, y_min, y_max, area)
    }

    fn make_component(labels: &LabelMap, label: u32) -> ComponentData {
        let (x_min, x_max, y_min, y_max, area) = compute_bbox(labels, label);
        ComponentData {
            x_min,
            x_max,
            y_min,
            y_max,
            label,
            area,
        }
    }

    #[test]
    fn test_find_single_peak() {
        let (pixels, labels) = make_gaussian_image(100, 100, &[(50, 50, 1.0, 3.0)]);
        let data = make_component(&labels, 1);

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
        let (pixels, labels) =
            make_gaussian_image(100, 100, &[(30, 50, 1.0, 2.5), (70, 50, 0.8, 2.5)]);
        let data = make_component(&labels, 1);

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
        let (pixels, labels) =
            make_gaussian_image(100, 100, &[(30, 50, 1.0, 2.5), (70, 50, 0.8, 2.5)]);
        let data = make_component(&labels, 1);

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
        let (pixels, labels) = make_gaussian_image(100, 100, &[(50, 50, 1.0, 3.0)]);
        let data = make_component(&labels, 1);

        let iter_count = data.iter_pixels(&pixels, &labels).count();
        assert_eq!(
            iter_count, data.area,
            "iter_pixels should yield exactly area pixels"
        );
    }
}
