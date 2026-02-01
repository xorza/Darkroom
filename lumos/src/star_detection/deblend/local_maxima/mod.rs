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

use super::{ComponentData, DeblendConfig, DeblendedCandidate, MAX_PEAKS, Pixel};
use crate::common::Buffer2;
use crate::math::Aabb;
use crate::star_detection::candidate_detection::LabelMap;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod bench;

/// Per-peak bounding box and area data (internal).
#[derive(Debug, Copy, Clone)]
struct PeakData {
    bbox: Aabb,
    area: usize,
}

impl Default for PeakData {
    fn default() -> Self {
        Self {
            bbox: Aabb::empty(),
            area: 0,
        }
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
            bbox: data.bbox,
            peak: peak.pos,
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
        pd.bbox.include(pixel.pos);
        pd.area += 1;
    }

    // Build candidates
    for (peak, pd) in peaks.iter().take(num_peaks).zip(peak_data.iter()) {
        if pd.area > 0 {
            result.push(DeblendedCandidate {
                bbox: pd.bbox,
                peak: peak.pos,
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
pub(crate) fn is_local_maximum(pixel: Pixel, pixels: &Buffer2<f32>) -> bool {
    let x = pixel.pos.x;
    let y = pixel.pos.y;
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
        let dx = (pixel.pos.x as i32 - peak.pos.x as i32).unsigned_abs() as usize;
        let dy = (pixel.pos.y as i32 - peak.pos.y as i32).unsigned_abs() as usize;
        dx * dx + dy * dy >= min_sep_sq
    });

    if well_separated {
        if !peaks.is_full() {
            peaks.push(pixel);
        }
    } else {
        // Replace nearby peak if this one is brighter
        for peak in peaks.iter_mut() {
            let dx = (pixel.pos.x as i32 - peak.pos.x as i32).unsigned_abs() as usize;
            let dy = (pixel.pos.y as i32 - peak.pos.y as i32).unsigned_abs() as usize;
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
        let dx = (pixel.pos.x as i32 - peak.pos.x as i32).unsigned_abs() as usize;
        let dy = (pixel.pos.y as i32 - peak.pos.y as i32).unsigned_abs() as usize;
        let dist_sq = dx * dx + dy * dy;

        if dist_sq < min_dist_sq {
            min_dist_sq = dist_sq;
            nearest = i;
        }
    }

    nearest
}
