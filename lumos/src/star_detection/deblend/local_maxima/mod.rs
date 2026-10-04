//! Deblending by local maxima.
//!
//! 1. Collect every local maximum of the component at least `min_prominence` of its brightest
//!    pixel.
//! 2. Rank them brightest first and keep each one that is at least `min_separation` from every
//!    brighter one kept — greedy non-maximum suppression, as in photutils' `find_peaks` and
//!    scikit-image's `peak_local_max`.
//! 3. Assign every pixel to its nearest kept peak (Voronoi partition).

use imaginarium::Buffer2;

use crate::math::vec2us::Vec2us;
use crate::star_detection::deblend::component::Component;
use crate::star_detection::deblend::component_pixels::ComponentPixels;
use crate::star_detection::deblend::deblend_buffers::DeblendBuffers;
use crate::star_detection::deblend::region::Region;
use crate::star_detection::deblend::{Pixel, peaks_too_close};

/// What the local-maxima deblender reads from the detection configuration.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LocalMaximaParams {
    /// Minimum distance from a kept peak to every brighter one kept, in pixels.
    pub(crate) min_separation: usize,
    /// A local maximum's least value, as a share of the component's brightest pixel.
    pub(crate) min_prominence: f32,
}

impl LocalMaximaParams {
    /// Split `component` at its local maxima onto `out`: one region when fewer than two survive,
    /// else one region per surviving peak. Returns how many it pushed. `buffers` is scratch the
    /// caller keeps across components.
    pub(crate) fn deblend(
        self,
        component: &Component<'_>,
        buffers: &mut DeblendBuffers,
        out: &mut Vec<Region>,
    ) -> usize {
        let Self {
            min_separation,
            min_prominence,
        } = self;
        let DeblendBuffers {
            pixels,
            maxima,
            peaks,
            occupied,
            assignment,
            ..
        } = buffers;
        pixels.fill(component);
        find_local_maxima(
            component,
            pixels,
            min_separation,
            min_prominence,
            maxima,
            Kept { peaks, occupied },
        );
        component.split_at(peaks, assignment, out)
    }
}

/// Where [`find_local_maxima`] puts the peaks it keeps, and the per-pixel map of them it checks
/// each candidate's neighbourhood in.
#[derive(Debug)]
struct Kept<'a> {
    peaks: &'a mut Vec<Pixel>,
    occupied: &'a mut Vec<bool>,
}

/// Into `kept.peaks`, the local maxima of `component` that are at least `min_prominence` of its
/// peak and `min_separation` from every brighter one kept, brightest first.
///
/// All candidates are collected before any is kept: a candidate too close to a brighter one
/// that is itself suppressed must still be kept, which a single pass in raster order cannot see.
/// Each candidate checks only the kept peaks inside its separation, in a map of the component's
/// pixels, so a component holding thousands of peaks costs candidates × separation², not
/// candidates × peaks, and its memory follows its pixels, not its box.
fn find_local_maxima(
    component: &Component<'_>,
    pixels: &ComponentPixels,
    min_separation: usize,
    min_prominence: f32,
    maxima: &mut Vec<Pixel>,
    kept: Kept<'_>,
) {
    let Kept { peaks, occupied } = kept;
    let residual = component.residual();
    let min_peak_value = component.peak().value * min_prominence;
    maxima.clear();
    maxima.extend(
        pixels
            .pixels
            .iter()
            .copied()
            .filter(|&p| p.value >= min_peak_value && is_local_maximum(p, residual)),
    );
    maxima.sort_unstable_by(Pixel::brighter_first);

    occupied.clear();
    occupied.resize(pixels.pixels.len(), false);
    let reach = min_separation.saturating_sub(1);
    let min_sep_sq = min_separation * min_separation;
    peaks.clear();
    for &candidate in maxima.iter() {
        let pos = candidate.pos;
        let crowded = (pos.y.saturating_sub(reach)..=pos.y + reach).any(|y| {
            (pos.x.saturating_sub(reach)..=pos.x + reach).any(|x| {
                let near = Vec2us::new(x, y);
                pixels.index_of(near).is_some_and(|index| occupied[index])
                    && peaks_too_close(pos, near, min_sep_sq)
            })
        });
        if !crowded {
            occupied[pixels
                .index_of(pos)
                .expect("a maximum is a pixel of the component")] = true;
            peaks.push(candidate);
        }
    }
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
