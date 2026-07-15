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

use std::cmp::Ordering;

use arrayvec::ArrayVec;

use crate::stacking::star_detection::deblend::region::Region;
use crate::stacking::star_detection::deblend::{
    ComponentData, MAX_PEAKS, Pixel, assign_to_nearest_peak, peaks_too_close,
};
use crate::stacking::star_detection::labeling::LabelMap;
use imaginarium::Buffer2;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod bench;

/// Deblend a component using local maxima detection.
///
/// Combines `find_local_maxima` and the shared nearest-peak assignment.
/// Returns a single candidate if no deblending is needed, or multiple
/// candidates if the component contains multiple peaks.
///
/// Uses `ArrayVec` to avoid heap allocation.
pub fn deblend_local_maxima(
    data: &ComponentData,
    pixels: &Buffer2<f32>,
    labels: &LabelMap,
    min_separation: usize,
    min_prominence: f32,
) -> ArrayVec<Region, MAX_PEAKS> {
    debug_assert_eq!(
        (pixels.width(), pixels.height()),
        (labels.width(), labels.height()),
        "pixels and labels must have same dimensions"
    );

    let peaks = find_local_maxima(data, pixels, labels, min_separation, min_prominence);

    if peaks.len() <= 1 {
        // Single peak - create one candidate
        let peak = if peaks.is_empty() {
            data.find_peak(pixels, labels)
        } else {
            peaks[0]
        };

        let mut result = ArrayVec::new();
        result.push(Region {
            bbox: data.bbox,
            peak: peak.pos,
            peak_value: peak.value,
            area: data.area,
        });
        result
    } else {
        // Multiple peaks - deblend by assigning pixels to nearest peak
        assign_to_nearest_peak(data, pixels, labels, &peaks)
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
    min_separation: usize,
    min_prominence: f32,
) -> ArrayVec<Pixel, MAX_PEAKS> {
    debug_assert_eq!(
        (pixels.width(), pixels.height()),
        (labels.width(), labels.height()),
        "pixels and labels must have same dimensions"
    );

    let mut peaks: ArrayVec<Pixel, MAX_PEAKS> = ArrayVec::new();

    // Find global max using find_peak (single iteration)
    let global_peak = data.find_peak(pixels, labels);
    let min_peak_value = global_peak.value * min_prominence;
    let min_sep_sq = min_separation * min_separation;

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
    peaks.sort_by(|a, b| b.value.partial_cmp(&a.value).unwrap_or(Ordering::Equal));

    peaks
}

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
/// Uses squared Euclidean distance (via [`peaks_too_close`]), matching the shared
/// nearest-peak assignment.
#[inline]
fn add_or_replace_peak(peaks: &mut ArrayVec<Pixel, MAX_PEAKS>, pixel: Pixel, min_sep_sq: usize) {
    let well_separated = peaks
        .iter()
        .all(|peak| !peaks_too_close(pixel.pos, peak.pos, min_sep_sq));

    if well_separated {
        if !peaks.is_full() {
            peaks.push(pixel);
        }
    } else {
        // Replace nearby peak if this one is brighter
        for peak in peaks.iter_mut() {
            if peaks_too_close(pixel.pos, peak.pos, min_sep_sq) && pixel.value > peak.value {
                *peak = pixel;
                break;
            }
        }
    }
}
