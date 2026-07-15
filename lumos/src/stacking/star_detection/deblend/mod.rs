//! Star deblending algorithms for separating overlapping sources.
//!
//! This module provides two deblending approaches:
//!
//! 1. **Local maxima deblending** (`local_maxima`): Fast algorithm that finds
//!    peaks in a connected component and assigns pixels to the nearest peak.
//!    Good for well-separated stars.
//!
//! 2. **Multi-threshold deblending** (`multi_threshold`): SExtractor-style
//!    tree-based algorithm that uses multiple threshold levels to separate
//!    blended sources. More accurate for crowded fields but slower.

use std::cmp::Ordering;

use arrayvec::ArrayVec;

use crate::math::rect::URect;
use crate::stacking::star_detection::labeling::LabelMap;
use common::Vec2us;
use imaginarium::Buffer2;

pub(crate) mod local_maxima;
pub(crate) mod multi_threshold;
pub(crate) mod region;

use region::Region;

#[cfg(test)]
mod tests;

/// Maximum number of peaks/candidates per component.
/// Components with more peaks than this will have excess peaks ignored.
pub(crate) const MAX_PEAKS: usize = 8;

/// A pixel with its coordinates and value.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Pixel {
    pub pos: Vec2us,
    pub value: f32,
}

/// Data for a connected component (allocation-free).
///
/// Instead of storing pixel coordinates, we store the component label
/// and iterate over the bounding box on-demand, checking the labels buffer.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ComponentData {
    /// Bounding box of the component.
    pub bbox: URect,
    /// Component label in the labels buffer.
    pub label: u32,
    /// Number of pixels in the component (pre-computed).
    pub area: usize,
}

impl ComponentData {
    /// Iterate over all pixels in this component.
    ///
    /// Scans the bounding box and yields pixels that match the component label.
    #[inline]
    pub(crate) fn iter_pixels<'a>(
        &'a self,
        pixels: &'a Buffer2<f32>,
        labels: &'a LabelMap,
    ) -> impl Iterator<Item = Pixel> + 'a {
        let width = pixels.width();
        let bbox = &self.bbox;
        (bbox.min.y..bbox.max.y).flat_map(move |y| {
            (bbox.min.x..bbox.max.x).filter_map(move |x| {
                let idx = y * width + x;
                if labels[idx] == self.label {
                    Some(Pixel {
                        pos: Vec2us::new(x, y),
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
    pub(crate) fn find_peak(&self, pixels: &Buffer2<f32>, labels: &LabelMap) -> Pixel {
        self.iter_pixels(pixels, labels)
            .max_by(|a, b| a.value.partial_cmp(&b.value).unwrap_or(Ordering::Equal))
            .expect("component must have at least one pixel")
    }
}

/// Assign every pixel of `data` to its nearest `peak` (squared-Euclidean Voronoi; the first peak wins
/// ties) and build one [`Region`] per peak, accumulating bounding box and area and dropping peaks that
/// captured no pixels. Peaks beyond [`MAX_PEAKS`] are ignored. This is the shared tail of both the
/// local-maxima and multi-threshold deblenders.
pub(crate) fn assign_to_nearest_peak(
    data: &ComponentData,
    pixels: &Buffer2<f32>,
    labels: &LabelMap,
    peaks: &[Pixel],
) -> ArrayVec<Region, MAX_PEAKS> {
    let mut result = ArrayVec::new();
    if peaks.is_empty() {
        return result;
    }
    let peaks = &peaks[..peaks.len().min(MAX_PEAKS)];

    // Per-peak (bbox, area) accumulators, indexed like `peaks`.
    let mut acc = [(URect::empty(), 0usize); MAX_PEAKS];
    for pixel in data.iter_pixels(pixels, labels) {
        let nearest = nearest_peak_index(pixel.pos, peaks);
        acc[nearest].0.include(pixel.pos);
        acc[nearest].1 += 1;
    }

    for (peak, &(bbox, area)) in peaks.iter().zip(acc.iter()) {
        if area > 0 {
            assert!(
                bbox.contains(peak.pos),
                "assigned region must contain its peak"
            );
            result.push(Region {
                bbox,
                peak: peak.pos,
                peak_value: peak.value,
                area,
            });
        }
    }
    result
}

/// Squared Euclidean distance between two pixel positions — the one peak-separation
/// metric shared by the Voronoi assignment and every min-separation check.
#[inline]
fn dist_sq(a: Vec2us, b: Vec2us) -> usize {
    let dx = (a.x as i32 - b.x as i32).unsigned_abs() as usize;
    let dy = (a.y as i32 - b.y as i32).unsigned_abs() as usize;
    dx * dx + dy * dy
}

/// Index of the nearest peak to `pos` by squared Euclidean distance; the first peak wins ties.
fn nearest_peak_index(pos: Vec2us, peaks: &[Pixel]) -> usize {
    let mut min_dist_sq = usize::MAX;
    let mut nearest = 0;
    for (i, peak) in peaks.iter().enumerate() {
        let d = dist_sq(pos, peak.pos);
        if d < min_dist_sq {
            min_dist_sq = d;
            nearest = i;
        }
    }
    nearest
}

/// Whether two peak positions are closer than a squared-distance threshold, by the same
/// squared Euclidean metric [`nearest_peak_index`] uses for the Voronoi assignment every
/// deblender's peaks eventually feed into. `min_sep_sq` is
/// `min_separation * min_separation`, pre-squared by the caller since peak-separation
/// checks run in a loop over many candidate pairs.
#[inline]
pub(crate) fn peaks_too_close(a: Vec2us, b: Vec2us, min_sep_sq: usize) -> bool {
    dist_sq(a, b) < min_sep_sq
}
