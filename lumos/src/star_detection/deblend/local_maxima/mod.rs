//! Deblending by local maxima.
//!
//! 1. Collect every local maximum of the component at least `min_relative_height` of its brightest
//!    pixel: a pixel at least as high as its 8 neighbours, as photutils' `find_peaks` and
//!    scikit-image's `peak_local_max` take one, with ties broken in raster order so a plateau gives
//!    a candidate and the component's own peak is always one.
//! 2. Rank them brightest first and keep each one that is at least `min_separation` from every
//!    brighter one kept — greedy non-maximum suppression, as in those two.
//! 3. Assign every pixel to its nearest kept peak (Voronoi partition).

use common::Buffer2;

use crate::star_detection::deblend;
use crate::star_detection::deblend::component::Component;
use crate::star_detection::deblend::component_pixels::ComponentPixels;
use crate::star_detection::deblend::deblend_buffers::DeblendBuffers;
use crate::star_detection::deblend::pixel::Pixel;
use crate::star_detection::deblend::region::Region;
use glam::USizeVec2;

/// What the local-maxima deblender reads from the detection configuration.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LocalMaximaParams {
    /// Minimum distance from a kept peak to every brighter one kept, in pixels.
    pub(crate) min_separation: usize,
    /// A local maximum's least value, as a share of the component's brightest pixel.
    pub(crate) min_relative_height: f32,
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
        let DeblendBuffers {
            pixels,
            maxima,
            peaks,
            occupied,
            assignment,
            ..
        } = buffers;
        self.find_local_maxima(
            component,
            pixels,
            Kept {
                maxima,
                peaks,
                occupied,
            },
        );
        component.split_at(peaks, assignment, out)
    }

    /// Into `kept.peaks`, the local maxima of `component` that are at least `min_relative_height`
    /// of its peak and `min_separation` from every brighter one kept, brightest first.
    ///
    /// All candidates are collected before any is kept: a candidate too close to a brighter one
    /// that is itself suppressed must still be kept, which a single pass in raster order cannot
    /// see. Each candidate checks only the kept peaks inside its separation, in a map of the
    /// component's pixels built into `pixels`, so a component holding thousands of peaks costs
    /// candidates × separation², not candidates × peaks, and its memory follows its pixels, not its
    /// box. A component of one candidate, the most common, keeps it with no map.
    fn find_local_maxima(
        self,
        component: &Component<'_>,
        pixels: &mut ComponentPixels,
        kept: Kept<'_>,
    ) {
        let Self {
            min_separation,
            min_relative_height,
        } = self;
        let Kept {
            maxima,
            peaks,
            occupied,
        } = kept;
        let residual = component.residual();
        let min_peak_value = component.peak().value * min_relative_height;
        maxima.clear();
        maxima.extend(
            component
                .pixels()
                .filter(|&p| p.value >= min_peak_value && is_local_maximum(p, residual)),
        );
        maxima.sort_unstable_by(Pixel::brighter_first);
        peaks.clear();
        if maxima.len() < 2 {
            peaks.extend_from_slice(maxima);
            return;
        }
        pixels.fill(component);

        occupied.clear();
        occupied.resize(pixels.pixels.len(), false);
        let reach = min_separation.saturating_sub(1);
        let min_sep_sq = min_separation * min_separation;
        for &candidate in maxima.iter() {
            let pos = candidate.pos();
            let crowded = (pos.y.saturating_sub(reach)..=pos.y + reach).any(|y| {
                (pos.x.saturating_sub(reach)..=pos.x + reach).any(|x| {
                    let near = USizeVec2::new(x, y);
                    pixels.idx_of(near).is_some_and(|idx| occupied[idx])
                        && deblend::dist_sq(pos, near) < min_sep_sq
                })
            });
            if !crowded {
                occupied[pixels
                    .idx_of(pos)
                    .expect("a maximum is a pixel of the component")] = true;
                peaks.push(candidate);
            }
        }
    }
}

/// The local-maxima candidates [`LocalMaximaParams::find_local_maxima`] ranks, where it puts the
/// peaks it keeps, and the per-pixel map of them it checks each candidate's neighbourhood in.
#[derive(Debug)]
struct Kept<'a> {
    maxima: &'a mut Vec<Pixel>,
    peaks: &'a mut Vec<Pixel>,
    occupied: &'a mut Vec<bool>,
}

/// Whether `pixel` is a local maximum of `residual`: at least as high as each of its 8
/// neighbours, and strictly higher than those before it in raster order, the order
/// [`Pixel::brighter_first`] breaks ties in. Of an equal-valued patch the pixels with no equal
/// neighbour before them are candidates, so a flat top is never left without one; a neighbour
/// outside the image does not count against it.
#[inline]
fn is_local_maximum(pixel: Pixel, residual: &Buffer2<f32>) -> bool {
    let x = pixel.pos().x;
    let y = pixel.pos().y;
    let v = pixel.value;
    let width = residual.width();
    let height = residual.height();

    // Before in raster order: the row above, and the left neighbour.
    (y == 0 || x == 0 || residual[(x - 1, y - 1)] < v)
        && (y == 0 || residual[(x, y - 1)] < v)
        && (y == 0 || x + 1 >= width || residual[(x + 1, y - 1)] < v)
        && (x == 0 || residual[(x - 1, y)] < v)
        // After it: the right neighbour, and the row below.
        && (x + 1 >= width || residual[(x + 1, y)] <= v)
        && (y + 1 >= height || x == 0 || residual[(x - 1, y + 1)] <= v)
        && (y + 1 >= height || residual[(x, y + 1)] <= v)
        && (y + 1 >= height || x + 1 >= width || residual[(x + 1, y + 1)] <= v)
}

#[cfg(test)]
mod tests;
