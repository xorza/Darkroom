//! Deblending by local maxima.
//!
//! 1. Collect every local maximum of the component at least `min_prominence` of its brightest
//!    pixel.
//! 2. Rank them brightest first and keep each one that is at least `min_separation` from every
//!    brighter one kept, up to [`MAX_PEAKS`] — greedy non-maximum suppression, as in photutils'
//!    `find_peaks` and scikit-image's `peak_local_max`.
//! 3. Assign every pixel to its nearest kept peak (Voronoi partition).

use arrayvec::ArrayVec;
use imaginarium::Buffer2;

use crate::stacking::star_detection::deblend::component::Component;
use crate::stacking::star_detection::deblend::region::Region;
use crate::stacking::star_detection::deblend::{MAX_PEAKS, Pixel, peaks_too_close};

/// Split `component` at its local maxima: one region when fewer than two survive, else one
/// region per surviving peak. `maxima` is scratch the caller keeps across components.
pub(crate) fn deblend_local_maxima(
    component: &Component<'_>,
    min_separation: usize,
    min_prominence: f32,
    maxima: &mut Vec<Pixel>,
) -> ArrayVec<Region, MAX_PEAKS> {
    let peaks = find_local_maxima(component, min_separation, min_prominence, maxima);
    if peaks.len() <= 1 {
        let mut result = ArrayVec::new();
        result.push(component.whole());
        result
    } else {
        component.assign_to_nearest(&peaks)
    }
}

/// The brightest [`MAX_PEAKS`] local maxima of `component` that are at least
/// `min_prominence` of its peak and `min_separation` from every brighter one kept, brightest
/// first.
///
/// All candidates are collected before any is kept: a candidate too close to a brighter one
/// that is itself suppressed must still be kept, which a single pass in raster order cannot see.
fn find_local_maxima(
    component: &Component<'_>,
    min_separation: usize,
    min_prominence: f32,
    maxima: &mut Vec<Pixel>,
) -> ArrayVec<Pixel, MAX_PEAKS> {
    let residual = component.residual();
    let min_peak_value = component.peak().value * min_prominence;
    maxima.clear();
    maxima.extend(
        component
            .pixels()
            .filter(|&p| p.value >= min_peak_value && is_local_maximum(p, residual)),
    );
    maxima.sort_unstable_by(Pixel::brighter_first);

    let min_sep_sq = min_separation * min_separation;
    let mut peaks: ArrayVec<Pixel, MAX_PEAKS> = ArrayVec::new();
    for &candidate in maxima.iter() {
        if peaks.is_full() {
            break;
        }
        if peaks
            .iter()
            .all(|kept| !peaks_too_close(candidate.pos, kept.pos, min_sep_sq))
        {
            peaks.push(candidate);
        }
    }
    peaks
}

/// Whether `pixel` is strictly greater than each of its 8 neighbours in `residual`; a neighbour
/// outside the image does not count against it.
#[inline]
fn is_local_maximum(pixel: Pixel, residual: &Buffer2<f32>) -> bool {
    let x = pixel.pos.x;
    let y = pixel.pos.y;
    let v = pixel.value;
    let width = residual.width();
    let height = residual.height();

    (x == 0 || residual[(x - 1, y)] < v)
        && (x + 1 >= width || residual[(x + 1, y)] < v)
        && (y == 0 || residual[(x, y - 1)] < v)
        && (y + 1 >= height || residual[(x, y + 1)] < v)
        && (x == 0 || y == 0 || residual[(x - 1, y - 1)] < v)
        && (x + 1 >= width || y == 0 || residual[(x + 1, y - 1)] < v)
        && (x == 0 || y + 1 >= height || residual[(x - 1, y + 1)] < v)
        && (x + 1 >= width || y + 1 >= height || residual[(x + 1, y + 1)] < v)
}

#[cfg(all(test, feature = "bench"))]
mod bench;
#[cfg(test)]
mod tests;
